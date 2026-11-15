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
        LiveOrchestrator, LiveSessionAddressing, LiveSessionIngressReconciler,
        LiveTransportContext, RealtimeSessionOpenProjectionError,
    };
    use meerkat::session_runtime::runtime_state::ArchiveRuntimeCleanup;
    use meerkat::surface::{
        ServiceLiveProjection, ServiceLiveToolDispatcher, ServiceMemberLiveHost,
        ServiceMemberLiveHostConfig, build_runtime_backed_service, default_persistent_executor,
        materialize_session, member_live_error_from_open, member_live_error_from_verb,
    };
    use meerkat::test_fixtures::realtime::ScriptedRealtimeSessionFactory;
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgent, FactoryAgentBuilder,
        PersistenceBundle, PersistentSessionService, Session, SessionAgentBuilder,
        StagedSessionRegistry,
    };
    use meerkat_client::TestClient;
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeLiveControlOutcome, BridgeLiveControlVerb,
    };
    use meerkat_contracts::{
        LiveCloseStatus, LiveCommitInputStatus, RealtimeTurningMode, WireLiveTransportBootstrap,
    };
    use meerkat_core::AgentEvent;
    use meerkat_core::live_adapter::LiveAdapterCommand;
    use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions};
    use meerkat_core::types::SessionId;
    use meerkat_live::{LiveAdapterHost, LiveChannelId, LiveWsState};
    use meerkat_runtime::MeerkatMachine;
    use meerkat_runtime::member_live::{MemberLiveError, MemberLiveHost};
    use meerkat_store::MemoryBlobStore;

    const ADVERTISED_BASE: &str = "wss://advertised.example:7443";
    /// The catalog's realtime model.
    const REALTIME_MODEL: &str = "gpt-realtime-2";
    /// A text model the catalog does not flag as realtime-capable.
    const TEXT_MODEL: &str = "gpt-5.5";

    struct NonFactorySessionAgentBuilder;

    #[async_trait::async_trait]
    impl SessionAgentBuilder for NonFactorySessionAgentBuilder {
        type Agent = FactoryAgent;

        fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
            meerkat_core::SessionCommsIdentityRoot::NotKept
        }

        async fn build_agent(
            &self,
            _req: &CreateSessionRequest,
            _event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
        ) -> Result<Self::Agent, meerkat::SessionError> {
            Err(meerkat::SessionError::Unsupported(
                "type-level live orchestration fixture does not build agents".to_string(),
            ))
        }
    }

    #[test]
    fn shared_live_pipeline_accepts_non_factory_session_agent_builders() {
        fn assert_member_live_host<T: MemberLiveHost>() {}
        fn assert_live_projection<T>()
        where
            T: meerkat_live::LiveProjectionSink
                + meerkat_live::LiveChannelCloseFeedback
                + meerkat_live::LiveChannelStatusFeedback
                + meerkat_live::LiveWsTokenAuthority,
        {
        }
        fn assert_send_sync<T: Send + Sync>() {}
        fn accepts_orchestrator(
            _orchestrator: Option<LiveOrchestrator<'static, NonFactorySessionAgentBuilder>>,
        ) {
        }
        fn builds_projection(
            service: Arc<PersistentSessionService<NonFactorySessionAgentBuilder>>,
            machine: Arc<MeerkatMachine>,
        ) -> ServiceLiveProjection<NonFactorySessionAgentBuilder> {
            ServiceLiveProjection::new(service, machine)
        }

        assert_member_live_host::<ServiceMemberLiveHost<NonFactorySessionAgentBuilder>>();
        assert_live_projection::<ServiceLiveProjection<NonFactorySessionAgentBuilder>>();
        assert_send_sync::<ServiceMemberLiveHostConfig<NonFactorySessionAgentBuilder>>();
        assert_send_sync::<ServiceLiveProjection<NonFactorySessionAgentBuilder>>();
        accepts_orchestrator(None);
        let _ = builds_projection;
    }

    struct Fixture {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        staged_sessions: Arc<StagedSessionRegistry>,
        staged_capacity_admissions: StagedCapacityAdmissions,
        host: Arc<LiveAdapterHost>,
        ws_state: Arc<LiveWsState>,
        actor_witness_slots: Arc<std::sync::Mutex<Vec<meerkat::LiveSessionActorWitnessSlot>>>,
        _temp: tempfile::TempDir,
        // Last, so it is released after everything else the test built.
        _open_reservation: tokio::sync::MutexGuard<'static, ()>,
    }

    // The production one-open memory reservation is process wide; nextest
    // isolates tests by process, plain `cargo test` does not. Every test that
    // builds a fixture holds this for its whole run, so one test's opens (and
    // the test that occupies the reservation on purpose) never meet another's.
    static OPEN_RESERVATION: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn build_fixture() -> Fixture {
        build_fixture_with_tools(None).await
    }

    /// The fixture, with `tools` as every session's tool dispatcher when
    /// given. The factory composes its catalog control plane over them.
    async fn build_fixture_with_tools(
        tools: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    ) -> Fixture {
        let open_reservation = OPEN_RESERVATION.lock().await;
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(meerkat::MemoryStore::new());
        let persistence = PersistenceBundle::new(
            session_store,
            Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
            Arc::new(MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
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
        builder.default_tool_dispatcher = tools;
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
            .expect("compose the runtime-backed service");
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
            actor_witness_slots: Arc::new(std::sync::Mutex::new(Vec::new())),
            _temp: temp,
            _open_reservation: open_reservation,
        }
    }

    fn create_request_for_model(model: &str) -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: model.to_string(),
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
        create_session_with_model(fx, REALTIME_MODEL).await
    }

    async fn create_session_with_model(fx: &Fixture, model: &str) -> SessionId {
        let session = Session::new();
        let session_id = session.id().clone();
        let service_for_executor = Arc::clone(&fx.service);
        let adapter_for_executor = Arc::clone(&fx.adapter);
        Box::pin(materialize_session(
            &fx.service,
            &fx.adapter,
            session,
            create_request_for_model(model),
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
        let actor_witness_slots = Arc::clone(&fx.actor_witness_slots);
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
            actor_witness_capture: Arc::new(move |_session_id, actor_witness_slot| {
                actor_witness_slots
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(actor_witness_slot);
            }),
            session_addressing: LiveSessionAddressing::SessionScoped,
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

    /// Open through the composed-surface projection seam with the realtime
    /// model selected for this channel over the member's text model, as a
    /// composed surface's per-open model selection does.
    async fn open_with_realtime_model(
        fx: &Fixture,
        ingress: &RecordingIngress,
        factory: &ScriptedRealtimeSessionFactory,
        session_id: &SessionId,
    ) -> meerkat_contracts::LiveOpenResult {
        let mut projection = orchestrator(fx, Some(ingress))
            .live_open_projection_for_session(
                session_id,
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await
            .expect("prepare projection");
        projection.open_config.llm_identity.model = REALTIME_MODEL.to_string();
        orchestrator(fx, Some(ingress))
            .open_live_channel_from_projection(
                &fx.host,
                transport_ctx(fx),
                factory,
                session_id,
                projection,
                None,
            )
            .await
            .expect("open under the channel's realtime model")
    }

    /// Every `Refresh` snapshot the adapter received, in order.
    fn refresh_snapshots(
        adapter: &meerkat::test_fixtures::realtime::ScriptedLiveAdapter,
    ) -> Vec<meerkat_core::live_adapter::LiveProjectionSnapshot> {
        adapter
            .commands()
            .into_iter()
            .filter_map(|command| match command {
                LiveAdapterCommand::Refresh { snapshot } => Some(snapshot),
                _ => None,
            })
            .collect()
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

    /// The compatibility projection seam is the same S5-S12 pipeline as the
    /// canonical seed entry point: equal failures clean up equal machine state,
    /// and both paths can subsequently re-admit without a shadow binding.
    #[tokio::test]
    async fn prepared_projection_and_seed_entry_share_failure_cleanup() {
        let fx = build_fixture().await;
        let seeded_session = create_realtime_session(&fx).await;
        let projected_session = create_realtime_session(&fx).await;
        let ingress = RecordingIngress::default();
        let unsupported = ScriptedRealtimeSessionFactory::supporting_no_provider();

        let seeded_error = orchestrator(&fx, Some(&ingress))
            .open_live_channel_with_seed(
                &fx.host,
                transport_ctx(&fx),
                Some(&unsupported),
                &seeded_session,
                None,
                None,
                None,
            )
            .await
            .expect_err("seed entry must reject the unsupported provider");

        let projection = orchestrator(&fx, Some(&ingress))
            .live_open_projection_for_session(
                &projected_session,
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await
            .expect("prepare projection");
        let projected_error = orchestrator(&fx, Some(&ingress))
            .open_live_channel_from_projection(
                &fx.host,
                transport_ctx(&fx),
                &unsupported,
                &projected_session,
                projection,
                None,
            )
            .await
            .expect_err("prepared projection must reject the unsupported provider");

        assert!(matches!(
            seeded_error,
            LiveOpenError::ProviderUnsupportedByFactory { .. }
        ));
        assert!(matches!(
            projected_error,
            LiveOpenError::ProviderUnsupportedByFactory { .. }
        ));
        for session_id in [&seeded_session, &projected_session] {
            assert!(
                fx.adapter
                    .live_active_channel_for_session(session_id)
                    .await
                    .is_none(),
                "both entry points must evict the failed S5 admission"
            );
        }

        let good = ScriptedRealtimeSessionFactory::new();
        orchestrator(&fx, Some(&ingress))
            .open_live_channel(
                &fx.host,
                transport_ctx(&fx),
                Some(&good),
                &seeded_session,
                None,
                None,
            )
            .await
            .expect("seed path must re-admit after cleanup");
        let projection = orchestrator(&fx, Some(&ingress))
            .live_open_projection_for_session(
                &projected_session,
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await
            .expect("prepare second projection");
        orchestrator(&fx, Some(&ingress))
            .open_live_channel_from_projection(
                &fx.host,
                transport_ctx(&fx),
                &good,
                &projected_session,
                projection,
                None,
            )
            .await
            .expect("projection path must re-admit after cleanup");
    }

    /// A prepared projection is sealed to its canonical session before S5.
    #[tokio::test]
    async fn prepared_projection_rejects_cross_session_use_before_admission() {
        let fx = build_fixture().await;
        let projection_owner = create_realtime_session(&fx).await;
        let requested_session = create_realtime_session(&fx).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();
        let projection = orchestrator(&fx, Some(&ingress))
            .live_open_projection_for_session(
                &projection_owner,
                RealtimeTurningMode::ProviderManaged,
                None,
            )
            .await
            .expect("prepare projection");

        let error = orchestrator(&fx, Some(&ingress))
            .open_live_channel_from_projection(
                &fx.host,
                transport_ctx(&fx),
                &factory,
                &requested_session,
                projection,
                None,
            )
            .await
            .expect_err("a projection cannot be replayed against another session");
        assert!(matches!(
            error,
            LiveOpenError::OpenConfig(
                meerkat::session_runtime::live_orchestration::RealtimeSessionOpenProjectionError::SessionMismatch { .. }
            )
        ));
        assert!(
            fx.adapter
                .live_active_channel_for_session(&requested_session)
                .await
                .is_none(),
            "projection mismatch must fail before generated admission"
        );
        assert!(factory.opens().is_empty());
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
                    ..
                } if item_id == "item-1"
            )),
            "bridge Truncate must map to TruncateAssistantOutput: {commands:?}"
        );
    }

    /// One channel whose realtime model differs from its member's text
    /// model, on a session with durable history and an image whose bytes
    /// were never hydrated. The public `live/refresh` verb and the member
    /// `refresh` control are both queued while the process-wide open
    /// reservation is occupied, and both payloads are history-free
    /// configuration updates under the channel's bound identity, with the
    /// durable currentness sidecars, no audio config and increasing snapshot
    /// versions. The channel's adapter is the scripted one, which accepts
    /// every command, so this is native host and payload acceptance, not
    /// provider acceptance. Once the channel is retired its refresh is
    /// refused locally, with no fallback to the text identity, and a sibling
    /// channel opened later still refreshes.
    ///
    /// The open reservation is process wide; nextest isolates tests by
    /// process.
    #[tokio::test]
    async fn refresh_projects_the_bound_identity_from_the_history_free_export() {
        let fx = build_fixture().await;
        let session_id = create_session_with_model(&fx, TEXT_MODEL).await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();
        let opened = open_with_realtime_model(&fx, &ingress, &factory, &session_id).await;
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        fx.service
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Text(
                    "durable history committed before the refresh".to_string(),
                ),
            )
            .await
            .expect("append durable history");
        fx.service
            .append_external_user_content(
                &session_id,
                meerkat_core::types::ContentInput::Blocks(vec![
                    meerkat_core::types::ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: meerkat_core::types::ImageData::Blob {
                            blob_id: meerkat_core::blob::content_blob_id(
                                "image/png",
                                "never-hydrated",
                            ),
                        },
                    },
                ]),
            )
            .await
            .expect("append an image whose bytes were never stored");
        let durable = fx
            .service
            .export_realtime_refresh_session_snapshot(&session_id)
            .await
            .expect("durable session");
        let text_identity = fx
            .service
            .live_session_llm_identity(&session_id)
            .await
            .expect("the member's text identity");
        assert_eq!(text_identity.model, TEXT_MODEL);
        let binding = fx
            .adapter
            .live_channel_binding(&session_id, &channel)
            .await
            .expect("binding read")
            .expect("the channel is bound");
        assert_eq!(binding.identity().model, REALTIME_MODEL);
        assert!(
            binding.follows_session_identity(),
            "no execution profile was admitted for this channel"
        );

        let held = meerkat_core::RealtimeOpenProjectionAdmission::global()
            .try_acquire()
            .expect("the open reservation is free before the refreshes");
        let public = orchestrator(&fx, None)
            .refresh_live_channel(&fx.host, &channel, None)
            .await
            .expect("public refresh while the open reservation is occupied");
        let member = orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::Refresh,
            )
            .await
            .expect("member refresh while the open reservation is occupied");
        drop(held);
        assert_eq!(public.status, meerkat_contracts::LiveRefreshStatus::Queued);
        assert_eq!(
            member,
            BridgeLiveControlOutcome::Refresh {
                status: meerkat_contracts::LiveRefreshStatus::Queued,
            }
        );

        let refreshes = refresh_snapshots(&adapter);
        assert_eq!(
            refreshes.len(),
            2,
            "both refreshes reach the scripted adapter"
        );
        let expected =
            meerkat_client::realtime_session::RealtimeSessionOpenConfig::for_refresh_from_messages(
                RealtimeTurningMode::ProviderManaged,
                text_identity.clone(),
                Vec::new(),
                durable.messages(),
            )
            .expect("expected refresh projection");
        let rewrite_generation = durable
            .transcript_rewrite_generation()
            .expect("rewrite generation");
        for snapshot in &refreshes {
            assert_eq!(snapshot.session_id, session_id);
            assert!(
                snapshot.seed_messages.is_empty(),
                "a refresh replays no history: {:?}",
                snapshot.seed_messages
            );
            assert_eq!(
                snapshot.model_id, REALTIME_MODEL,
                "the channel keeps its bound realtime model, not the text model"
            );
            assert_eq!(snapshot.provider_id, binding.identity().provider);
            assert_eq!(
                snapshot.canonical_system_messages,
                expected.canonical_system_messages_ref(),
                "the canonical System drift data is the durable session's"
            );
            assert_eq!(
                format!("{:?}", snapshot.user_content_identities),
                format!("{:?}", durable.realtime_user_content_identities()),
            );
            assert_eq!(
                format!("{:?}", snapshot.user_content_tombstones),
                format!("{:?}", durable.realtime_user_content_tombstones()),
            );
            assert_eq!(snapshot.transcript_rewrite_generation, rewrite_generation);
            assert!(snapshot.audio_config.is_none());
        }
        assert!(
            refreshes[1].snapshot_version > refreshes[0].snapshot_version,
            "each refresh stamps a newer snapshot version"
        );

        // Retirement: the refusal is local and never falls back to the text
        // identity.
        orchestrator(&fx, None)
            .close_live_channel(&fx.host, &channel, Some(&session_id))
            .await
            .expect("retire the channel");
        let public_retired = orchestrator(&fx, None)
            .refresh_live_channel(&fx.host, &channel, None)
            .await
            .expect_err("a retired channel is not refreshed");
        assert!(
            matches!(public_retired, LiveChannelVerbError::UnboundRequest { .. }),
            "{public_retired:?}"
        );
        let member_retired = orchestrator(&fx, None)
            .control_live_channel(
                &fx.host,
                transport_ctx(&fx),
                &channel,
                Some(&session_id),
                BridgeLiveControlVerb::Refresh,
            )
            .await
            .expect_err("a retired channel is not refreshed");
        assert!(
            matches!(member_retired, LiveChannelVerbError::UnboundRequest { .. }),
            "{member_retired:?}"
        );
        assert_eq!(
            refresh_snapshots(&adapter).len(),
            2,
            "no refresh reaches the retired adapter"
        );

        // A sibling opened later refreshes under its own bound identity.
        let sibling_session = create_session_with_model(&fx, TEXT_MODEL).await;
        fx.service
            .append_external_user_content(
                &sibling_session,
                meerkat_core::types::ContentInput::Text("sibling history".to_string()),
            )
            .await
            .expect("append sibling history");
        let sibling_factory = ScriptedRealtimeSessionFactory::new();
        let sibling =
            open_with_realtime_model(&fx, &ingress, &sibling_factory, &sibling_session).await;
        orchestrator(&fx, None)
            .refresh_live_channel(&fx.host, &LiveChannelId::new(&sibling.channel_id), None)
            .await
            .expect("a later sibling refreshes");
        let sibling_refreshes = refresh_snapshots(
            &sibling_factory
                .adapters()
                .pop()
                .expect("sibling scripted adapter"),
        );
        assert_eq!(sibling_refreshes.len(), 1);
        assert_eq!(sibling_refreshes[0].model_id, REALTIME_MODEL);
        assert!(sibling_refreshes[0].seed_messages.is_empty());
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
        // Another open on the host still holds the open-projection admission:
        // transient, so the member degrades typed instead of failing internal.
        assert!(matches!(
            member_live_error_from_open(LiveOpenError::OpenConfig(
                RealtimeSessionOpenProjectionError::Backpressured
            )),
            MemberLiveError::Unavailable { .. }
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
        assert!(matches!(
            member_live_error_from_verb(LiveChannelVerbError::CloseSettlementBusy {
                channel_id: "ch".to_string(),
                session_id: "sess".to_string(),
            }),
            MemberLiveError::Unavailable { .. }
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

        // Stock-realtime truncation enters the same canonical orchestrator
        // through an owner facade. The downstream surface supplies no
        // interaction identity and constructs no adapter command itself.
        let adapter = factory.adapters().pop().expect("scripted adapter minted");
        member_host
            .truncate_stock_live_output(
                &LiveChannelId::new(&opened.channel_id),
                "legacy-provider-item",
                2,
                125,
                Some("legacy played".to_string()),
            )
            .await
            .expect("stock truncate succeeds through owner facade");
        let commands = adapter.commands();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                LiveAdapterCommand::TruncateAssistantOutput {
                    item_id,
                    content_index: 2,
                    audio_played_ms: 125,
                    reported_playback_prefix: Some(prefix),
                    ..
                } if item_id == "legacy-provider-item" && prefix == "legacy played"
            )),
            "stock truncate must reach the canonical adapter command: {commands:?}"
        );

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

    // -- Deferred tools loaded inside a live conversation --

    const DEFERRED_LOOKUP: &str = "deferred_lookup";
    const DEFERRED_OTHER: &str = "deferred_other";
    const DEFERRED_UNSENDABLE: &str = "deferred_unsendable";

    /// Deferred-eligible tools behind an exact catalog whose definitions are
    /// over the deferred catalog's byte budget: enough for a session to run
    /// in the deferred mode, where the model sees the catalog control tools
    /// and loads the rest on demand. (Under the budget the tools would stay
    /// in every request and there would be nothing to load.)
    struct DeferredCatalogTools {
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
        catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
    }

    impl DeferredCatalogTools {
        fn new() -> Self {
            let object = serde_json::json!({ "type": "object" });
            Self::with_tools(&[(DEFERRED_LOOKUP, object.clone()), (DEFERRED_OTHER, object)])
        }

        /// The two ordinary tools and one whose parameter schema root is not
        /// an object: a realtime provider refuses such a definition.
        fn with_an_unsendable_tool() -> Self {
            let object = serde_json::json!({ "type": "object" });
            Self::with_tools(&[
                (DEFERRED_LOOKUP, object.clone()),
                (DEFERRED_OTHER, object),
                (DEFERRED_UNSENDABLE, serde_json::json!({ "type": "string" })),
            ])
        }

        /// A description of half the deferred catalog's byte budget, so any
        /// two of these tools are over it together.
        fn description(name: &str) -> String {
            let mut text = format!("Deferred test tool {name}.");
            while text.len()
                < meerkat_core::tool_catalog::DEFERRED_CATALOG_SCHEMA_VOLUME_THRESHOLD / 2
            {
                text.push_str(" It returns the text it ran with and reads nothing else.");
            }
            text
        }

        fn with_tools(specs: &[(&str, serde_json::Value)]) -> Self {
            let provenance = meerkat_core::ToolProvenance {
                kind: meerkat_core::ToolSourceKind::Callback,
                source_id: "deferred-catalog".into(),
                artifact: None,
            };
            let tools = specs
                .iter()
                .map(|(name, input_schema)| {
                    Arc::new(meerkat_core::ToolDef {
                        audience: Default::default(),
                        name: (*name).into(),
                        description: Self::description(name),
                        input_schema: input_schema.clone(),
                        provenance: Some(provenance.clone()),
                    })
                })
                .collect::<Vec<_>>();
            Self {
                catalog: tools
                    .iter()
                    .map(|tool| {
                        meerkat_core::ToolCatalogEntry::session_deferred(
                            Arc::clone(tool),
                            true,
                            provenance.clone(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into(),
                tools: tools.into(),
            }
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for DeferredCatalogTools {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            Arc::clone(&self.tools)
        }

        fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
            meerkat_core::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
            Ok(meerkat_core::types::ToolResult::new(
                call.id.to_string(),
                format!("ran {}", call.name),
                false,
            )
            .into())
        }
    }

    async fn deferred_catalog_fixture() -> (Fixture, SessionId) {
        deferred_catalog_fixture_with(DeferredCatalogTools::new()).await
    }

    async fn deferred_catalog_fixture_with(tools: DeferredCatalogTools) -> (Fixture, SessionId) {
        let fx = build_fixture_with_tools(Some(Arc::new(tools))).await;
        let session_id = create_realtime_session(&fx).await;
        // The live tool seam every composition installs on its host.
        fx.host
            .set_live_tool_dispatcher(Arc::new(ServiceLiveToolDispatcher::new(Arc::clone(
                &fx.service,
            ))));
        (fx, session_id)
    }

    fn tool_names(tools: &[meerkat_core::ToolDef]) -> std::collections::BTreeSet<String> {
        tools.iter().map(|tool| tool.name.to_string()).collect()
    }

    /// One tool call from the realtime provider, as the channel's pump feeds
    /// it to the host.
    async fn provider_tool_call(
        fx: &Fixture,
        channel: &LiveChannelId,
        call_id: &str,
        tool: &str,
        arguments: serde_json::Value,
    ) {
        fx.host
            .apply_observation(
                channel,
                &meerkat_core::live_adapter::LiveAdapterObservation::ToolCallRequested {
                    provider_call_id: meerkat_core::image_generation::ToolCallId::new(call_id),
                    tool_name: tool.into(),
                    arguments,
                },
            )
            .await
            .expect("the host handles the provider's tool call");
    }

    /// Where in the adapter's command stream the result of `call_id` was
    /// submitted, with its text and error flag.
    fn submitted_tool_result(
        commands: &[LiveAdapterCommand],
        call_id: &str,
    ) -> (usize, String, bool) {
        commands
            .iter()
            .enumerate()
            .find_map(|(index, command)| match command {
                LiveAdapterCommand::SubmitToolResult { result } if result.call_id.0 == call_id => {
                    Some((
                        index,
                        meerkat_core::types::text_content(&result.content),
                        result.is_error,
                    ))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("no result was submitted for {call_id}: {commands:?}"))
    }

    /// Every position in the adapter's command stream of a `Refresh` whose
    /// tool list holds `tool`.
    fn refreshes_listing(commands: &[LiveAdapterCommand], tool: &str) -> Vec<usize> {
        commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| match command {
                LiveAdapterCommand::Refresh { snapshot }
                    if tool_names(&snapshot.visible_tools).contains(tool) =>
                {
                    Some(index)
                }
                _ => None,
            })
            .collect()
    }

    async fn stored_row_count(fx: &Fixture, session_id: &SessionId) -> usize {
        fx.service
            .export_realtime_refresh_session_snapshot(session_id)
            .await
            .expect("the session's stored rows")
            .messages()
            .len()
    }

    /// A deferred tool the realtime model loads inside a live conversation
    /// is callable in that conversation: the provider gets the new tool list
    /// before it gets the load's result, the result says the tool is
    /// available now, a call of the tool runs, and the session's stored rows
    /// are unchanged by the load. A second load of the same tool says the
    /// same. The channel is composed the way a member host composes it.
    #[tokio::test]
    async fn a_deferred_tool_loaded_in_a_live_conversation_is_callable_in_it() {
        let (fx, session_id) = deferred_catalog_fixture().await;
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("live-pipeline-unit-deferred")
                    .expect("inproc comms runtime"),
            );
            fx.adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-live-pipeline-deferred",
                    ),
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
        let opened = member_host
            .open(&session_id, None, None)
            .await
            .expect("member open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        let control_tools: std::collections::BTreeSet<String> =
            ["tool_catalog_load", "tool_catalog_search"]
                .into_iter()
                .map(str::to_string)
                .collect();
        // The read a live open and refresh take their tool list from.
        let opened_with = tool_names(
            &fx.service
                .live_visible_tool_defs(&session_id)
                .await
                .expect("the session's live tool list"),
        );
        assert_eq!(
            opened_with, control_tools,
            "the conversation opens on the short list: the deferred tools stay behind the catalog"
        );
        let rows_before = stored_row_count(&fx, &session_id).await;

        provider_tool_call(
            &fx,
            &channel,
            "call-load",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP] }),
        )
        .await;
        let commands = adapter.commands();
        let (result_at, result_text, is_error) = submitted_tool_result(&commands, "call-load");
        assert!(!is_error, "the load succeeds: {result_text}");
        let refreshed_at = refreshes_listing(&commands, DEFERRED_LOOKUP);
        assert_eq!(
            refreshed_at.len(),
            1,
            "the provider's tool list is updated once with the loaded tool: {commands:?}"
        );
        assert!(
            refreshed_at[0] < result_at,
            "the tool list update precedes the load's result, so the model's \
             continuation already has the tool: {commands:?}"
        );
        assert!(
            refreshes_listing(&commands, DEFERRED_OTHER).is_empty(),
            "the tool nobody loaded stays behind the catalog"
        );
        let result: serde_json::Value =
            serde_json::from_str(&result_text).expect("the load's JSON result");
        assert_eq!(
            result["availability"], "available_now",
            "the load's result says the tool can be called now: {result}"
        );
        assert_eq!(result["resolutions"][0]["accepted"], true, "{result}");
        assert_eq!(
            stored_row_count(&fx, &session_id).await,
            rows_before,
            "a load made in a live conversation adds no stored row"
        );

        provider_tool_call(
            &fx,
            &channel,
            "call-tool",
            DEFERRED_LOOKUP,
            serde_json::json!({}),
        )
        .await;
        let commands = adapter.commands();
        let (_, call_text, call_is_error) = submitted_tool_result(&commands, "call-tool");
        assert!(
            !call_is_error && call_text == format!("ran {DEFERRED_LOOKUP}"),
            "the loaded tool runs in the same conversation: {call_text}"
        );

        provider_tool_call(
            &fx,
            &channel,
            "call-load-again",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP] }),
        )
        .await;
        let commands = adapter.commands();
        let (again_at, again_text, again_is_error) =
            submitted_tool_result(&commands, "call-load-again");
        assert!(!again_is_error, "a second load succeeds: {again_text}");
        let again: serde_json::Value =
            serde_json::from_str(&again_text).expect("the second load's JSON result");
        assert_eq!(
            again["availability"], "available_now",
            "a second load of an available tool says it is available: {again}"
        );
        let refreshed_at = refreshes_listing(&commands, DEFERRED_LOOKUP);
        assert!(
            refreshed_at.len() == 2 && refreshed_at[1] < again_at,
            "the second load updates the tool list again before its result: {commands:?}"
        );
    }

    /// A load that leaves nothing behind the catalog takes the control tools
    /// off the conversation's tool list with the same update: the update is
    /// the session's tool list as it is after the load, and a session shows
    /// the control tools only while a tool waits behind the catalog. A later
    /// call of the load tool is refused like a call of any tool the session
    /// does not show, and updates nothing.
    #[tokio::test]
    async fn a_live_load_that_leaves_nothing_behind_the_catalog_removes_the_control_tools() {
        let (fx, session_id) = deferred_catalog_fixture().await;
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("live-pipeline-unit-load-all")
                    .expect("inproc comms runtime"),
            );
            fx.adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-live-pipeline-load-all",
                    ),
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
        let opened = member_host
            .open(&session_id, None, None)
            .await
            .expect("member open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        provider_tool_call(
            &fx,
            &channel,
            "call-load-all",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP, DEFERRED_OTHER] }),
        )
        .await;
        let commands = adapter.commands();
        let (result_at, result_text, is_error) = submitted_tool_result(&commands, "call-load-all");
        assert!(!is_error, "the load succeeds: {result_text}");
        let result: serde_json::Value =
            serde_json::from_str(&result_text).expect("the load's JSON result");
        assert_eq!(result["availability"], "available_now", "{result}");
        let updates: Vec<(usize, std::collections::BTreeSet<String>)> = commands
            .iter()
            .enumerate()
            .filter_map(|(index, command)| match command {
                LiveAdapterCommand::Refresh { snapshot } => {
                    Some((index, tool_names(&snapshot.visible_tools)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 1, "one tool list update: {commands:?}");
        let (updated_at, updated_tools) = &updates[0];
        assert!(*updated_at < result_at, "the update precedes the result");
        assert_eq!(
            *updated_tools,
            [DEFERRED_LOOKUP, DEFERRED_OTHER]
                .into_iter()
                .map(str::to_string)
                .collect::<std::collections::BTreeSet<String>>(),
            "both tools are on the conversation's tool list and the control tools are not: \
             nothing waits behind the catalog any more"
        );

        provider_tool_call(
            &fx,
            &channel,
            "call-load-stale",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP] }),
        )
        .await;
        let commands = adapter.commands();
        let (_, stale_text, stale_is_error) = submitted_tool_result(&commands, "call-load-stale");
        assert!(
            stale_is_error && stale_text.contains("access_denied"),
            "a call of the load tool the session no longer shows is refused: {stale_text}"
        );
        assert_eq!(
            refreshes_listing(&commands, DEFERRED_LOOKUP).len(),
            1,
            "a refused call updates nothing: {commands:?}"
        );

        provider_tool_call(
            &fx,
            &channel,
            "call-other",
            DEFERRED_OTHER,
            serde_json::json!({}),
        )
        .await;
        let commands = adapter.commands();
        let (_, call_text, call_is_error) = submitted_tool_result(&commands, "call-other");
        assert!(
            !call_is_error && call_text == format!("ran {DEFERRED_OTHER}"),
            "a loaded tool runs in the same conversation: {call_text}"
        );
    }

    /// A tool loaded inside a live conversation is loaded for the session,
    /// not for that conversation only: the session's stored visibility state
    /// records it, a channel opened after the first one closed lists it, and
    /// the session's durable record still holds it once the live actor is
    /// gone, which is what a session built again from the store reads.
    #[tokio::test]
    async fn a_tool_loaded_in_a_live_conversation_stays_loaded_for_the_session() {
        let (fx, session_id) = deferred_catalog_fixture().await;
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("live-pipeline-unit-stays-loaded")
                    .expect("inproc comms runtime"),
            );
            fx.adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-live-pipeline-stays-loaded",
                    ),
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
        let opened = member_host
            .open(&session_id, None, None)
            .await
            .expect("member open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        provider_tool_call(
            &fx,
            &channel,
            "call-load",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP] }),
        )
        .await;
        let commands = adapter.commands();
        let (_, result_text, is_error) = submitted_tool_result(&commands, "call-load");
        assert!(!is_error, "the load succeeds: {result_text}");

        // What is stored with the session: the loaded tool is an active
        // request of its visibility state, so a build from the store has it.
        let stored = fx
            .service
            .export_realtime_refresh_session_snapshot(&session_id)
            .await
            .expect("the session as stored")
            .tool_visibility_state()
            .expect("a readable visibility state")
            .expect("the session stores its visibility state");
        assert!(
            stored
                .active_requested_deferred_names
                .iter()
                .any(|name| name.as_str() == DEFERRED_LOOKUP),
            "the load is stored with the session: {stored:?}"
        );

        let expected: std::collections::BTreeSet<String> =
            [DEFERRED_LOOKUP, "tool_catalog_load", "tool_catalog_search"]
                .into_iter()
                .map(str::to_string)
                .collect();

        // A second conversation: the first channel is closed, the next one
        // opens on the session's tool list, which has the loaded tool.
        let status = member_host
            .close(&session_id, &opened.channel_id)
            .await
            .expect("close succeeds");
        assert_eq!(status, LiveCloseStatus::Closed);
        let reopened = member_host
            .open(&session_id, None, None)
            .await
            .expect("a second open succeeds");
        assert_eq!(
            tool_names(
                &fx.service
                    .live_visible_tool_defs(&session_id)
                    .await
                    .expect("the session's live tool list"),
            ),
            expected,
            "a channel opened after the load lists the loaded tool; the other tool still \
             waits behind the catalog"
        );
        let status = member_host
            .close(&session_id, &reopened.channel_id)
            .await
            .expect("the second close succeeds");
        assert_eq!(status, LiveCloseStatus::Closed);

        // What a restore reads: the session's durable record holds the load,
        // also once the live actor is gone.
        fx.service
            .discard_live_session(&session_id)
            .await
            .expect("discard the live session");
        let durable = fx
            .service
            .load_authoritative_session(&session_id)
            .await
            .expect("the durable record loads")
            .expect("the durable record survives the live actor")
            .tool_visibility_state()
            .expect("a readable durable visibility state")
            .expect("the durable record stores the visibility state");
        assert!(
            durable
                .active_requested_deferred_names
                .iter()
                .any(|name| name.as_str() == DEFERRED_LOOKUP),
            "a session built again from its durable record has the loaded tool: {durable:?}"
        );
    }

    /// A loaded tool whose definition the conversation's provider would
    /// refuse does not end the conversation. The adapter's own check names
    /// it before anything is sent: the tool list update leaves it out, the
    /// load's result names it as loaded for the session and not in this
    /// conversation, with the provider's reason, and the other tool of the
    /// same load is available now and runs.
    #[tokio::test]
    async fn a_loaded_tool_the_provider_would_refuse_is_withheld_and_named() {
        let (fx, session_id) =
            deferred_catalog_fixture_with(DeferredCatalogTools::with_an_unsendable_tool()).await;
        #[cfg(feature = "comms")]
        {
            let comms: Arc<dyn meerkat_core::agent::CommsRuntime> = Arc::new(
                meerkat_comms::CommsRuntime::inproc_only("live-pipeline-unit-unsendable")
                    .expect("inproc comms runtime"),
            );
            fx.adapter
                .maybe_spawn_mob_comms_drain(
                    &session_id,
                    comms,
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(
                        "mob-live-pipeline-unsendable",
                    ),
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
        let opened = member_host
            .open(&session_id, None, None)
            .await
            .expect("member open succeeds");
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        provider_tool_call(
            &fx,
            &channel,
            "call-load",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP, DEFERRED_UNSENDABLE] }),
        )
        .await;
        let commands = adapter.commands();
        let (result_at, result_text, is_error) = submitted_tool_result(&commands, "call-load");
        assert!(
            !is_error,
            "the load is a result, not an error: {result_text}"
        );
        let refreshed_at = refreshes_listing(&commands, DEFERRED_LOOKUP);
        assert!(
            refreshed_at.len() == 1 && refreshed_at[0] < result_at,
            "the tool the provider takes is sent before the result: {commands:?}"
        );
        assert!(
            refreshes_listing(&commands, DEFERRED_UNSENDABLE).is_empty(),
            "the tool the provider would refuse is never sent: {commands:?}"
        );
        let result: serde_json::Value =
            serde_json::from_str(&result_text).expect("the load's JSON result");
        assert_eq!(result["availability"], "mixed", "{result}");
        let entry = |name: &str| -> serde_json::Value {
            result["tool_availability"]
                .as_array()
                .and_then(|entries| entries.iter().find(|entry| entry["name"] == name))
                .cloned()
                .unwrap_or_else(|| panic!("an entry for {name}: {result}"))
        };
        assert_eq!(
            entry(DEFERRED_LOOKUP),
            serde_json::json!({ "name": DEFERRED_LOOKUP, "availability": "available_now" })
        );
        let withheld = entry(DEFERRED_UNSENDABLE);
        assert_eq!(
            withheld["availability"], "loaded_not_in_this_conversation",
            "{result}"
        );
        assert!(
            withheld["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("root is not an object")),
            "the provider's reason is given: {result}"
        );
        // Loaded for the session all the same.
        assert!(
            tool_names(
                &fx.service
                    .live_visible_tool_defs(&session_id)
                    .await
                    .expect("the session's live tool list"),
            )
            .contains(DEFERRED_UNSENDABLE)
        );

        // The conversation goes on: the tool that was sent runs.
        provider_tool_call(
            &fx,
            &channel,
            "call-tool",
            DEFERRED_LOOKUP,
            serde_json::json!({}),
        )
        .await;
        let commands = adapter.commands();
        let (_, call_text, call_is_error) = submitted_tool_result(&commands, "call-tool");
        assert!(
            !call_is_error && call_text == format!("ran {DEFERRED_LOOKUP}"),
            "the loaded tool runs in the same conversation: {call_text}"
        );
    }

    /// A composition that installs no way to update the provider's tool list
    /// still loads the tool for the session, and says so: the load's result
    /// is not a bare acceptance, it states that the tool cannot be called in
    /// this conversation yet. A call of it is still served for the session.
    #[tokio::test]
    async fn a_live_load_without_a_tool_list_update_says_the_tool_is_not_available_here() {
        let (fx, session_id) = deferred_catalog_fixture().await;
        let factory = ScriptedRealtimeSessionFactory::new();
        let ingress = RecordingIngress::default();
        // Opened through the bare pipeline: no member host is composed.
        let opened = open_with_realtime_model(&fx, &ingress, &factory, &session_id).await;
        let channel = LiveChannelId::new(&opened.channel_id);
        let adapter = factory.adapters().pop().expect("scripted adapter minted");

        provider_tool_call(
            &fx,
            &channel,
            "call-load",
            "tool_catalog_load",
            serde_json::json!({ "names": [DEFERRED_LOOKUP] }),
        )
        .await;
        let commands = adapter.commands();
        let (_, result_text, is_error) = submitted_tool_result(&commands, "call-load");
        assert!(
            !is_error,
            "the tool is loaded for the session: {result_text}"
        );
        assert!(
            refresh_snapshots(&adapter).is_empty(),
            "nothing can update this conversation's tool list: {commands:?}"
        );
        let result: serde_json::Value =
            serde_json::from_str(&result_text).expect("the load's JSON result");
        assert_eq!(
            result["availability"], "loaded_not_in_this_conversation",
            "the result says the tool is loaded but not callable here yet: {result}"
        );

        let visible = fx
            .service
            .live_visible_tool_defs(&session_id)
            .await
            .expect("the session's visible tools");
        assert!(
            tool_names(&visible).contains(DEFERRED_LOOKUP),
            "the tool is loaded for the session, so the next open lists it: {visible:?}"
        );
    }
}
