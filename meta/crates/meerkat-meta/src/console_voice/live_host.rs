//! Console composition over Meerkat's shared live host and generated custody.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::experimental_gpt_live::{
    ExperimentalGptLiveOpenAuthority, ExperimentalLiveOpenAuthorityProvider as _,
    PublicGptLiveOpenAuthorityConfig, PublicGptLivePlaybackPolicy, PublicGptLiveProvisionalCaption,
    PublicGptLiveProvisionalCaptionRetraction, PublicGptLiveProvisionalCaptionSink,
};
use meerkat::session_runtime::live_summary::{
    LiveContextBootstrapMode, LiveContextSummarizer, LiveContextSummaryError,
    LiveContextSummaryPolicy, LiveContextSummarySnapshot,
};
use meerkat_core::{Config, SessionId, SessionLlmIdentity};
use meerkat_session::{PersistentSessionService, SessionAgentBuilder};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use super::auth::{ConsoleLiveBindingAuthority, ConsoleLiveGrant};
use super::summary_window::{SummaryCache, SummaryKey, recent_window};
use super::{
    ConsoleVoiceController, ConsoleVoiceHost, ConsoleVoiceSession, VoiceCaptionBatch,
    VoiceCaptionHub, VoiceCaptionRegistration, VoiceContextPreparation, VoiceError,
};
use crate::access::{ACTION_AGENT_SEND, ACTION_AGENT_VIEW, AccessController};
use crate::live_contracts::{ExperimentalLiveChannelStatus, PendingLiveChannelHandle};
use crate::live_wiring::{
    AuthenticatedHttpLiveAuthority, LiveCapabilityProvider, LiveOperation, LiveRpcHandler,
    LiveRpcResponseDeliveryCustody, LiveSurfaceAuthority, capture_live_rpc_response_delivery,
};
use crate::public_live_config::{ConsoleVoiceSummaryConfig, PublicLiveRegistration};
use crate::unified_runtime::UnifiedRuntime;

const PROFILE: &str = "openai.gpt-live-1.client-context.v1";

/// Admission ceiling handed to Meerkat's summary policy. The policy refuses a
/// snapshot above this size instead of windowing it; the configured
/// `max_input_bytes` window is cut below, inside the summariser.
const SUMMARY_CAPTURE_CEILING_BYTES: usize = 4 * 1024 * 1024;
const SUMMARY_TIMEOUT: Duration = Duration::from_mins(1);

struct FactorySummarizer {
    factory: meerkat::AgentFactory,
    config: Config,
    machine: Arc<meerkat_runtime::MeerkatMachine>,
    summary: ConsoleVoiceSummaryConfig,
    cache: SummaryCache,
}

#[async_trait]
impl LiveContextSummarizer for FactorySummarizer {
    async fn summarize(
        &self,
        snapshot: LiveContextSummarySnapshot<'_>,
    ) -> Result<String, LiveContextSummaryError> {
        let started = std::time::Instant::now();
        let mut identity = snapshot.llm_identity().clone();
        if let Some(model) = &self.summary.model {
            identity.model.clone_from(model);
        }
        // Newest messages that fit the configured window; a size bound only.
        // A newest message larger than the window is summarised whole (the
        // bound degrades, it does not fail); Meerkat's capture ceiling has
        // already refused a truly oversized snapshot.
        let window = recent_window(snapshot.messages(), self.summary.max_input_bytes);
        let key = SummaryKey::new(
            snapshot.session_id(),
            snapshot.canonical_message_cursor(),
            &identity.model,
            window,
        )
        .map_err(|_| LiveContextSummaryError::Producer("summary input encoding".to_string()))?;
        if let Some(text) = self.cache.get(&key) {
            tracing::info!(
                model = %identity.model,
                elapsed_ms = started.elapsed().as_millis(),
                output_bytes = text.len(),
                window_messages = window.len(),
                total_messages = snapshot.messages().len(),
                cache = "hit",
                "console voice context summary finished"
            );
            return Ok(text);
        }
        let client = self
            .factory
            .build_llm_client_for_identity_with_auth_lease(
                &self.config,
                &identity,
                Some(self.machine.generated_auth_lease_handle()),
            )
            .await
            .map_err(|_| {
                LiveContextSummaryError::Producer("summary client is unavailable".to_string())
            })?;
        let result = super::summary::summarize_context(
            client.as_ref(),
            &identity.model,
            window,
            snapshot.max_output_bytes(),
        )
        .await;
        tracing::info!(
            model = %identity.model,
            elapsed_ms = started.elapsed().as_millis(),
            output_bytes = ?result.as_ref().ok().map(String::len),
            window_messages = window.len(),
            total_messages = snapshot.messages().len(),
            cache = "miss",
            error = ?result.as_ref().err(),
            "console voice context summary finished"
        );
        let text = result.map_err(|error| {
            LiveContextSummaryError::Producer(format!("summary rejected: {error:?}"))
        })?;
        self.cache.insert(key, text.clone());
        Ok(text)
    }
}

/// Meerkat calls this synchronously on the provider observation path, in
/// delta order and outside its locks; the hub only replaces one bounded
/// buffer entry and wakes readers, so it never waits on the browser.
impl PublicGptLiveProvisionalCaptionSink for VoiceCaptionHub {
    fn publish(&self, caption: PublicGptLiveProvisionalCaption) {
        VoiceCaptionHub::publish_text(
            self,
            caption.session_id(),
            caption.channel_id().as_str(),
            caption.item_id(),
            caption.text(),
        );
    }

    fn retract(&self, retraction: PublicGptLiveProvisionalCaptionRetraction) {
        VoiceCaptionHub::retract_item(
            self,
            retraction.session_id(),
            retraction.channel_id().as_str(),
            retraction.item_id(),
        );
    }
}

struct SharedHost {
    handle: meerkat_mob::MobHandle,
    // Exported with the voice host: the owning public view.
    identity_runtime: Option<crate::identity_first::PublicIdentityRuntime>,
    access: AccessController,
    binding: Arc<ConsoleLiveBindingAuthority>,
    authority: Arc<ExperimentalGptLiveOpenAuthority>,
    handler: LiveRpcHandler,
    principal: String,
    selection: meerkat_contracts::WireLiveExecutionIdentityOverrideV1,
    captions: Arc<VoiceCaptionHub>,
}

struct Host(Arc<SharedHost>);

/// Console voice's public observation publisher. Playback-output
/// publications are refused (console's unmeasured policy never requests
/// one); barge-in playback hints go to the call's caption stream, which the
/// browser applies to its playback gain.
struct ConsoleLivePublisher {
    captions: Arc<VoiceCaptionHub>,
}

#[async_trait]
impl meerkat::experimental_gpt_live::ExperimentalLivePublicObservationPublisher
    for ConsoleLivePublisher
{
    async fn publish(
        &self,
        _observation: meerkat::experimental_gpt_live::ExperimentalLivePublicObservation,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLivePublicObservationDeliveryError>
    {
        // Console's unmeasured policy must never request a played-output
        // publication. Reject a policy mismatch rather than attest delivery.
        Err(meerkat::experimental_gpt_live::ExperimentalLivePublicObservationDeliveryError::Rejected)
    }

    /// `live/assistant_playback_hint` for console voice: the newest hint
    /// replaces the channel's entry in the caption stream. Never waits on
    /// the browser; a hint for a session without a console call is dropped.
    async fn publish_playback_hint(
        &self,
        binding: meerkat_live::ProviderWebrtcBinding,
        hint: meerkat::experimental_gpt_live::ExperimentalLivePlaybackHint,
    ) -> Result<(), meerkat::experimental_gpt_live::ExperimentalLivePublicObservationDeliveryError>
    {
        let hint = match hint {
            meerkat::experimental_gpt_live::ExperimentalLivePlaybackHint::Duck => {
                super::captions::VoicePlaybackHint::Duck
            }
            meerkat::experimental_gpt_live::ExperimentalLivePlaybackHint::Restore => {
                super::captions::VoicePlaybackHint::Restore
            }
            _ => {
                return Err(meerkat::experimental_gpt_live::ExperimentalLivePublicObservationDeliveryError::Rejected);
            }
        };
        self.captions.publish_playback_hint(
            binding.session_id(),
            binding.channel_id().as_str(),
            hint,
        );
        Ok(())
    }
}

impl ConsoleVoiceController {
    /// Compose one console-only public Live registration. A gateway must not
    /// simultaneously install a different live context-mirror owner.
    /// Console voice as the gateway's only live door: composes its own live
    /// context over `service`/`machine` and needs no arbitration.
    pub fn with_live_host<B: SessionAgentBuilder + 'static>(
        runtime: &Arc<UnifiedRuntime>,
        service: Arc<PersistentSessionService<B>>,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        factory: meerkat::AgentFactory,
        config: Config,
        registration: PublicLiveRegistration,
    ) -> Result<Self, String> {
        let ctx = Arc::new(crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            String::new(),
            None,
        ));
        Self::compose_live_host(runtime, ctx, service, machine, factory, registration, None)
    }

    /// Console voice beside the external live channel: both doors open
    /// member channels through `ctx` (one adapter host, one provider
    /// registration) and take turns on its voice-path arbiter, newest
    /// engagement first.
    pub fn with_shared_live_context<B: SessionAgentBuilder + 'static>(
        runtime: &Arc<UnifiedRuntime>,
        ctx: Arc<crate::live_wiring::GatewayLiveContext>,
        service: Arc<PersistentSessionService<B>>,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        factory: meerkat::AgentFactory,
        registration: PublicLiveRegistration,
    ) -> Result<Self, String> {
        Self::compose_live_host(runtime, ctx, service, machine, factory, registration, None)
    }

    pub(crate) fn compose_live_host<B: SessionAgentBuilder + 'static>(
        runtime: &Arc<UnifiedRuntime>,
        ctx: Arc<crate::live_wiring::GatewayLiveContext>,
        service: Arc<PersistentSessionService<B>>,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        factory: meerkat::AgentFactory,
        registration: PublicLiveRegistration,
        summary_override: Option<(LiveContextSummaryPolicy, String)>,
    ) -> Result<Self, String> {
        let summary_config = registration.summary.clone();
        let access = runtime
            .access_controller()
            .cloned()
            .unwrap_or_else(AccessController::disabled);
        let binding = Arc::new(ConsoleLiveBindingAuthority::new(
            runtime.mob_handle(),
            Arc::clone(&machine),
            access.clone(),
            registration.binding.clone(),
        ));
        let arbiter = Arc::clone(&ctx.arbiter);
        let transport =
            Arc::new(meerkat::experimental_gpt_live::ExperimentalGptLiveWebrtcTransport::new());
        let captions = Arc::new(VoiceCaptionHub::default());
        let authority =
            ExperimentalGptLiveOpenAuthority::new_public(PublicGptLiveOpenAuthorityConfig {
                agent_factory: factory.clone(),
                config_source: ctx.experimental_live_config_source(),
                binding_authority: binding.clone(),
                execution_identity: SessionLlmIdentity {
                    provider: meerkat_core::Provider::OpenAI,
                    model: meerkat::GPT_LIVE_PUBLIC_MODEL.to_string(),
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: Some(registration.binding.clone()),
                },
                realm: registration.realm.clone(),
                transport: transport.clone(),
                voice: registration.voice,
                session_instructions: registration.session_instructions,
                // Per-member identity, peers, tools and skills, resolved at
                // open for the target session; see `capabilities`.
                session_instructions_preface: Some(Arc::new(
                    super::capabilities::MemberPreface::new(runtime.mob_handle()),
                )),
            })
            .and_then(|authority| {
                authority.with_public_playback_policy(
                    PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured,
                )
            })
            // The canonical assistant row lands when its segment seals; the
            // console shows the in-progress text from these captions.
            .and_then(|authority| {
                authority.with_public_provisional_caption_sink(
                    Arc::clone(&captions) as Arc<dyn PublicGptLiveProvisionalCaptionSink>
                )
            })
            .map_err(|error| error.to_string())?;
        // Unit fixtures must never negotiate with the real provider.
        #[cfg(test)]
        let authority = authority.with_test_base_url(
            summary_override
                .as_ref()
                .map_or("http://127.0.0.1:9", |(_, url)| url.as_str()),
        );
        let authority = Arc::new(authority);
        let summary = match summary_override {
            Some((policy, _)) => policy,
            None => LiveContextSummaryPolicy::new(
                Arc::new(FactorySummarizer {
                    factory: factory.clone(),
                    config: ctx.config_source.config().clone(),
                    machine: Arc::clone(&machine),
                    summary: summary_config.clone(),
                    cache: SummaryCache::default(),
                }),
                SUMMARY_CAPTURE_CEILING_BYTES,
                summary_config.max_output_bytes,
                SUMMARY_TIMEOUT,
            )
            .map_err(|error| error.to_string())?,
        }
        .with_bootstrap_mode(LiveContextBootstrapMode::Concurrent);
        let capability = LiveCapabilityProvider::public(
            Arc::new(factory),
            registration.realm,
            authority.clone(),
            transport,
            runtime
                .mob_runtime()
                .agent_mob_mcp_state()
                .ok_or("console voice requires the Mob MCP owner")?,
            Arc::new(ConsoleLivePublisher {
                captions: Arc::clone(&captions),
            }),
        );
        let handler = crate::live_wiring::live_rpc_handler_with_console_policy(
            ctx, service, machine, capability, summary,
        );
        let selection = serde_json::from_value(json!({ "version": "v1", "profile_id": PROFILE }))
            .map_err(|error| format!("invalid public live profile: {error}"))?;
        Ok(Self {
            host: Some(Arc::new(Host(Arc::new(SharedHost {
                handle: runtime.mob_handle(),
                identity_runtime: runtime.identity_runtime(),
                access,
                binding,
                authority,
                handler,
                principal: registration.principal,
                selection,
                captions,
            })))),
            ..Self::default()
        }
        .with_arbiter(arbiter))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        Base64BlobStoreAdapter, BinaryBlobStore, DiscoverySpec, MetaConfig, MobBootstrapOptions,
        MobBootstrapSpec, ObjectStoreBlobStore,
    };
    use std::sync::atomic::AtomicUsize;
    use tower::ServiceExt as _;

    // These real hosts share upstream process-wide projection budgets: Core
    // admits one live open projection per process at a time and refuses a
    // concurrent one. Every test in this crate that opens a real live channel
    // takes this lock (the unified runtime's builder tests too).
    pub(crate) static SHARED_HOST_TEST_LOCK: Mutex<()> = Mutex::const_new(());

    /// Console voice puts Meerkat's barge-in hint on the call's caption
    /// stream for the channel the hint names; the newest hint wins.
    #[tokio::test]
    async fn console_publisher_routes_playback_hints_to_the_caption_stream() {
        use meerkat::experimental_gpt_live::{
            ExperimentalLivePlaybackHint, ExperimentalLivePublicObservationPublisher,
        };
        let captions = Arc::new(VoiceCaptionHub::default());
        let session = meerkat_core::SessionId::new();
        let registration = captions.register(&session);
        let publisher = ConsoleLivePublisher {
            captions: Arc::clone(&captions),
        };
        let binding = meerkat_live::ProviderWebrtcBinding::new(
            meerkat_live::LiveChannelId::new("voice-channel-a"),
            session.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(1),
            meerkat_live::LiveRuntimeBindingFence::new(1),
        );
        publisher
            .publish_playback_hint(binding.clone(), ExperimentalLivePlaybackHint::Duck)
            .await
            .expect("a duck hint is published");
        assert_eq!(
            registration
                .read("voice-channel-a", 0)
                .expect("read")
                .captions,
            vec![super::super::captions::VoiceCaption::PlaybackHint {
                hint: super::super::captions::VoicePlaybackHint::Duck
            }]
        );
        publisher
            .publish_playback_hint(binding, ExperimentalLivePlaybackHint::Restore)
            .await
            .expect("a restore hint is published");
        assert_eq!(
            registration
                .read("voice-channel-a", 0)
                .expect("read")
                .captions,
            vec![super::super::captions::VoiceCaption::PlaybackHint {
                hint: super::super::captions::VoicePlaybackHint::Restore
            }],
            "the newest hint replaces the channel's entry"
        );
    }

    #[derive(Default)]
    pub(crate) struct ProviderCapture {
        creates: AtomicUsize,
        body: StdMutex<Option<Value>>,
        disconnect: tokio::sync::Notify,
        disconnected: tokio::sync::Notify,
        /// The concurrent bootstrap summary. Meerkat seeds a summary that is
        /// ready before the provider open as a developer `session.input` item
        /// and delivers a late one on the thinking lane only after the user's
        /// first turn on the channel, never as spoken commentary.
        /// Wire fragments (500 chars each) concatenated in arrival order.
        summary_append: StdMutex<Option<String>>,
        /// Summary fragments received so far (thinking lane).
        summary_received: AtomicUsize,
        /// Fragments acknowledged so far; acks are held until the test
        /// releases them, then every received fragment is acknowledged.
        summary_acknowledged: AtomicUsize,
        release_summary_append: tokio::sync::Notify,
        /// Every thinking-lane append, the late summary included.
        thinking_received: AtomicUsize,
        /// Instructions-lane appends. Meerkat 0.8.41 sends none for the
        /// summary: a ready one is seeded at create, a late one rides the
        /// thinking lane. Acknowledged immediately.
        instructions_received: AtomicUsize,
        /// The test asks the fixture to make the user speak: one
        /// `session.input_transcript.delta`, the provider event Meerkat
        /// treats as the user's first turn on the channel.
        speak: tokio::sync::Notify,
        /// The test asks the fixture to make the assistant speak: one
        /// `session.output_transcript.delta` per queued fragment, in order.
        /// gpt-live-1 names no item on these events.
        say: tokio::sync::Notify,
        say_fragments: StdMutex<Vec<String>>,
    }

    // Only the external provider is simulated. HTTP, WebSocket sideband,
    // shared Meerkat custody and activation are exercised through real owners.
    pub(crate) struct ProviderFixture {
        pub(crate) url: String,
        pub(crate) capture: Arc<ProviderCapture>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for ProviderFixture {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl ProviderFixture {
        pub(crate) async fn start() -> Self {
            use axum::extract::State;
            use axum::extract::ws::{Message as SocketMessage, WebSocketUpgrade};
            use axum::response::IntoResponse as _;
            use axum::routing::{get, post};
            async fn create(
                State(capture): State<Arc<ProviderCapture>>,
                axum::Json(body): axum::Json<Value>,
            ) -> impl axum::response::IntoResponse {
                capture.creates.fetch_add(1, Ordering::SeqCst);
                *capture.body.lock().expect("capture") = Some(body);
                (
                    axum::http::StatusCode::CREATED,
                    axum::Json(json!({
                        "session":{"id":"console_voice_fixture"},
                        "transport":{"type":"webrtc","sdp":"v=0\r\nCONSOLE_FIXTURE_ANSWER"}
                    })),
                )
            }
            async fn attach(
                State(capture): State<Arc<ProviderCapture>>,
                upgrade: WebSocketUpgrade,
            ) -> axum::response::Response {
                upgrade.on_upgrade(move |mut socket| async move {
                    let session = json!({"id":"console_voice_fixture","model":"gpt-live-1","status":"active","expires_at":12345.5});
                    socket.send(SocketMessage::Text(json!({
                        "type":"session.started","event_id":"started","session":session
                    }).to_string().into())).await.expect("session started");
                    let mut held_summary_acks: Vec<Value> = Vec::new();
                    let mut summary_released = false;
                    loop {
                        let message = tokio::select! {
                            () = capture.disconnect.notified() => {
                                drop(socket);
                                capture.disconnected.notify_one();
                                return;
                            }
                            () = capture.release_summary_append.notified(), if !summary_released => {
                                summary_released = true;
                                for client_event_id in held_summary_acks.drain(..) {
                                    socket.send(SocketMessage::Text(json!({
                                        "type":"session.thinking.appended","event_id":"thinking-ack",
                                        "client_event_id":client_event_id,"start_ms":0.0,"end_ms":0.0
                                    }).to_string().into())).await.expect("thinking acknowledgement");
                                    capture.summary_acknowledged.fetch_add(1, Ordering::SeqCst);
                                }
                                continue;
                            }
                            () = capture.speak.notified() => {
                                socket.send(SocketMessage::Text(json!({
                                    "type":"session.input_transcript.delta","event_id":"first-user-delta",
                                    "delta":"hello","start_ms":0.0,"end_ms":600.0
                                }).to_string().into())).await.expect("user transcript delta");
                                continue;
                            }
                            () = capture.say.notified() => {
                                let fragments = std::mem::take(&mut *capture.say_fragments.lock().expect("say fragments"));
                                for (index, delta) in fragments.into_iter().enumerate() {
                                    let start_ms = 1000.0 + 100.0 * index as f64;
                                    socket.send(SocketMessage::Text(json!({
                                        "type":"session.output_transcript.delta","event_id":format!("assistant-delta-{index}"),
                                        "delta":delta,"start_ms":start_ms,"end_ms":start_ms + 100.0
                                    }).to_string().into())).await.expect("assistant transcript delta");
                                }
                                continue;
                            }
                            message = socket.recv() => message,
                        };
                        let Some(Ok(message)) = message else { break; };
                        let SocketMessage::Text(text) = message else { continue; };
                        let event: Value = serde_json::from_str(&text).expect("provider command");
                        match event["type"].as_str() {
                            Some("session.instructions.append") => {
                                capture.instructions_received.fetch_add(1, Ordering::SeqCst);
                                socket.send(SocketMessage::Text(json!({
                                    "type":"session.instructions.appended","event_id":"instructions-ack",
                                    "client_event_id":event["event_id"],"start_ms":0.0,"end_ms":0.0
                                }).to_string().into())).await.expect("instructions acknowledgement");
                            }
                            Some("session.thinking.append") => {
                                // The late summary arrives on the thinking
                                // lane as pipelined fragments; the ack of
                                // each is held until the test releases the
                                // lane so the "delivering" stage stays
                                // observable.
                                let fragment = event["content"].as_str().unwrap_or_default().to_string();
                                capture
                                    .summary_append
                                    .lock()
                                    .expect("summary capture")
                                    .get_or_insert_with(String::new)
                                    .push_str(&fragment);
                                capture.summary_received.fetch_add(1, Ordering::SeqCst);
                                capture.thinking_received.fetch_add(1, Ordering::SeqCst);
                                if summary_released {
                                    socket.send(SocketMessage::Text(json!({
                                        "type":"session.thinking.appended","event_id":"thinking-ack",
                                        "client_event_id":event["event_id"],"start_ms":0.0,"end_ms":0.0
                                    }).to_string().into())).await.expect("thinking acknowledgement");
                                    capture.summary_acknowledged.fetch_add(1, Ordering::SeqCst);
                                } else {
                                    held_summary_acks.push(event["event_id"].clone());
                                }
                            }
                            Some("session.commentary.append") => {
                                socket.send(SocketMessage::Text(json!({
                                    "type":"session.commentary.appended","event_id":"append-ack",
                                    "client_event_id":event["event_id"],"start_ms":0.0,"end_ms":0.0
                                }).to_string().into())).await.expect("append acknowledgement");
                            }
                            Some("session.close") => {
                                socket.send(SocketMessage::Text(json!({
                                    "type":"session.closed","event_id":"closed","session":session,
                                    "reason":"close_requested","usage":{"seconds":0.1}
                                }).to_string().into())).await.expect("close acknowledgement");
                                break;
                            }
                            _ => {}
                        }
                    }
                }).into_response()
            }
            let capture = Arc::new(ProviderCapture::default());
            let app = axum::Router::new()
                .route("/health", get(|| async { "ok" }))
                .route("/v1/live/sessions", post(create))
                .route("/v1/live/sessions/{session_id}/attach", get(attach))
                .with_state(capture.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture listener");
            let address = listener.local_addr().expect("fixture address");
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.expect("fixture server");
            });
            let health = reqwest::get(format!("http://{address}/health"))
                .await
                .expect("fixture health");
            assert_eq!(health.status(), reqwest::StatusCode::OK);
            Self {
                url: format!("http://{address}/v1/"),
                capture,
                server,
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum SummaryScenario {
        Success,
        Failure,
        CancelWhileGenerating,
    }

    pub(crate) struct Summary {
        pub(crate) calls: Arc<AtomicUsize>,
        pub(crate) release: Arc<tokio::sync::Notify>,
        pub(crate) cancelled: Arc<std::sync::atomic::AtomicBool>,
        pub(crate) scenario: SummaryScenario,
    }

    struct SummaryCancellation {
        cancelled: Arc<std::sync::atomic::AtomicBool>,
        finished: bool,
    }

    impl Drop for SummaryCancellation {
        fn drop(&mut self) {
            if !self.finished {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    #[async_trait]
    impl LiveContextSummarizer for Summary {
        async fn summarize(
            &self,
            snapshot: LiveContextSummarySnapshot<'_>,
        ) -> Result<String, LiveContextSummaryError> {
            assert_eq!(snapshot.llm_identity().model, "gpt-5.5");
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut cancellation = SummaryCancellation {
                cancelled: self.cancelled.clone(),
                finished: false,
            };
            self.release.notified().await;
            cancellation.finished = true;
            if self.scenario == SummaryScenario::Failure {
                Err(LiveContextSummaryError::Producer(
                    "private fixture producer detail".to_string(),
                ))
            } else {
                Ok("The background agent is configured for the test conversation.".to_string())
            }
        }
    }

    pub(crate) fn config() -> Config {
        let mut config = Config::default();
        let mut realm = meerkat_core::RealmConfigSection::default();
        realm.backend.insert(
            "api".to_string(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".to_string(),
                backend_kind: "openai_api".to_string(),
                base_url: None,
                options: Value::Null,
                server: None,
            },
        );
        realm.auth.insert(
            "key".to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".to_string(),
                auth_method: "api_key".to_string(),
                source: meerkat_core::CredentialSourceSpec::InlineSecret {
                    secret: "sk-voice-test-only".to_string(),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        realm.binding.insert(
            "openai".to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "api".to_string(),
                auth_profile: "key".to_string(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default: true,
            },
        );
        config.realm.insert("voice".to_string(), realm);
        config
    }

    pub(crate) async fn rpc(app: &axum::Router, token: &str, method: &str, params: Value) -> Value {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/console/rpc")
                    .header("content-type", "application/json")
                    .header("authorization", format!("Bearer {token}"))
                    .body(axum::body::Body::from(
                        json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
                    ))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .expect("body"),
        )
        .expect("response JSON")
    }

    #[tokio::test]
    async fn console_voice_actual_shared_host_opens_owned_pending_channel_without_mutating_background()
     {
        exercise_shared_host(false, SummaryScenario::Success).await;
    }

    #[tokio::test]
    async fn console_voice_actual_shared_host_closes_after_provider_disconnect() {
        exercise_shared_host(true, SummaryScenario::Success).await;
    }

    #[tokio::test]
    async fn console_voice_concurrent_summary_failure_is_typed_and_does_not_gate_activation() {
        exercise_shared_host(false, SummaryScenario::Failure).await;
    }

    #[tokio::test]
    async fn console_voice_close_cancels_exact_pending_summary_and_fences_status() {
        exercise_shared_host(false, SummaryScenario::CancelWhileGenerating).await;
    }

    async fn wait_context(
        app: &axum::Router,
        token: &str,
        pending: &Value,
        expected: Value,
    ) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let status = rpc(app, token, super::super::VOICE_CONTEXT_STATUS_METHOD, json!({
                    "identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"],
                })).await;
                assert!(status["error"].is_null(), "{status}");
                if status["result"]["context_preparation"] == expected { break status; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("actual context preparation transition")
    }

    /// Everything a real-host test needs: a fixture provider, SQLite stores,
    /// the persistent service, the machine, the factory and a bootstrapped
    /// runtime with member `agent-a` spawned.
    struct RealRuntime {
        _directory: tempfile::TempDir,
        provider: ProviderFixture,
        runtime: Arc<UnifiedRuntime>,
        service: Arc<PersistentSessionService<meerkat::FactoryAgentBuilder>>,
        machine: Arc<meerkat_runtime::MeerkatMachine>,
        factory: meerkat::AgentFactory,
        config: Config,
        /// Whole-blob session body reads through the runtime store.
        body_reads: Arc<AtomicUsize>,
        /// The counting runtime store the service and machine read through,
        /// for tests that read a member's body directly.
        runtime_store: Arc<dyn meerkat_runtime::RuntimeStore>,
        /// What the scripted stuck-tool client observed.
        stuck_tool: Arc<StuckToolProbe>,
    }

    /// Trigger text: the scripted client answers it with a `shell` call
    /// that never returns within the test.
    const STUCK_TOOL_TRIGGER: &str = "STUCK_TOOL_TRIGGER";

    /// Answers every turn with "ok", except a turn whose newest user message
    /// is [`STUCK_TOOL_TRIGGER`] (and has no tool result yet): that one calls
    /// the `shell` tool with a command that outlives the test.
    #[derive(Default)]
    struct StuckToolProbe {
        /// Notified when the client asked for the never-returning tool.
        started: tokio::sync::Notify,
        /// Model calls that carried a tool result: the tool returned.
        tool_results: AtomicUsize,
    }

    struct StuckToolClient {
        probe: Arc<StuckToolProbe>,
    }

    #[async_trait]
    impl meerkat_client::LlmClient for StuckToolClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> meerkat_client::types::LlmStream<'a> {
            use meerkat_client::{LlmDoneOutcome, LlmEvent};
            if matches!(
                request.messages.last(),
                Some(meerkat_core::Message::ToolResults { .. })
            ) {
                self.probe.tool_results.fetch_add(1, Ordering::SeqCst);
            }
            // `internal_turn` content arrives as an external-event notice.
            let wants_tool = matches!(
                request.messages.last(),
                Some(meerkat_core::Message::SystemNotice(notice))
                    if notice.body.as_deref() == Some(STUCK_TOOL_TRIGGER)
            );
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let events = if wants_tool {
                self.probe.started.notify_one();
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "stuck-call".to_string(),
                        name: "shell".to_string(),
                        args: json!({"command": "sleep 60"}),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                    },
                ]
            } else {
                vec![
                    LlmEvent::TextDelta {
                        delta: "ok".to_string(),
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    },
                ]
            };
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
            Ok(())
        }
    }

    impl RealRuntime {
        async fn start(suffix: &str) -> Self {
            Self::start_with(suffix, false).await
        }

        /// `stuck_tool`: members get the shell tool and the scripted
        /// [`StuckToolClient`] instead of the plain test client.
        async fn start_with(suffix: &str, stuck_tool: bool) -> Self {
            // Several tests share a logical suffix and `cargo test` runs them
            // on parallel threads in one process. The mob id becomes the comms
            // participant name and the discovery namespace is shared, so two
            // fixtures with the same suffix collided on
            // `ParticipantNameOccupied` and saw each other's history rows.
            // Derive both from a process-unique nonce.
            static FIXTURE_NONCE: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let nonce = FIXTURE_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let unique = format!("{suffix}-{}-{nonce}", std::process::id());
            let provider = ProviderFixture::start().await;
            std::fs::create_dir_all(".rct").expect("test state parent");
            let directory = tempfile::Builder::new()
                .prefix("console-voice-")
                .tempdir_in(".rct")
                .expect("test state");
            let store: Arc<dyn meerkat::SessionStore> = Arc::new(
                meerkat_store::SqliteSessionStore::open(directory.path().join("sessions.sqlite"))
                    .expect("session store"),
            );
            let body_reads = Arc::new(AtomicUsize::new(0));
            let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
                super::super::body_read_counting_store::BodyReadCountingRuntimeStore {
                    inner: Arc::new(
                        meerkat_runtime::store::SqliteRuntimeStore::new(
                            directory.path().join("runtime.sqlite"),
                        )
                        .expect("runtime store"),
                    ),
                    body_reads: Arc::clone(&body_reads),
                },
            );
            let stuck_tool_probe = Arc::new(StuckToolProbe::default());
            let binary: Arc<dyn BinaryBlobStore> = Arc::new(ObjectStoreBlobStore::memory());
            let blobs: Arc<dyn meerkat_core::BlobStore> =
                Arc::new(Base64BlobStoreAdapter::new(binary.clone()));
            let machine = Arc::new(
                meerkat_runtime::MeerkatMachine::persistent(runtime_store.clone(), blobs.clone())
                    .expect("acquire the live voice fixture runtime machine"),
            );
            let factory = meerkat::AgentFactory::new(directory.path())
                .session_store(store.clone())
                .runtime_root(directory.path())
                .project_root(directory.path())
                .builtins(stuck_tool)
                .shell(stuck_tool)
                .mob(true)
                .comms(true);
            let config = config();
            let client: Arc<dyn meerkat_client::LlmClient> = if stuck_tool {
                Arc::new(StuckToolClient {
                    probe: Arc::clone(&stuck_tool_probe),
                })
            } else {
                Arc::new(meerkat_client::TestClient::for_provider(
                    meerkat_core::Provider::OpenAI,
                ))
            };
            let mut builder = meerkat::FactoryAgentBuilder::new(factory.clone(), config.clone());
            builder.default_llm_client = Some(client.clone());
            builder.default_blob_store = Some(blobs.clone());
            let jobs: Arc<dyn meerkat::DetachedJobStore> =
                Arc::new(meerkat::MemoryDetachedJobStore::new());
            builder.default_detached_job_store = Some(Arc::clone(&jobs));
            let delivery = crate::mob_handle_runtime::MobRuntimeDelivery::new(
                Arc::clone(&runtime_store),
                jobs,
            );
            let mob_tools = Arc::clone(&builder.default_mob_tools);
            let service = Arc::new(PersistentSessionService::new(
                builder,
                16,
                store,
                runtime_store.clone(),
                blobs,
            ));
            let definition = meerkat_mob::MobDefinition::from_toml(&format!(
                r#"
    [mob]
    id = "console-voice-{unique}"
    [profiles.agent]
    model = "gpt-5.5"
    external_addressable = true
    skills = ["voice-notes"]
    [profiles.agent.tools]
    comms = true
    shell = {stuck_tool}
    [skills.voice-notes]
    source = "inline"
    content = "Keep spoken answers short."
    "#,
            ))
            .expect("definition");
            let mut spec = MobBootstrapSpec::new(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                service.clone(),
            )
            .with_session_runtime_adapter(machine.clone())
            .expect("acquire the fixture session runtime owner")
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(client),
            });
            spec = spec
                .with_runtime_delivery(delivery)
                .with_agent_mob_tools(mob_tools)
                .expect("install the live voice fixture's agent mob tools");
            spec.runtime_adapter = Some(machine.clone());
            spec.binary_blob_store = Some(binary);
            let runtime = Arc::new(
                UnifiedRuntime::bootstrap(
                    spec,
                    MetaConfig {
                        modules: vec![],
                        pre_spawn: vec![],
                        discovery: DiscoverySpec {
                            namespace: format!("voice-test-{unique}"),
                            modules: vec![],
                        },
                    },
                    Duration::from_secs(2),
                )
                .await
                .expect("runtime"),
            );
            runtime
                .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                    "agent".to_string(),
                    "agent-a".to_string(),
                    None,
                    None,
                    None,
                ))
                .await
                .expect("member");
            // The spawn returns once the member is durable, but its kickoff
            // turn ("You have been spawned as ...", answered by the test
            // client) commits asynchronously. Tests snapshot the source
            // transcript and assert that voice setup leaves it unchanged, so
            // a kickoff row landing between their `before` and `after` reads
            // is a false rewrite. Wait for the kickoff exchange to be durable
            // before handing the fixture out; on a two-core runner the gap
            // was wide enough to trip the assertion.
            let session = runtime
                .mob_handle()
                .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-a"))
                .await
                .expect("source session for kickoff settle");
            crate::test_wait::poll_until(
                "the spawn kickoff exchange of agent-a is durable",
                crate::test_wait::STRUCTURAL_BACKSTOP,
                async || {
                    let snapshot = service
                        .export_realtime_refresh_session_snapshot(&session)
                        .await
                        .expect("kickoff settle snapshot");
                    let messages = snapshot.messages();
                    let kickoff_seen = messages.iter().any(|message| match message {
                        meerkat_core::types::Message::User(user) => user.content.iter().any(|block| {
                            matches!(block, meerkat_core::types::ContentBlock::Text { text } if text.starts_with("You have been spawned as"))
                        }),
                        _ => false,
                    });
                    kickoff_seen && matches!(messages.last(), Some(meerkat_core::types::Message::BlockAssistant(_)))
                },
            )
            .await;
            Self {
                _directory: directory,
                provider,
                runtime,
                service,
                machine,
                factory,
                config,
                body_reads,
                runtime_store,
                stuck_tool: stuck_tool_probe,
            }
        }
    }
    /// The console host over a real runtime, with the fixture provider.
    fn real_console_host(
        runtime: &Arc<UnifiedRuntime>,
        service: &Arc<PersistentSessionService<meerkat::FactoryAgentBuilder>>,
        machine: &Arc<meerkat_runtime::MeerkatMachine>,
        factory: &meerkat::AgentFactory,
        config: &Config,
        provider_url: &str,
    ) -> ConsoleVoiceController {
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: Arc::new(AtomicUsize::new(0)),
                release: Arc::new(tokio::sync::Notify::new()),
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                scenario: SummaryScenario::Success,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        let registration = PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration");
        let ctx = Arc::new(crate::live_wiring::attach_live(
            Arc::clone(service),
            Arc::clone(machine),
            factory,
            config.clone(),
            String::new(),
            None,
        ));
        ConsoleVoiceController::compose_live_host(
            runtime,
            ctx,
            Arc::clone(service),
            Arc::clone(machine),
            factory.clone(),
            registration,
            Some((policy, provider_url.to_string())),
        )
        .expect("console host")
    }

    async fn timed_readiness(controller: &ConsoleVoiceController, identity: &str) -> Duration {
        let started = std::time::Instant::now();
        let flight = controller
            .readiness_flight("voice@example.com", identity)
            .expect("flight");
        let report = tokio::time::timeout(Duration::from_secs(5), flight.result)
            .await
            .expect("readiness answers within the server budget")
            .expect("readiness");
        assert!(report.available, "{identity} must be voice-ready");
        started.elapsed()
    }

    /// meerkat 0.8.47: readiness observes the durable source body-free and
    /// actor-free. With a ~20 MB whole-blob member, 500 polls read the body
    /// zero times, which the counting store checks directly. Their poll time
    /// is a per-poll proxy for body-sized work: the median poll stays under
    /// 100 ms. Such work slows every poll and so moves the median, while a
    /// loaded host stretches only the slow tail, which the median ignores.
    /// The positive control reads the member's body straight from the
    /// counting store, which the session service's verified-body cache
    /// (meerkat 0.8.49) cannot answer.
    #[tokio::test]
    async fn readiness_reads_no_body_of_a_large_whole_blob_member_across_five_hundred_polls() {
        const BODY_BYTES: usize = 20 * 1024 * 1024;
        const POLLS: usize = 500;
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let fixture = RealRuntime::start("readiness-large-body").await;
        let handle = fixture.runtime.mob_handle();
        let member = meerkat_mob::AgentIdentity::from("agent-a");
        let session = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("member session");
        // ~20 MB of external user content appended straight into the
        // member's session and persisted into its whole-blob body (no model
        // turn: a debug-build turn over 20 MB takes minutes).
        fixture
            .service
            .append_external_user_content(&session, "x".repeat(BODY_BYTES).into())
            .await
            .expect("append large content");
        let snapshot = fixture
            .service
            .export_realtime_refresh_session_snapshot(&session)
            .await
            .expect("snapshot");
        assert!(
            snapshot.messages().iter().any(|message| match message {
                meerkat_core::types::Message::User(user) => user.content.iter().any(|block| {
                    matches!(block, meerkat_core::types::ContentBlock::Text { text } if text.len() >= BODY_BYTES)
                }),
                _ => false,
            }),
            "the ~20 MB content is in the durable body"
        );
        let controller = real_console_host(
            &fixture.runtime,
            &fixture.service,
            &fixture.machine,
            &fixture.factory,
            &fixture.config,
            &fixture.provider.url,
        );
        // Warm the path once (first-use lazy initialization), then measure.
        timed_readiness(&controller, "agent-a").await;
        let before = fixture.body_reads.load(Ordering::SeqCst);
        let mut samples = Vec::with_capacity(POLLS);
        for _ in 0..POLLS {
            samples.push(timed_readiness(&controller, "agent-a").await);
        }
        assert_eq!(
            fixture.body_reads.load(Ordering::SeqCst),
            before,
            "readiness polls must never read the session body"
        );
        samples.sort();
        // The median, not a tail percentile: the bound is meant for a poll
        // that does body-sized work, which slows every poll. In loaded
        // full-suite runs on a shared host the median stayed under 15 ms
        // while 23-65 polls stalled for 50-435 ms, so a tail percentile
        // measures the host.
        let median = samples[POLLS / 2];
        let p99 = samples[(POLLS * 99).div_ceil(100) - 1];
        eprintln!("readiness over a ~20 MB member: p50 {median:?}, p99 {p99:?}");
        assert!(
            median < Duration::from_millis(100),
            "median readiness {median:?} over {POLLS} polls (all: {samples:?})"
        );
        // The open path's full validation still accepts the member. Since
        // meerkat 0.8.49 the session service may answer its body load from
        // the verified body it cached when the content was saved, so it no
        // longer proves the counter works.
        handle
            .member(&member)
            .await
            .expect("member handle")
            .validate_live_durable_source_availability()
            .await
            .expect("full durable validation");
        // Positive control: read this member's committed body straight from
        // the store the service and machine share, the read a readiness
        // body load would make on a cache miss. The counter must see it, so
        // the zero above is a measurement and not a disconnected counter.
        let before_control = fixture.body_reads.load(Ordering::SeqCst);
        let committed = fixture
            .runtime_store
            .load_committed_whole_blob_snapshot(&meerkat_runtime::LogicalRuntimeId::for_session(
                &session,
            ))
            .await
            .expect("direct committed body read")
            .expect("the member has a committed whole-blob body");
        assert!(
            committed.session().messages().iter().any(|message| match message {
                meerkat_core::types::Message::User(user) => user.content.iter().any(|block| {
                    matches!(block, meerkat_core::types::ContentBlock::Text { text } if text.len() >= BODY_BYTES)
                }),
                _ => false,
            }),
            "the directly read body is the member's ~20 MB body"
        );
        assert_eq!(
            fixture.body_reads.load(Ordering::SeqCst),
            before_control + 1,
            "the counting store observes a whole-blob body read"
        );
        controller.shutdown().await.expect("voice shutdown");
        fixture.runtime.shutdown().await;
    }

    /// A member stuck in a tool call that never returns keeps its turn open
    /// and its session busy; readiness does not wait on either.
    #[tokio::test]
    async fn readiness_stays_available_while_the_member_is_stuck_in_a_tool_call() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let fixture = RealRuntime::start_with("readiness-stuck-tool", true).await;
        let handle = fixture.runtime.mob_handle();
        let member = meerkat_mob::AgentIdentity::from("agent-a");
        let controller = real_console_host(
            &fixture.runtime,
            &fixture.service,
            &fixture.machine,
            &fixture.factory,
            &fixture.config,
            &fixture.provider.url,
        );
        timed_readiness(&controller, "agent-a").await;
        let started = fixture.stuck_tool.started.notified();
        handle
            .member(&member)
            .await
            .expect("member handle")
            .internal_turn(STUCK_TOOL_TRIGGER.to_string())
            .await
            .expect("stuck turn");
        tokio::time::timeout(Duration::from_secs(30), started)
            .await
            .expect("the member called the never-returning tool");
        let before = fixture.body_reads.load(Ordering::SeqCst);
        for _ in 0..10 {
            let elapsed = timed_readiness(&controller, "agent-a").await;
            assert!(
                elapsed < Duration::from_secs(1),
                "readiness took {elapsed:?} while the member is stuck in a tool call"
            );
        }
        assert_eq!(
            fixture.stuck_tool.tool_results.load(Ordering::SeqCst),
            0,
            "the tool never returned while readiness was polled"
        );
        assert_eq!(fixture.body_reads.load(Ordering::SeqCst), before);
        controller.shutdown().await.expect("voice shutdown");
        fixture.runtime.shutdown().await;
    }

    /// Readiness resolves each member's own session with one typed lookup
    /// (no roster scan), the preface finds a session's owner the same way,
    /// and the stage trace follows the real probe through Meta's
    /// binding-authority callbacks in order.
    #[tokio::test]
    async fn readiness_target_is_one_typed_lookup_and_traces_every_probe_stage() {
        use super::super::readiness_trace::VoiceReadinessStage;
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("readiness-target").await;
        for peer in ["agent-b", "agent-c"] {
            runtime
                .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                    "agent".to_string(),
                    peer.to_string(),
                    None,
                    None,
                    None,
                ))
                .await
                .expect("peer member");
        }
        let handle = runtime.mob_handle();
        let members = handle.list_members().await;
        assert_eq!(members.len(), 3);
        for name in ["agent-a", "agent-b", "agent-c"] {
            let member = meerkat_mob::AgentIdentity::from(name);
            let session = handle
                .resolve_bridge_session_id(&member)
                .await
                .expect("member session");
            let target = crate::rpc::resolve_live_member_target(
                &handle,
                runtime.identity_runtime_ref(),
                false,
                name,
            )
            .await
            .expect("typed target");
            assert_eq!(
                target,
                crate::rpc::LiveMemberTarget {
                    member: member.clone(),
                    session: Some(session.clone()),
                },
                "the resolver names the member whose binding it read"
            );
            assert_eq!(
                super::super::capabilities::bridge_session_owner(&handle, &members, &session).await,
                Some(member),
                "the preface finds the owner without a clone per member"
            );
        }
        assert_eq!(
            super::super::capabilities::bridge_session_owner(
                &handle,
                &members,
                &meerkat_core::SessionId::new()
            )
            .await,
            None
        );
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: Arc::new(AtomicUsize::new(0)),
                release: Arc::new(tokio::sync::Notify::new()),
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                scenario: SummaryScenario::Success,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        let registration = PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration");
        let ctx = Arc::new(crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            String::new(),
            None,
        ));
        let controller = ConsoleVoiceController::compose_live_host(
            &runtime,
            ctx,
            service.clone(),
            machine,
            factory,
            registration,
            Some((policy, provider.url.clone())),
        )
        .expect("console host");
        let flight = controller
            .readiness_flight("voice@example.com", "agent-b")
            .expect("flight");
        let trace = Arc::clone(&flight.trace);
        assert!(flight.result.await.expect("readiness").available);
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.active, None, "{snapshot}");
        assert_eq!(
            snapshot
                .stages
                .iter()
                .map(|entry| entry.stage)
                .collect::<Vec<_>>(),
            vec![
                VoiceReadinessStage::Arbiter,
                VoiceReadinessStage::Target,
                VoiceReadinessStage::Probe,
                VoiceReadinessStage::DurableSource,
                VoiceReadinessStage::BindingSelection,
                VoiceReadinessStage::BindingAuthorization,
                VoiceReadinessStage::Credential,
            ],
            "{snapshot}"
        );
        controller.shutdown().await.expect("voice shutdown");
        runtime.shutdown().await;
    }

    /// Console voice and the external `meta/live/*` door share one live
    /// context and take turns on its voice-path arbiter, newest engagement
    /// first, each loser closed through its own sequence with a typed reason.
    #[tokio::test]
    async fn console_voice_and_external_live_take_turns_on_the_shared_voice_path() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let contract: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/console_voice_v1.json"))
                .expect("shared voice contract");
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("shared-owner").await;
        runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "agent".to_string(),
                "agent-b".to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("external member");
        let session_b = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-b"))
            .await
            .expect("external member session");

        // ONE live context for both doors, exactly as the gateway composes it.
        // The external door's provider open goes through the context's
        // realtime session factory, which in production resolves a real
        // credential; unit fixtures must never negotiate with the provider,
        // so the shared context carries the scripted factory instead (the
        // console door negotiates through the fixture provider above).
        let realtime =
            Arc::new(meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new());
        let mut shared = crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            "ws://127.0.0.1/shared-owner".to_string(),
            None,
        );
        shared.session_factory = Arc::clone(&realtime)
            as Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>;
        let ctx = Arc::new(shared);
        let notifications: Arc<StdMutex<Vec<(String, Value)>>> = Arc::default();
        let sink = Arc::clone(&notifications);
        ctx.arbiter.set_notifier(Arc::new(move |method, params| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((method.to_string(), params));
        }));
        let release_summary = Arc::new(tokio::sync::Notify::new());
        release_summary.notify_one();
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: Arc::new(AtomicUsize::new(0)),
                release: Arc::clone(&release_summary),
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                scenario: SummaryScenario::Success,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        let registration = PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration");
        let controller = ConsoleVoiceController::compose_live_host(
            &runtime,
            Arc::clone(&ctx),
            service.clone(),
            Arc::clone(&machine),
            factory.clone(),
            registration,
            Some((policy, provider.url.clone())),
        )
        .expect("console host");
        let external = crate::live_wiring::live_rpc_handler_with_capabilities(
            Arc::clone(&ctx),
            service.clone(),
            Arc::clone(&machine),
            crate::live_wiring::LiveCapabilityProvider::disabled(),
        );
        let external_open = |rpc_id: &str| {
            external.dispatch(
                crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session_b.clone()),
                Some("agent-b".to_string()),
                "meta/live/open".to_string(),
                // The member's text model is not realtime-capable; the device daemon
                // selects the realtime lane per open exactly like this.
                json!({"identity":"agent-b", "model":"gpt-realtime-2"}),
                json!(rpc_id),
            )
        };
        let external_call = |method: &str, params: Value, rpc_id: &str| {
            external.dispatch(
                crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session_b.clone()),
                Some("agent-b".to_string()),
                method.to_string(),
                params,
                json!(rpc_id),
            )
        };
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret":"console-test-signing", "email_allowlist":["voice@example.com"]
        }))
        .expect("auth");
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &json!({
                "iss":"http://127.0.0.1/meta-gateway","aud":"persistent-gateway",
                "sub":"voice@example.com","email":"voice@example.com","exp":chrono::Utc::now().timestamp()+300
            }), &jsonwebtoken::EncodingKey::from_secret(b"console-test-signing")).expect("token");
        let app = runtime
            .build_reference_app_router(decisions)
            .layer(axum::Extension(controller.clone()));

        // Phase 1: the console holds the path. Its channel is bound to the
        // engagement, so the external door knows exactly what it preempts.
        let opened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-first"}),
        )
        .await;
        assert!(opened["error"].is_null(), "{opened}");
        let console_channel = opened["result"]["channel_id"]
            .as_str()
            .expect("console channel")
            .to_string();
        assert_eq!(
            ctx.arbiter.holder().await,
            Some(crate::live_wiring::LiveOwner::ConsoleVoice {
                principal: "voice@example.com".to_string(),
                identity: "agent-a".to_string(),
                channel_id: Some(console_channel.clone()),
            })
        );

        // Phase 2: an external open preempts the console call. The open
        // result names what it superseded; the console learns why through
        // its own replacement poll; readiness names the new holder; the
        // stdio notification fires at once.
        let external_first = external_open("external-first").await;
        assert!(external_first.error.is_none(), "{external_first:?}");
        let external_result = external_first.result.clone().expect("external open result");
        let external_channel = external_result["channel_id"]
            .as_str()
            .expect("external channel")
            .to_string();
        assert_eq!(
            external_result["superseded"],
            json!({"owner":"console_voice","identity":"agent-a","channel_id":console_channel}),
        );
        assert_eq!(
            realtime.open_count(),
            1,
            "the external door opened through the scripted realtime factory"
        );
        let replacement = rpc(
            &app,
            &token,
            "meta/console/voice/replacement",
            json!({"identity":"agent-a","request_id":"voice-first"}),
        )
        .await;
        assert_eq!(
            replacement["error"], contract["superseded_error"],
            "{replacement}"
        );
        let readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        let mut expected_readiness = contract["readiness_external_live_active"].clone();
        expected_readiness["holder"]["channel_id"] = json!(external_channel);
        assert_eq!(readiness["result"], expected_readiness, "{readiness}");
        {
            let notified = notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(notified.len(), 1, "{notified:?}");
            assert_eq!(notified[0].0, "meta/live/superseded");
            assert_eq!(notified[0].1["owner"], "console_voice");
            assert_eq!(notified[0].1["identity"], "agent-a");
            assert_eq!(notified[0].1["channel_id"], console_channel);
            assert_eq!(notified[0].1["reason"], "superseded_by_external_live");
            assert_eq!(notified[0].1["superseded_by"]["owner"], "external_live");
        }
        // The console's own close of the superseded call is idempotent.
        let closed = rpc(
            &app,
            &token,
            "meta/console/voice/close",
            json!({"identity":"agent-a","request_id":"voice-first"}),
        )
        .await;
        assert_eq!(closed["result"], json!({"phase":"closed"}), "{closed}");
        assert_eq!(
            ctx.arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live"),
            "a superseded console close never evicts the external owner"
        );

        // Phase 3: a console open takes the path back. The external channel is
        // closed through the external door's own sequence and its status
        // reports the typed reason; the notification names the console.
        let reopened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-second"}),
        )
        .await;
        assert!(reopened["error"].is_null(), "{reopened}");
        let second_console_channel = reopened["result"]["channel_id"]
            .as_str()
            .expect("second console channel")
            .to_string();
        let status = external_call(
            "meta/live/status",
            json!({"identity":"agent-b","channel_id":external_channel}),
            "external-status",
        )
        .await;
        assert!(status.error.is_none(), "{status:?}");
        let status = status.result.expect("status result");
        assert_eq!(status["status"], json!({"status":"closed"}), "{status}");
        assert_eq!(
            status["close_reason"], "superseded_by_console_voice",
            "{status}"
        );
        {
            let notified = notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(notified.len(), 2, "{notified:?}");
            assert_eq!(notified[1].1["owner"], "external_live");
            assert_eq!(notified[1].1["identity"], "agent-b");
            assert_eq!(notified[1].1["channel_id"], external_channel);
            assert_eq!(notified[1].1["reason"], "superseded_by_console_voice");
            assert_eq!(notified[1].1["superseded_by"]["owner"], "console_voice");
        }
        let readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(
            readiness["result"], contract["readiness_available"],
            "{readiness}"
        );

        // Phase 4: the external door wins again, then closes on its own. An
        // ordinary close frees the path and carries no close reason.
        let external_second = external_open("external-second").await;
        assert!(external_second.error.is_none(), "{external_second:?}");
        let external_result = external_second.result.expect("second external open");
        assert_eq!(
            external_result["superseded"],
            json!({"owner":"console_voice","identity":"agent-a","channel_id":second_console_channel}),
        );
        let second_external_channel = external_result["channel_id"]
            .as_str()
            .expect("second external channel")
            .to_string();
        let replacement = rpc(
            &app,
            &token,
            "meta/console/voice/replacement",
            json!({"identity":"agent-a","request_id":"voice-second"}),
        )
        .await;
        assert_eq!(
            replacement["error"], contract["superseded_error"],
            "{replacement}"
        );
        let closed = external_call(
            "meta/live/close",
            json!({"identity":"agent-b","channel_id":second_external_channel}),
            "external-close",
        )
        .await;
        assert!(closed.error.is_none(), "{closed:?}");
        let closed = closed.result.expect("close result");
        assert_eq!(closed["status"], "closed", "{closed}");
        assert!(closed.get("close_reason").is_none(), "{closed}");
        assert!(ctx.arbiter.holder().await.is_none());
        let readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(
            readiness["result"], contract["readiness_available"],
            "{readiness}"
        );

        // Phase 5: the external channel ends inside meerkat-live (its socket
        // dropped) without passing through the door. The stale holder must
        // not lock the console out: readiness is available and the console
        // opens without preempting anything.
        let external_third = external_open("external-third").await;
        assert!(external_third.error.is_none(), "{external_third:?}");
        let third_external_channel =
            external_third.result.expect("third external open")["channel_id"]
                .as_str()
                .expect("third external channel")
                .to_string();
        assert_eq!(
            ctx.arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
        let direct_host = crate::live_wiring::member_live_host_for_test(&ctx, &service, &machine);
        direct_host
            .close_live_channel(
                None,
                &meerkat_core::LiveChannelId::new(&third_external_channel),
            )
            .await
            .expect("meerkat-side close of the external channel");
        let readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(
            readiness["result"], contract["readiness_available"],
            "a dead external channel must not hold the path: {readiness}"
        );
        let notifications_before = notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        let third_console = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-third"}),
        )
        .await;
        assert!(third_console["error"].is_null(), "{third_console}");
        assert_eq!(
            notifications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            notifications_before,
            "opening over a dead holder supersedes nobody"
        );
        assert_eq!(
            ctx.arbiter.holder().await.map(|owner| owner.kind()),
            Some("console_voice")
        );
        assert_eq!(
            realtime.open_count(),
            3,
            "every external open in this test went through the scripted factory"
        );
        controller.shutdown().await.expect("voice shutdown");
        runtime.shutdown().await;
    }
    /// Provider speech reaches the console as provisional captions through
    /// the real sink Meerkat calls, keyed by the segment item id that the
    /// committed assistant row later records in
    /// `realtime_origin.provider_item_ids` on the same session and channel.
    #[tokio::test]
    async fn console_voice_captions_carry_provider_speech_until_the_committed_row() {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("captions").await;
        let session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-a"))
            .await
            .expect("source session");
        // No summary is delivered, so no provider-acknowledged append forms
        // a between-speech boundary inside the reply below.
        let release_summary = Arc::new(tokio::sync::Notify::new());
        release_summary.notify_one();
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: Arc::new(AtomicUsize::new(0)),
                release: release_summary,
                cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                scenario: SummaryScenario::Failure,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        let registration = PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration");
        let ctx = Arc::new(crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            String::new(),
            None,
        ));
        let controller = ConsoleVoiceController::compose_live_host(
            &runtime,
            ctx,
            service.clone(),
            machine,
            factory,
            registration,
            Some((policy, provider.url.clone())),
        )
        .expect("console host");
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret":"console-test-signing", "email_allowlist":["voice@example.com"]
        }))
        .expect("auth");
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &json!({
                "iss":"http://127.0.0.1/meta-gateway","aud":"persistent-gateway",
                "sub":"voice@example.com","email":"voice@example.com","exp":chrono::Utc::now().timestamp()+300
            }), &jsonwebtoken::EncodingKey::from_secret(b"console-test-signing")).expect("token");
        let app = runtime
            .build_reference_app_router(decisions)
            .layer(axum::Extension(controller.clone()));
        let opened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-request"}),
        )
        .await;
        assert!(opened["error"].is_null(), "{opened}");
        let pending = &opened["result"];
        let channel = pending["channel_id"].as_str().expect("channel").to_string();
        let captions = |after: u64, wait_ms: u64| {
            rpc(
                &app,
                &token,
                super::super::VOICE_CAPTIONS_METHOD,
                json!({"identity":"agent-a","request_id":"voice-request","channel_id":channel,
                    "after":after,"wait_ms":wait_ms}),
            )
        };
        let nothing = captions(0, 0).await;
        assert_eq!(
            nothing["result"],
            json!({"identity":"agent-a","request_id":"voice-request","channel_id":channel,
                "cursor":0,"captions":[]}),
            "{nothing}"
        );
        for mismatch in [
            json!({"identity":"agent-a","request_id":"voice-request","channel_id":"other-channel"}),
            json!({"identity":"agent-a","request_id":"other-request","channel_id":channel}),
        ] {
            let response = rpc(&app, &token, super::super::VOICE_CAPTIONS_METHOD, mismatch).await;
            assert_eq!(
                response["error"]["data"]["kind"], "voice_request_conflict",
                "{response}"
            );
        }
        let unknown_field = rpc(&app, &token, super::super::VOICE_CAPTIONS_METHOD, json!({
            "identity":"agent-a","request_id":"voice-request","channel_id":channel,"item_id":"x",
        }))
        .await;
        assert_eq!(unknown_field["error"]["code"], -32602, "{unknown_field}");

        let registered = rpc(&app, &token, "meta/live/playback_owner/register", json!({
            "identity":"agent-a","channel_id":channel,"pending_receipt":pending["pending_receipt"]
        })).await;
        let answer = rpc(
            &app,
            &token,
            "live/webrtc/answer",
            json!({
                "identity":"agent-a","channel_id":channel,
                "pending_receipt":pending["pending_receipt"],
                "readiness_receipt":registered["result"]["readiness_receipt"],
                "token":pending["transport"]["token"],"offer_sdp":"v=0\r\nCONSOLE_FIXTURE_OFFER"
            }),
        )
        .await;
        assert!(answer["error"].is_null(), "{answer}");
        let delivered = rpc(
            &app,
            &token,
            "meta/console/voice/answer_received",
            json!({
                "identity":"agent-a","request_id":"voice-request","channel_id":channel
            }),
        )
        .await;
        assert_eq!(delivered["result"], json!({"accepted":true}), "{delivered}");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = rpc(&app, &token, "meta/live/status", json!({
                    "identity":"agent-a","channel_id":channel,"pending_receipt":pending["pending_receipt"]
                })).await;
                if status["result"]["phase"] == "active" { break; }
                assert!(status["error"].is_null(), "{status}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("generated activation");
        wait_context(
            &app,
            &token,
            pending,
            json!({"phase":"failed","reason":"generation"}),
        )
        .await;
        provider.capture.speak.notify_one();

        let fragments = ["The vault ", "phrase is ", "amber."];
        *provider
            .capture
            .say_fragments
            .lock()
            .expect("say fragments") = fragments.iter().map(ToString::to_string).collect();
        provider.capture.say.notify_one();
        let spoken = fragments.concat();
        let (item_id, cursor) = tokio::time::timeout(Duration::from_secs(10), async {
            let mut after = 0;
            loop {
                let read = captions(after, 2_000).await;
                assert!(read["error"].is_null(), "{read}");
                after = read["result"]["cursor"].as_u64().expect("cursor");
                let latest = read["result"]["captions"]
                    .as_array()
                    .expect("captions")
                    .last()
                    .cloned();
                if let Some(caption) = latest {
                    assert_eq!(caption["kind"], "caption", "{read}");
                    if caption["text"] == spoken {
                        break (
                            caption["item_id"].as_str().expect("item id").to_string(),
                            after,
                        );
                    }
                }
            }
        })
        .await
        .expect("the provider's speech reaches the console as one growing caption");
        let settled = captions(cursor, 0).await;
        assert_eq!(settled["result"]["captions"], json!([]), "{settled}");
        assert_eq!(settled["result"]["cursor"], cursor);
        let replay = captions(0, 0).await;
        assert_eq!(
            replay["result"]["captions"],
            json!([{"kind":"caption","item_id":item_id,"text":spoken}]),
            "one segment is one caption entry carrying the whole text: {replay}"
        );

        // Close seals the open segment into one canonical assistant row.
        let closed = rpc(
            &app,
            &token,
            "meta/console/voice/close",
            json!({"identity":"agent-a","request_id":"voice-request"}),
        )
        .await;
        assert_eq!(closed["result"], json!({"phase":"closed"}), "{closed}");
        let after_close = captions(cursor, 0).await;
        assert_eq!(
            after_close["error"]["data"]["kind"], "voice_closed",
            "{after_close}"
        );
        let snapshot = service
            .export_realtime_refresh_session_snapshot(&session)
            .await
            .expect("history");
        let rows: Vec<Value> = snapshot
            .messages()
            .iter()
            .map(|message| serde_json::to_value(message).expect("message JSON"))
            .filter(|message| message["role"] == "block_assistant")
            .filter_map(|message| {
                let origin = message
                    .get("realtime_origin")
                    .or_else(|| message.pointer("/identity/realtime_origin"))?
                    .clone();
                Some(origin)
            })
            .collect();
        assert_eq!(
            rows.iter()
                .filter(|origin| origin["provider_item_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == item_id.as_str())))
                .map(|origin| (origin["session_id"].clone(), origin["channel_id"].clone()))
                .collect::<Vec<_>>(),
            vec![(json!(session.to_string()), json!(channel))],
            "exactly one committed row names the caption's item on its session and channel: {rows:?}"
        );
        controller.shutdown().await.expect("voice shutdown");
        runtime.shutdown().await;
    }

    async fn exercise_shared_host(disconnect_provider: bool, scenario: SummaryScenario) {
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let contract: Value =
            serde_json::from_str(include_str!("../../tests/fixtures/console_voice_v1.json"))
                .expect("shared voice contract");
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start(if disconnect_provider {
            "disconnect"
        } else {
            "normal"
        })
        .await;
        // A second member gives the capabilities preface a peer to name.
        runtime
            .spawn(meerkat_mob::SpawnMemberSpec::from_wire(
                "agent".to_string(),
                "agent-b".to_string(),
                None,
                None,
                None,
            ))
            .await
            .expect("peer member");
        let session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-a"))
            .await
            .expect("source session");
        let before = service
            .export_realtime_refresh_session_snapshot(&session)
            .await
            .expect("before");
        let calls = Arc::new(AtomicUsize::new(0));
        let release_summary = Arc::new(tokio::sync::Notify::new());
        let summary_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(Summary {
                calls: calls.clone(),
                release: release_summary.clone(),
                cancelled: summary_cancelled.clone(),
                scenario,
            }),
            4 * 1024 * 1024,
            4096,
            Duration::from_secs(30),
        )
        .expect("summary policy");
        assert_eq!(
            policy.bootstrap_mode(),
            LiveContextBootstrapMode::BeforeOpen,
            "generic upstream policy remains unchanged; only console composition opts in"
        );
        let registration = PublicLiveRegistration::parse(&json!({
            "principal":"voice@example.com","realm":"voice",
            "auth_binding":{"realm":"voice","binding":"openai"},"voice":"marin"
        }))
        .expect("registration");
        let ctx = Arc::new(crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            String::new(),
            None,
        ));
        let controller = ConsoleVoiceController::compose_live_host(
            &runtime,
            ctx,
            service.clone(),
            Arc::clone(&machine),
            factory,
            registration,
            Some((policy, provider.url.clone())),
        )
        .expect("console host");
        assert!(matches!(
            controller.ready("other@example.com", "agent-a").await,
            Err(VoiceError::Unauthorized)
        ));
        assert!(
            controller
                .ready("voice@example.com", "agent-a")
                .await
                .expect("readiness")
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "readiness must not invoke summary production"
        );
        // Open no longer re-probes readiness; the host's own target
        // resolution must still refuse what the probe refused, with the
        // same typed answers the console maps today.
        let open_request = |request_id: &str, identity: &str| super::super::VoiceRequest {
            identity: identity.to_string(),
            request_id: request_id.to_string(),
        };
        assert_eq!(
            controller
                .open("other@example.com", open_request("voice-denied", "agent-a"))
                .await
                .err(),
            Some(VoiceError::Unauthorized),
            "a foreign principal is refused by the open path itself"
        );
        assert_eq!(
            controller
                .open(
                    "voice@example.com",
                    open_request("voice-missing", "agent-missing")
                )
                .await
                .err(),
            Some(VoiceError::Unavailable),
            "an unknown member is unavailable on the open path itself"
        );
        assert!(
            !controller
                .ready("voice@example.com", "agent-missing")
                .await
                .expect("missing member readiness"),
            "readiness and open agree on an unknown member"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "refused opens never reach summary production"
        );
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret":"console-test-signing", "email_allowlist":["voice@example.com"]
        }))
        .expect("auth");
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &json!({
                "iss":"http://127.0.0.1/meta-gateway","aud":"persistent-gateway",
                "sub":"voice@example.com","email":"voice@example.com","exp":chrono::Utc::now().timestamp()+300
            }), &jsonwebtoken::EncodingKey::from_secret(b"console-test-signing")).expect("token");
        let mut read_only_decisions = decisions.clone();
        read_only_decisions.console.read_only = true;
        let read_only_app = runtime
            .build_reference_app_router(read_only_decisions)
            .layer(axum::Extension(controller.clone()));
        let app = runtime
            .build_reference_app_router(decisions)
            .layer(axum::Extension(controller.clone()));
        let readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(readiness["result"], contract["readiness_available"]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "HTTP readiness must not summarize"
        );
        let opened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-request"}),
        )
        .await;
        assert!(opened["error"].is_null(), "{opened}");
        let pending = &opened["result"];
        assert_eq!(pending["execution_mode"], "client_context");
        assert_eq!(pending["target_identity"], "agent-a");
        assert_eq!(pending["capabilities"]["text_in"], false);
        assert_eq!(pending["capabilities"]["text_out"], false);
        let context = wait_context(
            &app,
            &token,
            pending,
            json!({"phase":"preparing","stage":"generating"}),
        )
        .await;
        assert_eq!(
            context["result"],
            json!({
                "identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"],
                "context_preparation":{"phase":"preparing","stage":"generating"},
            })
        );
        let read_only_context = rpc(&read_only_app, &token, super::super::VOICE_CONTEXT_STATUS_METHOD,
            json!({"identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"]})).await;
        assert_eq!(
            read_only_context["result"], context["result"],
            "read-only console permits owned status reads"
        );
        for invalid in [
            json!({"identity":"agent-a","request_id":"voice-request"}),
            json!({"identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]}),
        ] {
            let response = rpc(
                &app,
                &token,
                super::super::VOICE_CONTEXT_STATUS_METHOD,
                invalid,
            )
            .await;
            assert_eq!(response["error"]["code"], -32602);
        }
        for mismatch in [
            json!({"identity":"agent-a","request_id":"other-request","channel_id":pending["channel_id"]}),
            json!({"identity":"other-agent","request_id":"voice-request","channel_id":pending["channel_id"]}),
            json!({"identity":"agent-a","request_id":"voice-request","channel_id":"other-channel"}),
        ] {
            let response = rpc(
                &app,
                &token,
                super::super::VOICE_CONTEXT_STATUS_METHOD,
                mismatch,
            )
            .await;
            assert_eq!(response["error"]["data"]["kind"], "voice_request_conflict");
        }
        assert_eq!(
            controller
                .context_status(
                    "other@example.com",
                    super::super::VoiceContextStatusRequest {
                        identity: "agent-a".to_string(),
                        request_id: "voice-request".to_string(),
                        channel_id: pending["channel_id"].as_str().expect("channel").to_string(),
                    }
                )
                .await,
            Err(VoiceError::RequestConflict)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!opened.to_string().contains("sk-voice-test-only"));
        let registered = rpc(&app, &token, "meta/live/playback_owner/register", json!({
                "identity":"agent-a","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]
            })).await;
        assert!(
            registered["result"]["readiness_receipt"].as_str().is_some(),
            "{registered}"
        );
        let attached_readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(
            attached_readiness["result"], contract["readiness_available"],
            "an existing live attachment must not fail the exact auth probe"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "attached readiness does not summarize or reopen"
        );
        assert_eq!(
            provider.capture.creates.load(Ordering::SeqCst),
            0,
            "readiness, summary, and pending open do not create a provider session"
        );
        let answer = rpc(
            &app,
            &token,
            "live/webrtc/answer",
            json!({
                "identity":"agent-a","channel_id":pending["channel_id"],
                "pending_receipt":pending["pending_receipt"],
                "readiness_receipt":registered["result"]["readiness_receipt"],
                "token":pending["transport"]["token"],"offer_sdp":"v=0\r\nCONSOLE_FIXTURE_OFFER"
            }),
        )
        .await;
        assert_eq!(
            answer["result"]["answer_sdp"], "v=0\r\nCONSOLE_FIXTURE_ANSWER",
            "{answer}"
        );
        let before_ack = rpc(&app, &token, "meta/live/status", json!({
            "identity":"agent-a","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]
        })).await;
        assert_eq!(
            before_ack["result"]["phase"], "pending",
            "SDP response alone cannot activate"
        );
        let delivered = rpc(
            &app,
            &token,
            "meta/console/voice/answer_received",
            json!({
                "identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"]
            }),
        )
        .await;
        assert_eq!(delivered["result"], json!({"accepted":true}), "{delivered}");
        let active = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = rpc(&app, &token, "meta/live/status", json!({
                    "identity":"agent-a","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]
                })).await;
                if status["result"]["phase"] == "active" { break status; }
                assert!(status["error"].is_null(), "{status}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("generated activation");
        assert!(
            active["result"]["handle"]["activation_receipt"]
                .as_str()
                .is_some()
        );
        wait_context(
            &app,
            &token,
            pending,
            json!({"phase":"preparing","stage":"generating"}),
        )
        .await;
        assert_eq!(
            provider.capture.summary_received.load(Ordering::SeqCst),
            0,
            "Active can be reached before summary generation or summary delivery"
        );
        if scenario != SummaryScenario::CancelWhileGenerating {
            release_summary.notify_one();
            if scenario == SummaryScenario::Failure {
                let failed = wait_context(
                    &app,
                    &token,
                    pending,
                    json!({"phase":"failed","reason":"generation"}),
                )
                .await;
                assert!(
                    !failed
                        .to_string()
                        .contains("private fixture producer detail")
                );
                assert_eq!(provider.capture.summary_received.load(Ordering::SeqCst), 0);
            } else {
                // Meerkat 0.8.41 never appends a late summary into silence:
                // generation may finish, but the status stays at generating
                // and no lane carries anything until the user's first turn.
                tokio::time::sleep(Duration::from_millis(300)).await;
                wait_context(
                    &app,
                    &token,
                    pending,
                    json!({"phase":"preparing","stage":"generating"}),
                )
                .await;
                assert_eq!(
                    provider.capture.summary_received.load(Ordering::SeqCst),
                    0,
                    "a late summary must not be appended before the user speaks"
                );
                assert_eq!(
                    provider
                        .capture
                        .instructions_received
                        .load(Ordering::SeqCst),
                    0,
                    "nothing rides the instructions lane before the user speaks"
                );
                provider.capture.speak.notify_one();
                wait_context(
                    &app,
                    &token,
                    pending,
                    json!({"phase":"preparing","stage":"delivering"}),
                )
                .await;
                tokio::time::timeout(Duration::from_secs(5), async {
                    while provider.capture.summary_received.load(Ordering::SeqCst) == 0 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("actual summary append on the thinking lane after the first user turn");
                assert_eq!(
                    provider.capture.summary_acknowledged.load(Ordering::SeqCst),
                    0
                );
                wait_context(
                    &app,
                    &token,
                    pending,
                    json!({"phase":"preparing","stage":"delivering"}),
                )
                .await;
                let summary_append = provider
                    .capture
                    .summary_append
                    .lock()
                    .expect("summary append")
                    .clone()
                    .expect("append");
                assert!(
                    summary_append
                        .contains(meerkat::experimental_gpt_live::LIVE_LATE_SUMMARY_PREFIX),
                    "the late summary must open with the context-data prefix: {summary_append}"
                );
                assert!(
                    summary_append
                        .contains("The background agent is configured for the test conversation."),
                    "the thinking-lane append must carry the generated summary: {summary_append}"
                );
                provider.capture.release_summary_append.notify_one();
                wait_context(
                    &app,
                    &token,
                    pending,
                    json!({"phase":"provider_acknowledged"}),
                )
                .await;
                let fragments = provider.capture.summary_received.load(Ordering::SeqCst);
                assert!(
                    fragments >= 1,
                    "the summary must have arrived in at least one fragment"
                );
                assert_eq!(
                    provider
                        .capture
                        .instructions_received
                        .load(Ordering::SeqCst),
                    0,
                    "the late summary travels on the thinking lane; the instructions lane stays quiet"
                );
                assert_eq!(
                    provider.capture.summary_acknowledged.load(Ordering::SeqCst),
                    fragments,
                    "every summary fragment must be acknowledged exactly once"
                );
            }
        }
        // With the open reservation held throughout, a strict Refresh on the
        // exact activation receipt still reaches the strict adapter, which
        // refuses it locally; a wrong receipt is refused before the adapter,
        // and the channel stays bound. The strict adapter answers every
        // Refresh with its wildcard not-ready refusal and ignores the
        // payload, so the binding read below does not establish the
        // submitted model or history projection; the supported-provider
        // refresh controls cover that. The console redacts every refusal to
        // `voice_host_failed`; the shared host's refusal detail is read from
        // the console's warning.
        let activation_receipt = active["result"]["handle"]["activation_receipt"]
            .as_str()
            .expect("activation receipt")
            .to_string();
        let held_reservation = released_open_reservation("strict refresh").await;
        assert_open_reservation_held("strict refresh, before the calls");
        let (wrong_receipt, wrong_receipt_log) = crate::test_diagnostics::capture_warnings(rpc(
            &app,
            &token,
            "meta/live/refresh",
            json!({
                "identity":"agent-a","channel_id":pending["channel_id"],
                "activation_receipt":"not-the-activation-receipt"
            }),
        ))
        .await;
        let (refreshed, refresh_log) = crate::test_diagnostics::capture_warnings(rpc(
            &app,
            &token,
            "meta/live/refresh",
            json!({
                "identity":"agent-a","channel_id":pending["channel_id"],
                "activation_receipt":activation_receipt
            }),
        ))
        .await;
        assert_open_reservation_held("strict refresh, after both calls");
        drop(held_reservation);
        assert_eq!(
            wrong_receipt["error"]["data"]["kind"], "voice_host_failed",
            "{wrong_receipt}"
        );
        assert!(
            wrong_receipt_log.contains("console live control refused by shared host")
                && !wrong_receipt_log.contains("live channel request rejected")
                && !wrong_receipt_log.contains("adapter not ready"),
            "a wrong receipt is refused before the adapter: {wrong_receipt_log}"
        );
        assert_eq!(
            refreshed["error"]["data"]["kind"], "voice_host_failed",
            "{refreshed}"
        );
        assert!(
            refresh_log.contains("adapter not ready: current status is Ready"),
            "the strict adapter's own refusal of Refresh, not a snapshot-version or enqueue failure: {refresh_log}"
        );
        assert!(
            !refresh_log.contains("failed to build session config"),
            "the refresh projection never needs the open reservation: {refresh_log}"
        );
        let binding = machine
            .live_channel_binding(
                &session,
                &meerkat_core::LiveChannelId::new(
                    pending["channel_id"].as_str().expect("channel id"),
                ),
            )
            .await
            .expect("binding read")
            .expect("the voice channel is bound");
        assert!(
            !binding.follows_session_identity(),
            "the voice channel was opened under its selected profile"
        );
        assert_ne!(
            binding.identity().model,
            "gpt-5.5",
            "the voice channel's realtime identity is not the member's text identity"
        );
        let active_readiness = rpc(
            &app,
            &token,
            "meta/console/voice/readiness",
            json!({"identity":"agent-a"}),
        )
        .await;
        assert_eq!(
            active_readiness["result"], contract["readiness_available"],
            "{active_readiness}"
        );
        assert_eq!(
            provider.capture.creates.load(Ordering::SeqCst),
            1,
            "active probe must not reopen"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "active probe must not summarize"
        );
        let body = provider
            .capture
            .body
            .lock()
            .expect("create body")
            .clone()
            .expect("provider created");
        // Meerkat 0.8.50: an open whose context summary is not ready yet (a
        // Late open) seeds the most recent committed turns verbatim as
        // startup input, at most
        // `meerkat_openai::public_live::LIVE_STARTUP_VERBATIM_ITEMS_MAX`
        // provider items, starting at a user row. The pending-context notice
        // still rides the instructions.
        const LIVE_STARTUP_VERBATIM_ITEMS_MAX: usize = 4;
        let input = body["session"]["input"]
            .as_array()
            .unwrap_or_else(|| panic!("a Late open seeds the recent turns: {body}"));
        assert!(
            !input.is_empty() && input.len() <= LIVE_STARTUP_VERBATIM_ITEMS_MAX,
            "the verbatim startup seed is bounded: {body}"
        );
        assert_eq!(
            input[0]["role"], "user",
            "the verbatim seed starts at a user row: {body}"
        );
        assert!(
            !body
                .to_string()
                .contains("The background agent is configured for the test conversation."),
            "concurrent context must not be required by the provider-create request"
        );
        assert!(body["session"]["tools"].is_null());
        // The capabilities preface opens the session instructions at create
        // time, on its own carrier, whether or not the summary later
        // succeeds: identity and role, the peer, the tool, and the skill.
        assert!(
            body["session"]["instructions"].is_string(),
            "create body must carry session instructions: {body}"
        );
        let instructions = body["session"]["instructions"]
            .as_str()
            .expect("string checked above");
        assert!(
            instructions.starts_with("You speak for agent-a (agent) in mob console-voice-"),
            "the preface must open the session instructions: {instructions}"
        );
        for needle in [
            "You are connected to: agent-b (agent).",
            "available through the executor: comms.",
            "Skills: voice-notes.",
            "answer from this list",
        ] {
            assert!(
                instructions.contains(needle),
                "the preface must carry {needle:?}: {instructions}"
            );
        }
        assert!(
            instructions.contains("do not greet or introduce yourself"),
            "the default continuing-conversation guidance must survive the preface: {instructions}"
        );
        if disconnect_provider {
            provider.capture.disconnect.notify_one();
            provider.capture.disconnected.notified().await;
        }
        let closed = tokio::time::timeout(
            Duration::from_secs(5),
            rpc(
                &app,
                &token,
                "meta/console/voice/close",
                json!({"identity":"agent-a","request_id":"voice-request"}),
            ),
        )
        .await
        .expect("exact close must complete within the browser teardown deadline");
        assert_eq!(closed["result"], json!({"phase":"closed"}), "{closed}");
        if scenario == SummaryScenario::CancelWhileGenerating {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !summary_cancelled.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("close cancels the actual summarizer future");
            assert_eq!(provider.capture.summary_received.load(Ordering::SeqCst), 0);
        }
        let closed_context = rpc(&app, &token, super::super::VOICE_CONTEXT_STATUS_METHOD, json!({
            "identity":"agent-a","request_id":"voice-request","channel_id":pending["channel_id"],
        })).await;
        assert_eq!(closed_context["error"]["data"]["kind"], "voice_closed");
        let closed_replacement = rpc(
            &app,
            &token,
            "meta/console/voice/replacement",
            json!({"identity":"agent-a","request_id":"voice-request"}),
        )
        .await;
        assert_eq!(closed_replacement["error"]["data"]["kind"], "voice_closed");
        let after = service
            .export_realtime_refresh_session_snapshot(&session)
            .await
            .expect("after");
        // Voice setup must not rewrite source history. The one utterance the
        // fixture spoke may have become a canonical user row at the end of
        // the source; every pre-existing row stays byte-identical.
        assert!(
            after.messages().len() >= before.messages().len(),
            "voice setup must not drop source history"
        );
        assert_eq!(
            &after.messages()[..before.messages().len()],
            before.messages(),
            "voice setup must not rewrite source history"
        );
        for appended in &after.messages()[before.messages().len()..] {
            let meerkat_core::Message::User(user) = appended else {
                panic!("only the spoken user utterance may be appended by voice: {appended:?}");
            };
            assert!(
                user.content.iter().any(|block| {
                    matches!(block, meerkat_core::ContentBlock::Text { text } if text.contains("hello"))
                }),
                "an appended row must be the fixture's spoken utterance: {user:?}"
            );
        }
        assert_eq!(
            service
                .live_session_llm_identity(&session)
                .await
                .expect("identity")
                .model,
            "gpt-5.5"
        );
        // agent-a plus the agent-b peer spawned above: voice open adds no member.
        assert_eq!(runtime.mob_handle().list_members().await.len(), 2);
        let reopened = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-reopened"}),
        )
        .await;
        assert!(reopened["error"].is_null(), "{reopened}");
        assert_ne!(reopened["result"]["channel_id"], pending["channel_id"]);
        let stale_context = rpc(&app, &token, super::super::VOICE_CONTEXT_STATUS_METHOD, json!({
            "identity":"agent-a","request_id":"voice-reopened","channel_id":pending["channel_id"],
        })).await;
        assert_eq!(
            stale_context["error"]["data"]["kind"],
            "voice_request_conflict"
        );
        let reclosed = rpc(
            &app,
            &token,
            "meta/console/voice/close",
            json!({"identity":"agent-a","request_id":"voice-reopened"}),
        )
        .await;
        assert_eq!(reclosed["result"], json!({"phase":"closed"}), "{reclosed}");
        controller.shutdown().await.expect("voice shutdown");
        runtime.shutdown().await;
    }

    /// Hang guard for the reservation tests' teardown steps: the console
    /// host's shutdown and the reservation's release after it.
    const VOICE_TEARDOWN_BOUND: Duration = Duration::from_secs(10);
    /// Deadline on each reservation test's isolated child process.
    const VOICE_CHILD_DEADLINE: Duration = Duration::from_secs(90);

    /// The console door over `runtime` with `controller`, and a bearer token
    /// for the fixture principal.
    fn console_app(
        runtime: &Arc<UnifiedRuntime>,
        controller: &ConsoleVoiceController,
    ) -> (axum::Router, String) {
        let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
            "shared_secret":"console-test-signing", "email_allowlist":["voice@example.com"]
        }))
        .expect("auth");
        let token = jsonwebtoken::encode(&jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256), &json!({
                "iss":"http://127.0.0.1/meta-gateway","aud":"persistent-gateway",
                "sub":"voice@example.com","email":"voice@example.com","exp":chrono::Utc::now().timestamp()+300
            }), &jsonwebtoken::EncodingKey::from_secret(b"console-test-signing")).expect("token");
        let app = runtime
            .build_reference_app_router(decisions)
            .layer(axum::Extension(controller.clone()));
        (app, token)
    }

    /// Open a console voice call for `agent-a`; returns the pending channel.
    async fn open_pending_voice(app: &axum::Router, token: &str, request_id: &str) -> Value {
        let opened = rpc(
            app,
            token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":request_id}),
        )
        .await;
        assert!(opened["error"].is_null(), "{opened}");
        opened["result"].clone()
    }

    /// Register the playback owner and deliver the browser offer: the
    /// provider answers, and the channel stays pending until acknowledged.
    async fn answer_pending_voice(app: &axum::Router, token: &str, pending: &Value) {
        let registered = rpc(app, token, "meta/live/playback_owner/register", json!({
                "identity":"agent-a","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]
            })).await;
        assert!(
            registered["result"]["readiness_receipt"].as_str().is_some(),
            "{registered}"
        );
        let answer = rpc(
            app,
            token,
            "live/webrtc/answer",
            json!({
                "identity":"agent-a","channel_id":pending["channel_id"],
                "pending_receipt":pending["pending_receipt"],
                "readiness_receipt":registered["result"]["readiness_receipt"],
                "token":pending["transport"]["token"],"offer_sdp":"v=0\r\nCONSOLE_FIXTURE_OFFER"
            }),
        )
        .await;
        assert_eq!(
            answer["result"]["answer_sdp"], "v=0\r\nCONSOLE_FIXTURE_ANSWER",
            "{answer}"
        );
        let status = rpc(app, token, "meta/live/status", json!({
            "identity":"agent-a","channel_id":pending["channel_id"],"pending_receipt":pending["pending_receipt"]
        })).await;
        assert_eq!(
            status["result"]["phase"], "pending",
            "the answered channel is not yet acknowledged: {status}"
        );
    }

    /// The process-wide realtime open reservation is held: a probe is refused
    /// as backpressured.
    fn assert_open_reservation_held(stage: &str) {
        match meerkat_core::RealtimeOpenProjectionAdmission::global().try_acquire() {
            Err(meerkat_core::RealtimeOpenProjectionAdmissionError::Backpressured { .. }) => {}
            probe => {
                panic!("{stage}: the pending open must hold the open reservation; probe: {probe:?}")
            }
        }
    }

    /// Wait, under a hang guard, until the process-wide realtime open
    /// reservation is free in this process and runtime; returns the probe
    /// lease that observed it.
    async fn released_open_reservation(stage: &str) -> meerkat_core::RealtimeOpenProjectionLease {
        tokio::time::timeout(VOICE_TEARDOWN_BOUND, async {
            loop {
                match meerkat_core::RealtimeOpenProjectionAdmission::global().try_acquire() {
                    Ok(lease) => break lease,
                    Err(meerkat_core::RealtimeOpenProjectionAdmissionError::Backpressured {
                        ..
                    }) => tokio::task::yield_now().await,
                    Err(error) => panic!("{stage}: the open reservation probe failed: {error:?}"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{stage}: the open reservation must be released"))
    }

    /// A pending console open holds the process-wide realtime open
    /// reservation from its open result through the provider answer. An
    /// exact close releases it in the same process and runtime, so a reopen
    /// is admitted and its own close releases it again.
    #[tokio::test]
    async fn console_voice_pending_open_holds_the_open_reservation_until_close() {
        // The open reservation is process-wide.
        if !crate::test_process_isolation::run_body_in_isolated_process_within(
            module_path!(),
            "console_voice_pending_open_holds_the_open_reservation_until_close",
            VOICE_CHILD_DEADLINE,
        )
        .await
        {
            return;
        }
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("reservation-close").await;
        let controller = real_console_host(
            &runtime,
            &service,
            &machine,
            &factory,
            &config,
            &provider.url,
        );
        let (app, token) = console_app(&runtime, &controller);
        let pending = open_pending_voice(&app, &token, "voice-reservation").await;
        assert_open_reservation_held("after the open result");
        answer_pending_voice(&app, &token, &pending).await;
        assert_open_reservation_held("after the provider answer, before acknowledgement");
        let closed = tokio::time::timeout(
            Duration::from_secs(5),
            rpc(
                &app,
                &token,
                "meta/console/voice/close",
                json!({"identity":"agent-a","request_id":"voice-reservation"}),
            ),
        )
        .await
        .expect("exact close must complete within the browser teardown deadline");
        assert_eq!(closed["result"], json!({"phase":"closed"}), "{closed}");
        drop(released_open_reservation("after the close").await);
        let reopened = open_pending_voice(&app, &token, "voice-reservation-reopened").await;
        assert_ne!(reopened["channel_id"], pending["channel_id"]);
        assert_open_reservation_held("after the reopen result");
        let reclosed = rpc(
            &app,
            &token,
            "meta/console/voice/close",
            json!({"identity":"agent-a","request_id":"voice-reservation-reopened"}),
        )
        .await;
        assert_eq!(reclosed["result"], json!({"phase":"closed"}), "{reclosed}");
        drop(released_open_reservation("after the reopened close").await);
        tokio::time::timeout(VOICE_TEARDOWN_BOUND, controller.shutdown())
            .await
            .expect("voice shutdown answers within its bound")
            .expect("voice shutdown");
        runtime.shutdown().await;
    }

    /// A console voice open made while another open on the gateway holds
    /// the process-wide realtime open reservation is refused as the typed,
    /// retryable `voice_open_busy`, never as a host failure. Once that open
    /// lets the reservation go, opening again succeeds.
    #[tokio::test]
    async fn console_voice_open_while_another_open_holds_the_reservation_is_busy() {
        // The open reservation is process-wide.
        if !crate::test_process_isolation::run_body_in_isolated_process_within(
            module_path!(),
            "console_voice_open_while_another_open_holds_the_reservation_is_busy",
            VOICE_CHILD_DEADLINE,
        )
        .await
        {
            return;
        }
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("reservation-busy").await;
        let controller = real_console_host(
            &runtime,
            &service,
            &machine,
            &factory,
            &config,
            &provider.url,
        );
        let (app, token) = console_app(&runtime, &controller);
        // Another open on this gateway is being prepared: it holds the
        // reservation.
        let other_open = released_open_reservation("before the open").await;
        let refused = rpc(
            &app,
            &token,
            "meta/console/voice/open",
            json!({"identity":"agent-a","request_id":"voice-busy"}),
        )
        .await;
        assert_eq!(
            refused["error"]["data"]["kind"], "voice_open_busy",
            "{refused}"
        );
        drop(other_open);
        let opened = open_pending_voice(&app, &token, "voice-busy-again").await;
        assert!(opened["channel_id"].as_str().is_some(), "{opened}");
        let closed = rpc(
            &app,
            &token,
            "meta/console/voice/close",
            json!({"identity":"agent-a","request_id":"voice-busy-again"}),
        )
        .await;
        assert_eq!(closed["result"], json!({"phase":"closed"}), "{closed}");
        tokio::time::timeout(VOICE_TEARDOWN_BOUND, controller.shutdown())
            .await
            .expect("voice shutdown answers within its bound")
            .expect("voice shutdown");
        runtime.shutdown().await;
    }

    /// Shutting the console host down cancels an open that has its open
    /// result but no provider answer yet, and releases the reservation
    /// without ending the process or its runtime.
    #[tokio::test]
    async fn console_voice_shutdown_releases_the_open_reservation_of_an_unanswered_open() {
        // The open reservation is process-wide.
        if !crate::test_process_isolation::run_body_in_isolated_process_within(
            module_path!(),
            "console_voice_shutdown_releases_the_open_reservation_of_an_unanswered_open",
            VOICE_CHILD_DEADLINE,
        )
        .await
        {
            return;
        }
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("reservation-unanswered").await;
        let controller = real_console_host(
            &runtime,
            &service,
            &machine,
            &factory,
            &config,
            &provider.url,
        );
        let (app, token) = console_app(&runtime, &controller);
        let _pending = open_pending_voice(&app, &token, "voice-reservation").await;
        assert_open_reservation_held("after the open result");
        tokio::time::timeout(VOICE_TEARDOWN_BOUND, controller.shutdown())
            .await
            .expect("voice shutdown answers within its bound")
            .expect("voice shutdown");
        drop(released_open_reservation("after shutdown of an unanswered open").await);
        runtime.shutdown().await;
    }

    /// Shutting the console host down cancels an answered open that the
    /// browser has not acknowledged, and releases the reservation without
    /// ending the process or its runtime.
    #[tokio::test]
    async fn console_voice_shutdown_releases_the_open_reservation_of_an_answered_open() {
        // The open reservation is process-wide.
        if !crate::test_process_isolation::run_body_in_isolated_process_within(
            module_path!(),
            "console_voice_shutdown_releases_the_open_reservation_of_an_answered_open",
            VOICE_CHILD_DEADLINE,
        )
        .await
        {
            return;
        }
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            provider,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("reservation-answered").await;
        let controller = real_console_host(
            &runtime,
            &service,
            &machine,
            &factory,
            &config,
            &provider.url,
        );
        let (app, token) = console_app(&runtime, &controller);
        let pending = open_pending_voice(&app, &token, "voice-reservation").await;
        answer_pending_voice(&app, &token, &pending).await;
        assert_open_reservation_held("after the provider answer, before acknowledgement");
        tokio::time::timeout(VOICE_TEARDOWN_BOUND, controller.shutdown())
            .await
            .expect("voice shutdown answers within its bound")
            .expect("voice shutdown");
        drop(released_open_reservation("after shutdown of an answered open").await);
        runtime.shutdown().await;
    }

    /// What a `meta/live/refresh` delivered, beside the durable session it
    /// refreshes from and the open it updates.
    #[derive(Debug)]
    #[allow(dead_code)]
    struct RefreshObservation {
        reply: String,
        refreshes_dispatched: usize,
        seed_rows: Option<usize>,
        model: (String, Option<String>),
        provider: (String, Option<String>),
        rewrite_generation: (String, Option<String>),
        canonical_system_messages_match: Option<bool>,
        user_content_identities_match: Option<bool>,
        user_content_tombstones_match: Option<bool>,
        audio_config: Option<String>,
        contended_reply: String,
        refreshes_after_contended: usize,
        unbound_reply: String,
        absent_reply: String,
        refreshed_again_reply: String,
        refreshes_after_refusals: usize,
        opens: usize,
        close_reply: String,
    }

    /// A refresh over existing canonical history is a configuration-only
    /// update: the adapter receives no seed history (only open replays it),
    /// the currentness sidecars match the durable session, the channel keeps
    /// the realtime model and provider its open bound (not the member's text
    /// model), and the refresh does not contend for the process open
    /// reservation. A channel with no bound identity is refused as an unbound
    /// channel request, never refreshed on the text identity. The external
    /// door opens through the scripted realtime factory, whose adapter records
    /// every command as dispatched; the provider adapter's own command-byte
    /// custody is not part of this scripted adapter.
    #[tokio::test]
    async fn live_refresh_over_existing_history_is_a_history_free_configuration_update() {
        // The open reservation is process-wide.
        if !crate::test_process_isolation::run_body_in_isolated_process(
            module_path!(),
            "live_refresh_over_existing_history_is_a_history_free_configuration_update",
        ) {
            return;
        }
        let _guard = SHARED_HOST_TEST_LOCK.lock().await;
        let RealRuntime {
            _directory,
            runtime,
            service,
            machine,
            factory,
            config,
            ..
        } = RealRuntime::start("refresh").await;
        let session = runtime
            .mob_handle()
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("agent-a"))
            .await
            .expect("source session");
        let durable = service
            .export_realtime_refresh_session_snapshot(&session)
            .await
            .expect("durable session");
        assert!(
            !durable.messages().is_empty(),
            "the source carries canonical history (its spawn kickoff)"
        );
        let realtime =
            Arc::new(meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory::new());
        let mut live = crate::live_wiring::attach_live(
            Arc::clone(&service),
            Arc::clone(&machine),
            &factory,
            config,
            "ws://127.0.0.1/refresh".to_string(),
            None,
        );
        live.session_factory = Arc::clone(&realtime)
            as Arc<dyn meerkat_client::realtime_session::RealtimeSessionFactory>;
        let ctx = Arc::new(live);
        let door = crate::live_wiring::live_rpc_handler_with_capabilities(
            Arc::clone(&ctx),
            service.clone(),
            Arc::clone(&machine),
            crate::live_wiring::LiveCapabilityProvider::disabled(),
        );
        let call = |method: &str, params: Value, rpc_id: &str| {
            door.dispatch(
                crate::live_wiring::LiveSurfaceAuthority::host_trusted_stdio(),
                Some(session.clone()),
                Some("agent-a".to_string()),
                method.to_string(),
                params,
                json!(rpc_id),
            )
        };
        // The member's text model is not realtime-capable; the open selects
        // the realtime model for this channel.
        let opened = call(
            "meta/live/open",
            json!({"identity":"agent-a","model":"gpt-realtime-2"}),
            "refresh-open",
        )
        .await;
        assert!(opened.error.is_none(), "{opened:?}");
        let channel = opened
            .result
            .as_ref()
            .and_then(|result| result["channel_id"].as_str())
            .expect("channel")
            .to_string();
        let channel_id = meerkat_core::LiveChannelId::new(&channel);
        tokio::time::timeout(Duration::from_secs(10), async {
            while machine
                .live_session_for_active_channel(&channel_id)
                .await
                .is_none()
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the opened channel becomes active");
        drop(released_open_reservation("after the open").await);

        let refreshed = call(
            "meta/live/refresh",
            json!({"identity":"agent-a","channel_id":channel}),
            "refresh",
        )
        .await;
        let adapter = realtime
            .adapters()
            .into_iter()
            .next()
            .expect("the scripted adapter");
        let refreshes: Vec<_> = adapter
            .commands()
            .into_iter()
            .filter_map(|command| match command {
                meerkat_core::live_adapter::LiveAdapterCommand::Refresh { snapshot } => {
                    Some(snapshot)
                }
                _ => None,
            })
            .collect();
        let held = meerkat_core::RealtimeOpenProjectionAdmission::global()
            .try_acquire()
            .expect("the open reservation is free before the contended refresh");
        let contended = call(
            "meta/live/refresh",
            json!({"identity":"agent-a","channel_id":channel}),
            "refresh-contended",
        )
        .await;
        drop(held);
        let refresh_count = || {
            adapter
                .commands()
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        meerkat_core::live_adapter::LiveAdapterCommand::Refresh { .. }
                    )
                })
                .count()
        };
        let refreshes_after_contended = refresh_count();

        // While the valid channel is still open: a refresh of a channel the
        // machine never admitted, and the bound-identity read for it, both get
        // the local unbound-channel refusal, never a refresh on the member's
        // text identity.
        let unbound = call(
            "meta/live/refresh",
            json!({"identity":"agent-a","channel_id":"refresh-never-admitted"}),
            "refresh-unbound",
        )
        .await;
        let absent = crate::live_wiring::live_refresh_channel_identity(
            &machine,
            &session,
            &meerkat_core::LiveChannelId::new("refresh-never-admitted"),
            json!("refresh-absent-binding"),
        )
        .await
        .expect_err("an absent channel binding is refused, never replaced");
        // The original channel still refreshes after those refusals.
        let refreshed_again = call(
            "meta/live/refresh",
            json!({"identity":"agent-a","channel_id":channel}),
            "refresh-again",
        )
        .await;
        let refreshes_after_refusals = refresh_count();
        let closed = call(
            "meta/live/close",
            json!({"identity":"agent-a","channel_id":channel}),
            "refresh-close",
        )
        .await;

        let opens = realtime.opens();
        let open_identity = &opens.first().expect("the scripted open").identity;
        let text_identity = service
            .live_session_llm_identity(&session)
            .await
            .expect("the member's text identity");
        assert_ne!(
            text_identity.model, open_identity.model,
            "the channel's realtime model differs from the member's text model"
        );
        let expected_projection =
            meerkat_client::realtime_session::RealtimeSessionOpenConfig::for_open_from_messages(
                meerkat_contracts::RealtimeTurningMode::ProviderManaged,
                open_identity.clone(),
                Vec::new(),
                Vec::new(),
                durable.messages(),
            )
            .expect("the durable projection");
        let rewrite_generation = durable
            .transcript_rewrite_generation()
            .expect("rewrite generation");
        let refresh = refreshes.first();
        let observation = RefreshObservation {
            reply: format!("{:?}", refreshed.result.as_ref().ok_or(&refreshed.error)),
            refreshes_dispatched: refreshes.len(),
            seed_rows: refresh.map(|snapshot| snapshot.seed_messages.len()),
            model: (
                open_identity.model.clone(),
                refresh.map(|snapshot| snapshot.model_id.clone()),
            ),
            provider: (
                format!("{:?}", open_identity.provider),
                refresh.map(|snapshot| format!("{:?}", snapshot.provider_id)),
            ),
            rewrite_generation: (
                format!("{rewrite_generation:?}"),
                refresh.map(|snapshot| format!("{:?}", snapshot.transcript_rewrite_generation)),
            ),
            canonical_system_messages_match: refresh.map(|snapshot| {
                snapshot.canonical_system_messages
                    == expected_projection.canonical_system_messages_ref()
            }),
            user_content_identities_match: refresh.map(|snapshot| {
                format!("{:?}", snapshot.user_content_identities)
                    == format!("{:?}", durable.realtime_user_content_identities())
            }),
            user_content_tombstones_match: refresh.map(|snapshot| {
                format!("{:?}", snapshot.user_content_tombstones)
                    == format!("{:?}", durable.realtime_user_content_tombstones())
            }),
            audio_config: refresh.map(|snapshot| format!("{:?}", snapshot.audio_config)),
            contended_reply: format!("{:?}", contended.result.as_ref().ok_or(&contended.error)),
            refreshes_after_contended,
            unbound_reply: format!("{:?}", unbound.result.as_ref().ok_or(&unbound.error)),
            absent_reply: format!("{:?}", absent.result.as_ref().ok_or(&absent.error)),
            refreshed_again_reply: format!(
                "{:?}",
                refreshed_again
                    .result
                    .as_ref()
                    .ok_or(&refreshed_again.error)
            ),
            refreshes_after_refusals,
            opens: realtime.open_count(),
            close_reply: format!("{:?}", closed.result.as_ref().ok_or(&closed.error)),
        };
        assert!(
            refreshed.error.is_none()
                && refreshed
                    .result
                    .as_ref()
                    .is_some_and(|result| result["status"] == "queued"),
            "the refresh is queued: {observation:#?}"
        );
        assert_eq!(
            observation.refreshes_dispatched, 1,
            "the refresh reached the adapter once: {observation:#?}"
        );
        assert!(
            observation.canonical_system_messages_match == Some(true)
                && observation.user_content_identities_match == Some(true)
                && observation.user_content_tombstones_match == Some(true)
                && observation.rewrite_generation.1.as_ref()
                    == Some(&observation.rewrite_generation.0),
            "the currentness sidecars are the durable session's: {observation:#?}"
        );
        assert_eq!(
            observation.seed_rows,
            Some(0),
            "a refresh carries no seed history; only open replays it: {observation:#?}"
        );
        assert!(
            contended.error.is_none()
                && contended
                    .result
                    .as_ref()
                    .is_some_and(|result| result["status"] == "queued")
                && observation.refreshes_after_contended == 2,
            "a configuration-only refresh under a held open reservation is queued and reaches \
             the adapter: {observation:#?}"
        );
        assert!(
            observation.model.1.as_ref() == Some(&observation.model.0)
                && observation.provider.1.as_ref() == Some(&observation.provider.0)
                && observation.audio_config.as_deref() == Some("None"),
            "a refresh keeps the channel's model and provider and changes configuration only: \
             {observation:#?}"
        );
        assert!(
            closed.error.is_none()
                && closed
                    .result
                    .as_ref()
                    .is_some_and(|result| result["status"] == "closed"),
            "the refreshed channel closes: {observation:#?}"
        );
        // The local unbound-channel class: invalid params (-32602), channel
        // not found.
        let unbound_class = |code: i64, message: &str| {
            code == -32602 && message == "channel refresh-never-admitted not found"
        };
        assert!(
            unbound
                .error
                .as_ref()
                .is_some_and(|error| unbound_class(error.code, &error.message))
                && absent
                    .error
                    .as_ref()
                    .is_some_and(|error| unbound_class(error.code, &error.message)),
            "a never-admitted channel is refused as an unbound channel request, never \
             refreshed on the text identity: {observation:#?}"
        );
        assert!(
            refreshed_again.error.is_none()
                && refreshed_again
                    .result
                    .as_ref()
                    .is_some_and(|result| result["status"] == "queued")
                && observation.refreshes_after_refusals == 3
                && observation.opens == 1,
            "the original channel refreshes again after the refusals, on its one open: \
             {observation:#?}"
        );
        runtime.shutdown().await;
    }
}
impl SharedHost {
    async fn target(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<Arc<ConsoleLiveGrant>, VoiceError> {
        if principal != self.principal {
            return Err(VoiceError::Unauthorized);
        }
        let access = self.access.view_for_subject(Some(principal));
        if !access.allows_agent(ACTION_AGENT_VIEW, identity)
            || !access.allows_agent(ACTION_AGENT_SEND, identity)
        {
            return Err(VoiceError::Unauthorized);
        }
        // Every unavailable answer below names its cause in the log; the wire
        // answer stays the plain typed `Unavailable`.
        let unavailable = |cause: &str| {
            tracing::warn!(identity, cause, "console voice target unavailable");
            VoiceError::Unavailable
        };
        let authoritative = if let Some(runtime) = &self.identity_runtime {
            runtime
                .runtime()
                .member_alias_lifecycle_target(identity)
                .await
                .map_err(|error| unavailable(&format!("identity lifecycle lookup: {error:?}")))?
                .is_some()
        } else {
            false
        };
        // One typed lookup: the resolver names the member whose machine
        // binding it read, so that member is the target session's owner.
        // No roster scan and no per-member machine-state clone.
        let target = crate::rpc::resolve_live_member_target(
            &self.handle,
            self.identity_runtime
                .as_ref()
                .map(crate::identity_first::PublicIdentityRuntime::runtime),
            authoritative,
            identity,
        )
        .await
        .map_err(|error| unavailable(&format!("live target resolution: {error:?}")))?;
        let owner = target.member;
        let session = target
            .session
            .ok_or_else(|| unavailable("no live-capable session for identity"))?;
        self.binding
            .register(principal, owner, session)
            .map_err(|error| match error {
                VoiceError::Busy => {
                    tracing::warn!(
                        identity,
                        "console voice target grant is held by another principal or identity"
                    );
                    VoiceError::Busy
                }
                other => other,
            })
    }

    fn guard(
        &self,
        grant: Arc<ConsoleLiveGrant>,
        identity: String,
    ) -> Result<LiveSurfaceAuthority, VoiceError> {
        let principal = crate::http_console::console_human_principal_ref(&grant.principal)
            .ok_or(VoiceError::Unauthorized)?;
        Ok(LiveSurfaceAuthority::authenticated_http(Arc::new(
            HttpAuthority {
                grant,
                identity,
                principal,
                access: self.access.clone(),
            },
        )))
    }

    async fn open_owned(
        self: &Arc<Self>,
        principal: &str,
        identity: &str,
    ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
        let target_started = std::time::Instant::now();
        // The readiness probe reported a grant held by another principal as
        // plain unavailability; the open path keeps that answer rather than
        // the same-request "teardown pending" retry hint.
        let grant = self
            .target(principal, identity)
            .await
            .map_err(|error| match error {
                VoiceError::Busy => VoiceError::Unavailable,
                other => other,
            })?;
        let target_ms = elapsed_ms(target_started);
        let surface = self.guard(Arc::clone(&grant), identity.to_string())?;
        let live_open_started = std::time::Instant::now();
        let (response, delivery) = capture_live_rpc_response_delivery(self.handler.dispatch(
            surface, Some(grant.session.clone()), Some(identity.to_string()),
            "meta/live/open".to_string(),
            json!({"identity": identity, "transport": "webrtc", "execution_identity": {"version":"v1", "profile_id":PROFILE}}),
            json!("console-voice-open"),
        )).await;
        tracing::info!(
            target: "meerkat_meta::console_voice::timing",
            identity,
            target_ms,
            live_open_ms = elapsed_ms(live_open_started),
            ok = response.error.is_none(),
            "console voice shared host open"
        );
        let pending = match response
            .result
            .and_then(|value| serde_json::from_value::<PendingLiveChannelHandle>(value).ok())
        {
            Some(pending) if response.error.is_none() => pending,
            _ => {
                if let Some(delivery) = delivery {
                    let _ = delivery.rejected().await;
                }
                tracing::warn!(error = ?response.error, "console live open refused by shared host");
                // Meerkat's open admission refuses an unavailable target,
                // credential, binding, or member with the capability code;
                // that is the same answer the readiness probe gave.
                // Another open on this gateway still holds the process-wide
                // open reservation: busy, and a new open succeeds later.
                return Err(match response.error.as_ref().map(|error| error.code) {
                    Some(crate::rpc::CAPABILITY_UNAVAILABLE_CODE) => VoiceError::Unavailable,
                    Some(crate::live_wiring::LIVE_OPEN_BUSY_CODE) => VoiceError::OpenBusy,
                    _ => VoiceError::HostFailed,
                });
            }
        };
        let captions = self.captions.register(&grant.session);
        Ok(Arc::new(Session {
            shared: Arc::clone(self),
            captions,
            grant,
            identity: identity.to_string(),
            current: StdMutex::new(pending.clone()),
            channels: StdMutex::new(vec![pending]),
            deliveries: Mutex::new(Deliveries {
                open: delivery,
                ..Deliveries::default()
            }),
            polls: StdMutex::new(HashMap::new()),
        }))
    }
}

fn elapsed_ms(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[async_trait]
impl ConsoleVoiceHost for Host {
    async fn ready(&self, principal: &str, identity: &str) -> Result<bool, VoiceError> {
        use super::readiness_trace::{VoiceReadinessStage, enter as enter_stage};
        enter_stage(VoiceReadinessStage::Target);
        let target_started = std::time::Instant::now();
        let grant = match self.0.target(principal, identity).await {
            Ok(grant) => grant,
            Err(VoiceError::Unavailable | VoiceError::Busy) => return Ok(false),
            Err(error) => {
                tracing::warn!(
                    identity,
                    ?error,
                    "console voice readiness refused by target authorization"
                );
                return Err(error);
            }
        };
        let target_ms = elapsed_ms(target_started);
        // Shared admission resolves the actual selected configured credential,
        // but does not open/register a provider channel at this preparation seam.
        let probe_started = std::time::Instant::now();
        enter_stage(VoiceReadinessStage::Probe);
        let probe = self
            .0
            .authority
            .probe_execution_readiness(&grant.session, &self.0.selection)
            .await;
        match &probe {
            Ok(()) => tracing::debug!(
                target: "meerkat_meta::console_voice::timing",
                identity,
                target_ms,
                probe_ms = elapsed_ms(probe_started),
                "console voice readiness probe"
            ),
            Err(error) => tracing::warn!(
                target: "meerkat_meta::console_voice::timing",
                identity,
                target_ms,
                probe_ms = elapsed_ms(probe_started),
                cause = %error,
                "console voice readiness probe failed: execution credential not ready"
            ),
        }
        probe.map(|()| true).map_err(|_| VoiceError::Unavailable)
    }

    async fn open(
        &self,
        principal: &str,
        identity: &str,
    ) -> Result<Arc<dyn ConsoleVoiceSession>, VoiceError> {
        let tracked = if let Some(runtime) = &self.0.identity_runtime {
            runtime
                .runtime()
                .member_alias_lifecycle_target(identity)
                .await
                .map_err(|_| VoiceError::Unavailable)?
        } else {
            None
        };
        if let Some(target) = tracked {
            let shared = Arc::clone(&self.0);
            let principal = principal.to_string();
            let identity = identity.to_string();
            crate::identity_first::IdentityRuntime::run_member_alias_targets_operation_tracked(
                vec![target],
                move || async move {
                    shared
                        .open_owned(&principal, &identity)
                        .await
                        .map_err(|error| format!("{error:?}"))
                },
            )
            .await
            .map_err(|_| VoiceError::HostFailed)
        } else {
            self.0.open_owned(principal, identity).await
        }
    }
}

struct HttpAuthority {
    grant: Arc<ConsoleLiveGrant>,
    identity: String,
    principal: meerkat_core::PrincipalRef,
    access: AccessController,
}

#[async_trait]
impl AuthenticatedHttpLiveAuthority for HttpAuthority {
    fn principal(&self) -> &meerkat_core::PrincipalRef {
        &self.principal
    }

    async fn authorize(
        &self,
        operation: LiveOperation,
        session: Option<&SessionId>,
        params: &Value,
        _machine: &meerkat_runtime::MeerkatMachine,
    ) -> Result<(), crate::rpc::JsonRpcError> {
        if session != Some(&self.grant.session)
            || params.get("identity").and_then(Value::as_str) != Some(self.identity.as_str())
        {
            return Err(VoiceError::Unauthorized.rpc_error());
        }
        if !matches!(
            operation,
            LiveOperation::Close | LiveOperation::Status | LiveOperation::PlaybackOwnerRevoke
        ) {
            let access = self.access.view_for_subject(Some(&self.grant.principal));
            if self.grant.revoked.load(Ordering::SeqCst)
                || !access.allows_agent(ACTION_AGENT_VIEW, &self.identity)
                || !access.allows_agent(ACTION_AGENT_SEND, &self.identity)
            {
                return Err(VoiceError::Unauthorized.rpc_error());
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Deliveries {
    open: Option<LiveRpcResponseDeliveryCustody>,
    answers: HashMap<String, LiveRpcResponseDeliveryCustody>,
    received: HashSet<String>,
}

/// Activation polls observed for one pending channel.
struct PollTrace {
    count: u32,
    first: std::time::Instant,
}

struct Session {
    shared: Arc<SharedHost>,
    /// Provisional assistant captions of this call's channels.
    captions: VoiceCaptionRegistration,
    grant: Arc<ConsoleLiveGrant>,
    identity: String,
    current: StdMutex<PendingLiveChannelHandle>,
    channels: StdMutex<Vec<PendingLiveChannelHandle>>,
    deliveries: Mutex<Deliveries>,
    /// Status polls per channel until the channel is first seen active.
    polls: StdMutex<HashMap<String, PollTrace>>,
}

impl Session {
    /// Count activation status polls per channel and report how many the
    /// caller needed, and how long it waited, once the channel is active.
    fn trace_status_poll(&self, channel: &str, result: &Value) {
        let phase = result.get("phase").and_then(Value::as_str).unwrap_or("");
        let mut polls = self
            .polls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let trace = polls
            .entry(channel.to_string())
            .or_insert_with(|| PollTrace {
                count: 0,
                first: std::time::Instant::now(),
            });
        trace.count += 1;
        if phase == "active" || phase == "closed" || phase == "revoked" {
            let trace = polls.remove(channel).unwrap_or(PollTrace {
                count: 0,
                first: std::time::Instant::now(),
            });
            tracing::info!(
                target: "meerkat_meta::console_voice::timing",
                identity = %self.identity,
                channel_id = channel,
                phase,
                polls = trace.count,
                waited_ms = elapsed_ms(trace.first),
                "console voice status polls settled"
            );
        }
    }

    async fn call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Option<LiveRpcResponseDeliveryCustody>), VoiceError> {
        let channel_id = params
            .get("channel_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let surface = self
            .shared
            .guard(Arc::clone(&self.grant), self.identity.clone())?;
        let (response, delivery) =
            capture_live_rpc_response_delivery(self.shared.handler.dispatch(
                surface,
                Some(self.grant.session.clone()),
                Some(self.identity.clone()),
                method.to_string(),
                params,
                json!("console-voice-control"),
            ))
            .await;
        if response.error.is_some() {
            if let Some(delivery) = delivery {
                let _ = delivery.rejected().await;
            }
            tracing::warn!(
                method,
                identity = %self.identity,
                channel_id,
                error = ?response.error,
                "console live control refused by shared host"
            );
            return Err(VoiceError::HostFailed);
        }
        Ok((response.result.ok_or(VoiceError::HostFailed)?, delivery))
    }

    async fn retain_replacement(&self) -> Result<Option<Value>, VoiceError> {
        let Some(replacement) = self
            .shared
            .handler
            .pending_replacement(&self.grant.session)
            .await
        else {
            return Ok(None);
        };
        let reason = match &replacement {
            meerkat::surface::ExperimentalLiveReplacementRequired::CanonicalContext { .. } => {
                "canonical_context"
            }
            meerkat::surface::ExperimentalLiveReplacementRequired::DelegationResult { .. } => {
                "delegation_result"
            }
        };
        let pending = PendingLiveChannelHandle::new(
            self.identity.clone(),
            self.pending().execution_mode,
            replacement.pending_receipt(),
            replacement.open().clone(),
        );
        let (status, _) = self
            .call(
                "meta/live/status",
                json!({
                    "identity": self.identity,
                    "channel_id": pending.channel_id,
                    "pending_receipt": pending.pending_receipt,
                }),
            )
            .await?;
        let status: ExperimentalLiveChannelStatus =
            serde_json::from_value(status).map_err(|_| VoiceError::HostFailed)?;
        if matches!(
            status,
            ExperimentalLiveChannelStatus::Closed | ExperimentalLiveChannelStatus::Revoked
        ) {
            tracing::debug!(
                channel_id = %pending.channel_id,
                "ignoring terminal console voice replacement"
            );
            return Ok(None);
        }
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !channels
            .iter()
            .any(|channel| channel.channel_id == pending.channel_id)
        {
            channels.push(pending.clone());
        }
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pending.clone();
        Ok(Some(
            json!({"required":true, "reason":reason, "replacement":pending,
                "canonical_seed_cursor":replacement.canonical_seed_cursor()}),
        ))
    }
}

#[async_trait]
impl ConsoleVoiceSession for Session {
    fn pending(&self) -> PendingLiveChannelHandle {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn context_preparation(
        &self,
        channel: &str,
    ) -> Result<VoiceContextPreparation, VoiceError> {
        let pending = self.pending();
        if pending.channel_id != channel {
            return Err(VoiceError::RequestConflict);
        }
        if self.grant.revoked.load(Ordering::SeqCst) {
            return Err(VoiceError::Closed);
        }
        let access = self
            .shared
            .access
            .view_for_subject(Some(&self.grant.principal));
        if !access.allows_agent(ACTION_AGENT_VIEW, &self.identity) {
            return Err(VoiceError::Unauthorized);
        }
        // Durable-source availability is validated once, at open admission
        // (`prepare_open`). Re-validating on every context status poll loaded
        // the full persisted session body on the mob actor once a second per
        // preparing voice call and queued every other mob command behind it.
        // The custody read below is the authority for whether this channel is
        // still preparing, active, or closed.
        let read = async {
            let custody = self
                .shared
                .handler
                .read_console_channel_custody(
                    &self.grant.session,
                    &meerkat_core::LiveChannelId::new(channel),
                    &pending.pending_receipt,
                )
                .await
                .map_err(|_| VoiceError::ContextReadFailed)?;
            if matches!(
                custody.phase(),
                meerkat::surface::ExperimentalLiveChannelPhaseStatus::Closed
                    | meerkat::surface::ExperimentalLiveChannelPhaseStatus::Revoked
            ) {
                return Err(VoiceError::Closed);
            }
            if self.pending().channel_id != channel {
                return Err(VoiceError::RequestConflict);
            }
            Ok(VoiceContextPreparation::from(custody.context_preparation()))
        };
        tokio::time::timeout(Duration::from_secs(5), read)
            .await
            .map_err(|_| VoiceError::ContextReadFailed)?
    }

    async fn captions(
        &self,
        channel: &str,
        after: u64,
        wait: Duration,
    ) -> Result<VoiceCaptionBatch, VoiceError> {
        if self.pending().channel_id != channel {
            return Err(VoiceError::RequestConflict);
        }
        if self.grant.revoked.load(Ordering::SeqCst) {
            return Err(VoiceError::Closed);
        }
        let access = self
            .shared
            .access
            .view_for_subject(Some(&self.grant.principal));
        if !access.allows_agent(ACTION_AGENT_VIEW, &self.identity) {
            return Err(VoiceError::Unauthorized);
        }
        self.captions.wait_read(channel, after, wait).await
    }

    async fn dispatch(&self, method: &str, params: Value) -> Result<Value, VoiceError> {
        let channel = params
            .get("channel_id")
            .and_then(Value::as_str)
            .ok_or(VoiceError::InvalidRequest)?
            .to_string();
        if method == "meta/live/playback_owner/register" {
            let pending = self.pending();
            if params.get("pending_receipt").and_then(Value::as_str)
                != Some(pending.pending_receipt.as_str())
            {
                return Err(VoiceError::InvalidRequest);
            }
            if let Some(delivery) = self.deliveries.lock().await.open.take() {
                delivery
                    .delivered()
                    .await
                    .map_err(|_| VoiceError::HostFailed)?;
            }
        }
        // Only pending-receipt polls are activation polls; the active call's
        // periodic status checks carry the activation receipt instead.
        let activation_poll =
            method == "meta/live/status" && params.get("pending_receipt").is_some();
        let (result, delivery) = self.call(method, params).await?;
        if activation_poll {
            self.trace_status_poll(&channel, &result);
        }
        if let Some(delivery) = delivery {
            if method != "live/webrtc/answer" {
                let _ = delivery.rejected().await;
                return Err(VoiceError::HostFailed);
            }
            self.deliveries
                .lock()
                .await
                .answers
                .insert(channel, delivery);
        }
        Ok(result)
    }

    async fn answer_received(&self, channel: &str) -> Result<(), VoiceError> {
        let mut deliveries = self.deliveries.lock().await;
        if deliveries.received.contains(channel) {
            return Ok(());
        }
        let delivery = deliveries
            .answers
            .remove(channel)
            .ok_or(VoiceError::InvalidRequest)?;
        delivery
            .delivered()
            .await
            .map_err(|_| VoiceError::HostFailed)?;
        deliveries.received.insert(channel.to_string());
        Ok(())
    }

    async fn replacement_required(&self) -> Result<Value, VoiceError> {
        Ok(self
            .retain_replacement()
            .await?
            .unwrap_or_else(|| json!({"required":false})))
    }

    async fn close(&self) -> Result<(), VoiceError> {
        self.grant.revoked.store(true, Ordering::SeqCst);
        // A closing call shows no more captions; waiting reads answer now.
        self.captions.close();
        let mut deliveries = self.deliveries.lock().await;
        if let Some(delivery) = deliveries.open.take() {
            delivery
                .rejected()
                .await
                .map_err(|_| VoiceError::HostFailed)?;
        }
        for (_, delivery) in std::mem::take(&mut deliveries.answers) {
            delivery
                .rejected()
                .await
                .map_err(|_| VoiceError::HostFailed)?;
        }
        drop(deliveries);
        self.retain_replacement().await?;
        let channels = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        // Recovery may have superseded the caller's last active handle.
        // Close the newly discovered replacement before older fenced channels.
        for channel in channels.into_iter().rev() {
            let params = json!({"identity":self.identity, "channel_id":channel.channel_id,
                "pending_receipt":channel.pending_receipt});
            let (status, _) = self.call("meta/live/status", params.clone()).await?;
            let status: ExperimentalLiveChannelStatus =
                serde_json::from_value(status).map_err(|_| VoiceError::HostFailed)?;
            match status {
                // Receipt close also cancels recovery prepared after transport loss.
                ExperimentalLiveChannelStatus::Closed
                | ExperimentalLiveChannelStatus::Revoked
                | ExperimentalLiveChannelStatus::Pending => {
                    self.call("meta/live/close", params).await?;
                }
                ExperimentalLiveChannelStatus::Active { handle } => {
                    self.call("meta/live/close", json!({"identity":self.identity,
                        "channel_id":handle.channel_id,"activation_receipt":handle.activation_receipt})).await?;
                }
            }
        }
        Ok(())
    }
}
