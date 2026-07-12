//! Phase 6b live-pipeline battery (T-L3..T-L10, T-L13, T-L14).
//!
//! Exercises the extracted `LiveOrchestrator` open/verb pipeline and the
//! `ServiceMemberLiveHost` member-host adapter against a real
//! runtime-backed service + machine, with the deterministic
//! `ScriptedRealtimeSessionFactory` (ADJ-P6B-4) standing in for the
//! provider — no socket is ever opened; bootstrap URL truth, fail-closed
//! eviction, session pins, and cause mappings are all asserted against
//! generated machine authority.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(all(
    feature = "session-store",
    feature = "live",
    feature = "memory-store",
    feature = "test-realtime-fixtures",
    not(target_arch = "wasm32")
))]
mod live_pipeline {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::session_runtime::admission::StagedCapacityAdmissions;
    use meerkat::session_runtime::errors::{
        LiveChannelVerbError, LiveIngressError, LiveOpenError, LiveOpenPrecheckError,
    };
    use meerkat::session_runtime::live_orchestration::{
        LiveOrchestrator, LiveSessionIngressReconciler, LiveTransportContext,
    };
    use meerkat::session_runtime::runtime_state::ArchiveRuntimeCleanup;
    use meerkat::surface::{
        ServiceLiveProjection, ServiceLiveToolDispatcher, ServiceMemberLiveHost,
        ServiceMemberLiveHostConfig, build_runtime_backed_service, default_persistent_executor,
        materialize_session, member_live_error_from_open, member_live_error_from_verb,
    };
    use meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory;
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistenceBundle,
        PersistentSessionService, Session, StagedSessionRegistry,
    };
    use meerkat_client::TestClient;
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeLiveControlOutcome, BridgeLiveControlVerb,
    };
    use meerkat_contracts::{
        LiveCloseStatus, LiveCommitInputStatus, RealtimeTurningMode, WireLiveTransportBootstrap,
    };
    use meerkat_core::live_adapter::LiveAdapterCommand;
    use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
    use meerkat_core::types::SessionId;
    use meerkat_live::{LiveAdapterHost, LiveChannelId, LiveWsState};
    use meerkat_runtime::MeerkatMachine;
    use meerkat_runtime::member_live::{MemberLiveError, MemberLiveHost};
    use meerkat_store::MemoryBlobStore;

    const ADVERTISED_BASE: &str = "wss://advertised.example:7443";

    struct Fixture {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        host: Arc<LiveAdapterHost>,
        ws_state: Arc<LiveWsState>,
        _temp: tempfile::TempDir,
    }

    async fn build_fixture() -> Fixture {
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence = PersistenceBundle::new(
            session_store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(MemoryBlobStore::new()),
        );
        let temp = tempfile::tempdir().expect("tempdir");
        let factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
        let mut config = Config::default();
        config.realm.insert(
            "default".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[(
                "openai",
                "test-openai-key",
            )]),
        );
        let mut builder = FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(TestClient::default()));
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
        let service = Arc::new(service);

        let projection = Arc::new(ServiceLiveProjection::new(
            Arc::clone(&service),
            Arc::clone(&adapter),
        ));
        let projection_sink: Arc<dyn meerkat_live::LiveProjectionSink> = projection.clone();
        let close_feedback: Arc<dyn meerkat_live::LiveChannelCloseFeedback> = projection.clone();
        let status_feedback: Arc<dyn meerkat_live::LiveChannelStatusFeedback> = projection.clone();
        let token_authority: Arc<dyn meerkat_live::LiveWsTokenAuthority> = projection;
        let host = Arc::new(LiveAdapterHost::new(projection_sink));
        let ws_state = Arc::new(LiveWsState::new(
            Arc::clone(&host),
            close_feedback,
            status_feedback,
            token_authority,
        ));

        Fixture {
            service,
            adapter,
            staged_sessions: Arc::new(StagedSessionRegistry::new()),
            staged_capacity_admissions: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            host,
            ws_state,
            _temp: temp,
        }
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-realtime-2".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "live pipeline unit battery".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    async fn create_realtime_session(fx: &Fixture) -> SessionId {
        let session = Session::new();
        let session_id = session.id().clone();
        let service_for_executor = Arc::clone(&fx.service);
        let adapter_for_executor = Arc::clone(&fx.adapter);
        Box::pin(materialize_session(
            &fx.service,
            &fx.adapter,
            session,
            create_request(),
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize realtime session");
        session_id
    }

    /// Recording session-owned ingress hook (T-L7).
    #[derive(Default)]
    struct RecordingIngress {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LiveSessionIngressReconciler for RecordingIngress {
        async fn ensure_session_owned_live_ingress(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), LiveIngressError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn orchestrator<'a>(
        fx: &'a Fixture,
        ingress: Option<&'a dyn LiveSessionIngressReconciler>,
    ) -> LiveOrchestrator<'a> {
        LiveOrchestrator {
            service: &fx.service,
            staged_sessions: &fx.staged_sessions,
            staged_capacity_admissions: &fx.staged_capacity_admissions,
            runtime_adapter: &fx.adapter,
            host: Some(Arc::clone(&fx.host)),
            config_runtime: None,
            default_llm_client: None,
            agent_llm_client_decorator: None,
            external_tools: None,
            archive_runtime_cleanup: ArchiveRuntimeCleanup {
                runtime_adapter: Arc::clone(&fx.adapter),
                pending_session_event_streams: None,
                mcp_state: None,
                mob_state: None,
            },
            realm_id: None,
            instance_id: None,
            backend: None,
            ingress_reconciler: ingress,
        }
    }

    fn transport_ctx(fx: &Fixture) -> LiveTransportContext<'_> {
        LiveTransportContext {
            ws_state: Some(fx.ws_state.as_ref()),
            base_url: Some(ADVERTISED_BASE),
            #[cfg(feature = "live-webrtc")]
            webrtc: None,
        }
    }

    fn ws_url(result: &meerkat_contracts::LiveOpenResult) -> (String, String) {
        match &result.transport {
            WireLiveTransportBootstrap::Websocket { url, token } => (url.clone(), token.clone()),
            other => panic!("expected websocket bootstrap, got {other:?}"),
        }
    }

    /// T-L3: the happy-path open returns a bootstrap whose base is the
    /// CONTEXT base URL (never any listener addr), with the machine-minted
    /// token, the channel pin, and the typed audio format param; the
    /// pipeline owns the `turning_mode` default (DEC-P6B-L15).
    #[tokio::test]
    async fn open_mints_bootstrap_from_context_base_url() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();

        let result = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect("scripted open must succeed");

        let (url, token) = ws_url(&result);
        assert!(
            url.starts_with(ADVERTISED_BASE),
            "bootstrap base must be the context base URL, got {url}"
        );
        assert!(url.contains(&format!("channel={}", result.channel_id)));
        assert!(url.contains("&format=pcm_24k_mono"));
        assert!(
            !token.is_empty(),
            "machine-minted token must ride the bootstrap"
        );

        let opens = factory.opens();
        assert_eq!(opens.len(), 1);
        assert_eq!(
            opens[0].turning_mode,
            RealtimeTurningMode::ProviderManaged,
            "absent turning_mode must default to ProviderManaged in the pipeline"
        );
    }

    /// T-L6: #302 fires BEFORE any admission — no channel is minted.
    #[tokio::test]
    async fn missing_factory_fails_before_any_admission() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;

        let error = orchestrator(&fx, None)
            .open_live_channel(&fx.host, transport_ctx(&fx), None, &session_id, None, None)
            .await
            .expect_err("open without a factory must fail closed");
        assert!(matches!(error, LiveOpenError::RealtimeFactoryMissing));
        assert!(
            fx.adapter
                .live_active_channel_for_session(&session_id)
                .await
                .is_none(),
            "no channel may be minted before the factory gate"
        );
    }

    /// T-L4: S8/S9/S11 failure arms leave NO machine binding behind (#355
    /// eviction) — a post-failure open re-admits cleanly.
    #[tokio::test]
    async fn every_failure_arm_evicts_the_binding_and_readmits() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let ingress = RecordingIngress::default();

        // S8 — factory supports no provider.
        let unsupported = ScriptedRealtimeSessionFactory::supporting_no_provider();
        let error = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&unsupported),
                &session_id,
                None,
                None,
            )
            .await
            .expect_err("unsupported provider must fail closed");
        assert!(matches!(
            error,
            LiveOpenError::ProviderUnsupportedByFactory { .. }
        ));
        assert!(
            fx.adapter
                .live_active_channel_for_session(&session_id)
                .await
                .is_none(),
            "S8 failure must evict the admission"
        );

        // S9 — provider adapter open fails.
        let failing = ScriptedRealtimeSessionFactory::new();
        failing.fail_opens();
        let error = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&failing),
                &session_id,
                None,
                None,
            )
            .await
            .expect_err("adapter open failure must fail closed");
        assert!(matches!(error, LiveOpenError::AdapterOpen(_)));
        assert!(
            fx.adapter
                .live_active_channel_for_session(&session_id)
                .await
                .is_none(),
            "S9 failure must evict the admission"
        );

        // S11 — websocket requested but not composed.
        let good = ScriptedRealtimeSessionFactory::new();
        let bare_ctx = LiveTransportContext {
            ws_state: None,
            base_url: None,
            #[cfg(feature = "live-webrtc")]
            webrtc: None,
        };
        let error = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                bare_ctx,
                Some(&good),
                &session_id,
                None,
                Some(meerkat_contracts::LiveOpenTransport::Websocket),
            )
            .await
            .expect_err("websocket without WS state must fail closed");
        assert!(matches!(error, LiveOpenError::WebsocketNotConfigured));
        assert!(
            fx.adapter
                .live_active_channel_for_session(&session_id)
                .await
                .is_none(),
            "S11 failure must evict the admission"
        );

        // The #355 proof: a fresh open over the same session re-admits.
        orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&good),
                &session_id,
                None,
                None,
            )
            .await
            .expect("post-failure open must re-admit cleanly");
    }

    /// T-L5: a second open on the same session is the typed AlreadyBound.
    #[tokio::test]
    async fn duplicate_open_rejects_already_bound() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();

        orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect("first open succeeds");
        let error = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect_err("duplicate open must reject");
        assert!(matches!(
            error,
            LiveOpenError::AdmissionRejectedAlreadyBound { .. }
        ));
    }

    /// T-L7 (session-owned half): the surface hook fires exactly once per
    /// successful open. (The mob-owned skip is pinned end-to-end by the
    /// cross-host ingress-skip row — a mob-owned drain cannot be arranged
    /// without the mob provisioning machinery.)
    #[cfg(feature = "comms")]
    #[tokio::test]
    async fn session_owned_open_calls_the_ingress_hook_once() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();

        orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect("open succeeds");
        assert_eq!(
            ingress.calls.load(Ordering::SeqCst),
            1,
            "session-owned reconciliation must run exactly once per open"
        );
    }

    /// T-L8: verbs on an unbound channel record through the generated
    /// unbound-request authority and error typed.
    #[tokio::test]
    async fn unbound_channel_records_via_machine_authority_then_errors_typed() {
        let fx = build_fixture().await;
        let unknown = LiveChannelId::new("chan-unknown");

        let error = orchestrator(&fx, None)
            .close_live_channel(&fx.host, &unknown, None)
            .await
            .expect_err("close of an unknown channel must error typed");
        match error {
            LiveChannelVerbError::UnboundRequest { channel_id, .. } => {
                assert_eq!(channel_id, "chan-unknown");
            }
            other => panic!("expected UnboundRequest, got {other:?}"),
        }

        let error = orchestrator(&fx, None)
            .live_channel_status(&fx.host, &unknown, None)
            .await
            .expect_err("status of an unknown channel must error typed");
        assert!(matches!(error, LiveChannelVerbError::UnboundRequest { .. }));
    }

    /// T-L9 (DEC-P6B-L6): a session-pin mismatch fails BEFORE the close
    /// reserve — host and machine state stay untouched, and the correctly
    /// pinned close still succeeds afterwards.
    #[tokio::test]
    async fn session_pin_mismatch_fails_before_any_side_effect() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();

        let opened = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect("open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);

        let other_session = SessionId::new();
        let error = orchestrator(&fx, None)
            .close_live_channel(&fx.host, &channel, Some(&other_session))
            .await
            .expect_err("pin mismatch must fail closed");
        assert!(matches!(
            error,
            LiveChannelVerbError::SessionPinMismatch { .. }
        ));
        assert_eq!(
            fx.adapter
                .live_active_channel_for_session(&session_id)
                .await
                .map(|c| c.to_string()),
            Some(opened.channel_id.clone()),
            "a pin-mismatched close must not touch the binding"
        );

        orchestrator(&fx, None)
            .close_live_channel(&fx.host, &channel, Some(&session_id))
            .await
            .expect("correctly pinned close succeeds");
    }

    /// T-L10: each control verb maps to its adapter command / refresh path
    /// and yields the typed per-verb outcome.
    #[tokio::test]
    async fn control_verbs_map_to_adapter_commands() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();

        let opened = orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&factory),
                &session_id,
                None,
                None,
            )
            .await
            .expect("open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        let outcome = orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::CommitInput,
            )
            .await
            .expect("commit_input verb");
        assert_eq!(
            outcome,
            BridgeLiveControlOutcome::CommitInput {
                status: LiveCommitInputStatus::Committed,
            }
        );

        orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::Interrupt,
            )
            .await
            .expect("interrupt verb");
        orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::Truncate {
                    item_id: "item-1".to_string(),
                    content_index: 0,
                    audio_played_ms: 10,
                },
            )
            .await
            .expect("truncate verb");
        let outcome = orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::Refresh,
            )
            .await
            .expect("refresh verb");
        assert!(matches!(outcome, BridgeLiveControlOutcome::Refresh { .. }));

        let commands = adapter.commands();
        assert!(
            commands.iter().any(|c| matches!(
                c,
                LiveAdapterCommand::CommitInput {
                    response_modality: None
                }
            )),
            "bridge CommitInput must map to the modality-less adapter command: {commands:?}"
        );
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, LiveAdapterCommand::Interrupt)),
            "bridge Interrupt must map to the adapter Interrupt: {commands:?}"
        );
        assert!(
            commands.iter().any(|c| matches!(
                c,
                LiveAdapterCommand::TruncateAssistantOutput {
                    item_id,
                    content_index: 0,
                    audio_played_ms: 10,
                } if item_id == "item-1"
            )),
            "bridge Truncate must map to TruncateAssistantOutput: {commands:?}"
        );
    }

    /// T-L13: error-mapping spot checks (the exhaustive matches ARE the
    /// compile pin — no wildcard arms exist in either projection).
    #[test]
    fn member_error_mapping_spot_checks() {
        assert!(matches!(
            member_live_error_from_open(LiveOpenError::RealtimeFactoryMissing),
            MemberLiveError::TransportUnavailable
        ));
        assert!(matches!(
            member_live_error_from_open(LiveOpenError::AdmissionRejectedAlreadyBound {
                session_id: SessionId::new(),
            }),
            MemberLiveError::ChannelAlreadyBound
        ));
        match member_live_error_from_open(LiveOpenError::Precheck(
            LiveOpenPrecheckError::ModelNotRealtime {
                model: "gpt-5.4".to_string(),
                provider: "openai",
            },
        )) {
            MemberLiveError::ModelNotRealtime { model, provider } => {
                assert_eq!(model, "gpt-5.4");
                assert_eq!(provider, "openai");
            }
            other => panic!("expected ModelNotRealtime, got {other:?}"),
        }
        match member_live_error_from_open(LiveOpenError::ProviderUnsupportedByFactory {
            provider: "openai",
        }) {
            MemberLiveError::AdapterUnavailable { provider } => assert_eq!(provider, "openai"),
            other => panic!("expected AdapterUnavailable, got {other:?}"),
        }
        assert!(matches!(
            member_live_error_from_open(LiveOpenError::WebrtcNotCompiled),
            MemberLiveError::TransportUnsupported { .. }
        ));
        assert!(matches!(
            member_live_error_from_verb(LiveChannelVerbError::SessionPinMismatch {
                channel_id: "chan-1".to_string(),
            }),
            MemberLiveError::ChannelNotFound
        ));
        assert!(matches!(
            member_live_error_from_verb(LiveChannelVerbError::CommitOmitted),
            MemberLiveError::Internal { .. }
        ));
    }

    /// T-L14 (+ the member-side reconciliation primitives, T-L24 unit
    /// level): `ServiceMemberLiveHost` opens on a service-resident session
    /// with the ADVERTISED base; an absent session degrades `Unavailable`;
    /// `status(None)` discovers the active channel; close-by-id clears it
    /// (and re-close reports the typed not-found); a fresh open succeeds.
    #[tokio::test]
    async fn service_member_live_host_round_trip_and_reconciliation_primitives() {
        let fx = build_fixture().await;
        let session_id = create_realtime_session(&fx).await;
        // Production shape: member sessions are mob-owned by construction,
        // and the S10 ingress stage (comms builds) skips on that fact — a
        // session-owned session is unreachable through this host, so the
        // fixture records the mob-owned ingress the way materialization
        // does.
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("live-pipeline-unit-member")
                    .expect("inproc comms runtime"),
            );
            fx.adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from("mob-live-pipeline-unit"),
                )
                .await
                .expect("record mob-owned ingress for the member session");
        }
        let factory: Arc<ScriptedRealtimeSessionFactory> =
            Arc::new(ScriptedRealtimeSessionFactory::new());
        let member_host = ServiceMemberLiveHost::new(ServiceMemberLiveHostConfig {
            service: Arc::clone(&fx.service),
            runtime_adapter: Arc::clone(&fx.adapter),
            host: Arc::clone(&fx.host),
            ws_state: Some(Arc::clone(&fx.ws_state)),
            base_url: Some(ADVERTISED_BASE.to_string()),
            session_factory: Arc::clone(&factory)
                as Arc<dyn meerkat_llm_core::realtime_session::RealtimeSessionFactory>,
            realm_id: None,
            instance_id: None,
            backend: None,
        });

        // Probe before any open: an honest "nothing to reconcile".
        assert!(matches!(
            member_host.status(&session_id, None).await,
            Err(MemberLiveError::ChannelNotFound)
        ));

        // Absent session degrades typed.
        let missing = SessionId::new();
        assert!(matches!(
            member_host.open(&missing, None, None).await,
            Err(MemberLiveError::Unavailable { .. })
        ));

        // Open: bootstrap base = the advertised URL by construction.
        let opened = member_host
            .open(&session_id, None, None)
            .await
            .expect("member open succeeds");
        let (url, _token) = ws_url(&opened);
        assert!(url.starts_with(ADVERTISED_BASE));

        // Duplicate open: the member cause parity for AlreadyBound.
        assert!(matches!(
            member_host.open(&session_id, None, None).await,
            Err(MemberLiveError::ChannelAlreadyBound)
        ));

        // status(None) discovers the orphan's channel id (ADJ-P6B-2).
        let report = member_host
            .status(&session_id, None)
            .await
            .expect("status probe succeeds");
        assert_eq!(report.channel_id, opened.channel_id);

        // Close-by-id clears; a re-close is the typed not-found
        // (idempotent-safe for the caller-driven reconciliation loop).
        let status = member_host
            .close(&session_id, &opened.channel_id)
            .await
            .expect("close succeeds");
        assert_eq!(status, LiveCloseStatus::Closed);
        assert!(matches!(
            member_host.close(&session_id, &opened.channel_id).await,
            Err(MemberLiveError::ChannelNotFound)
        ));

        // A fresh open succeeds — the reconciliation loop's terminal.
        member_host
            .open(&session_id, None, None)
            .await
            .expect("reopen after close succeeds");
    }

    /// T-L14 (tool seam): `ServiceLiveToolDispatcher` routes through the
    /// canonical service dispatch seam and fails typed for an unknown
    /// session — never a silent skip.
    #[tokio::test]
    async fn service_live_tool_dispatcher_fails_typed_for_unknown_session() {
        let fx = build_fixture().await;
        let dispatcher = ServiceLiveToolDispatcher::new(Arc::clone(&fx.service));
        let call = meerkat_core::ToolCall {
            id: "call-1".to_string(),
            name: "nonexistent_tool".to_string(),
            args: serde_json::json!({}),
        };
        let result = meerkat_live::LiveToolDispatcher::dispatch_live_tool_call(
            &dispatcher,
            &SessionId::new(),
            call,
        )
        .await;
        assert!(
            result.is_err(),
            "dispatch against an unknown session must fail typed"
        );
    }
}
