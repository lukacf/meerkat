//! Booted-service rows of the member rewind fork seam.
//!
//! A member rewind rotates a mob member onto a durable fork of its own
//! session. These rows boot a real `PersistentSessionService` and pin what
//! the rotation needs of it: a preflight that decides every refusal of the
//! fork with no session row written and returns the evidence the planned
//! child must show, a planned fork whose child is born bound to the same
//! member, a verification that compares every evidence field, and reads of
//! an archived lineage session that write nothing to it.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[cfg(all(feature = "session-store", not(target_arch = "wasm32")))]
mod member_rewind_fork {

    use std::collections::BTreeSet;
    use std::sync::Arc;

    use async_trait::async_trait;
    use meerkat_core::SessionServiceTranscriptEditExt;
    use meerkat_core::error::AgentError;
    use meerkat_core::generated::session_document::ObservedSessionTailKind;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy,
        MemberRewindChildObservation, MemberRewindEvidenceField, MemberRewindForkEvidence,
        MemberRewindForkRequest, MemberRewindSource, SessionError, SessionService,
    };
    use meerkat_core::types::{ContentInput, RunResult};
    use meerkat_core::{
        AgentLlmClient, AssistantBlock, BlockAssistantMessage, ContentBlock,
        DurableForkSourceAdmission, DurableSessionForkTarget, ForkInstructionPolicy,
        ForkInstructionRefusalCause, ImageData, InstructionActivationExpectation,
        InstructionActivationId, InstructionActivationRequest, InstructionContentDigest,
        InstructionKey, InstructionNamespace, InstructionRevisionId, InstructionRevisionRef,
        Message, MobMemberBinding, Provider, Session, SessionFilter, SessionId, SessionLlmIdentity,
        SessionLlmRequestPolicy, StopReason, SystemPromptOverride, ToolResult,
        TransientTurnContextStateHandle, UserMessage,
    };
    use meerkat_core::{BlobAddressAttestation, BlobId, BlobPayload, BlobStore, BlobStoreError};
    use meerkat_runtime::{
        InMemoryRuntimeStore, LogicalRuntimeId, RuntimeStore, SerializedSessionSnapshot,
    };
    use meerkat_session::event_store::{EventStore, EventStoreError, FileEventStore, StoredEvent};
    use meerkat_session::projector::SessionProjector;
    use meerkat_session::{
        ForkInstructionPolicies, MachineSessionArchiveProtocol, PersistentSessionService,
        SessionAgent, SessionAgentBuilder, SessionSnapshot,
    };
    use meerkat_store::{MemoryBlobStore, MemoryStore, SessionStore};
    use tokio::sync::mpsc;

    /// Holds a turn of a booted session open, so the session reads active:
    /// armed, the next run waits here until it is released.
    #[derive(Default)]
    struct TurnGate {
        armed: std::sync::atomic::AtomicBool,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    struct DummyAgent {
        session: Session,
        turn_gate: Arc<TurnGate>,
    }

    #[async_trait]
    impl SessionAgent for DummyAgent {
        async fn run_with_events(
            &mut self,
            _prompt: ContentInput,
            _event_tx: mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<RunResult, AgentError> {
            if self
                .turn_gate
                .armed
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                self.turn_gate.entered.notify_one();
                self.turn_gate.release.notified().await;
            }
            Err(AgentError::ConfigError(
                "a member rewind fork row completes no turn".to_string(),
            ))
        }

        fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {}

        fn set_turn_tool_overlay(
            &mut self,
            _overlay: Option<meerkat_core::service::TurnToolOverlay>,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn hot_swap_llm_identity(
            &mut self,
            _client: Arc<dyn AgentLlmClient>,
            _identity: SessionLlmIdentity,
            _request_policy: SessionLlmRequestPolicy,
        ) -> Result<(), AgentError> {
            Ok(())
        }

        fn cancel(&mut self) {}

        fn session_id(&self) -> SessionId {
            self.session.id().clone()
        }

        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot {
                created_at: self.session.created_at(),
                updated_at: self.session.updated_at(),
                message_count: self.session.messages().len(),
                total_tokens: 0,
                usage: Default::default(),
                last_assistant_text: None,
            }
        }

        fn session_clone(&self) -> Result<Session, AgentError> {
            Ok(self.session.clone())
        }

        fn session_transcript_authority(
            &self,
        ) -> Result<meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot, AgentError>
        {
            meerkat_session::ephemeral::SessionTranscriptAuthoritySnapshot::from_session(
                &self.session,
            )
        }

        fn durable_llm_identity(&self) -> Option<SessionLlmIdentity> {
            Some(SessionLlmIdentity {
                model: "noop".to_string(),
                provider: Provider::Other,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            })
        }

        fn observed_session_tail(&self) -> ObservedSessionTailKind {
            ObservedSessionTailKind::Empty
        }

        fn transient_turn_context_state(&self) -> TransientTurnContextStateHandle {
            TransientTurnContextStateHandle::new()
        }
    }

    #[derive(Default)]
    struct DummyBuilder {
        turn_gate: Arc<TurnGate>,
    }

    #[async_trait]
    impl SessionAgentBuilder for DummyBuilder {
        type Agent = DummyAgent;

        fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
            meerkat_core::SessionCommsIdentityRoot::NotKept
        }

        async fn build_agent(
            &self,
            req: &CreateSessionRequest,
            _event_tx: mpsc::Sender<meerkat_core::AgentEvent>,
        ) -> Result<Self::Agent, SessionError> {
            let session = req
                .build
                .as_ref()
                .and_then(|build| build.resume_session.clone())
                .unwrap_or_default();
            Ok(DummyAgent {
                session,
                turn_gate: Arc::clone(&self.turn_gate),
            })
        }
    }

    /// A blob store whose next read after it is armed waits until it is
    /// released: it holds a durable fork inside its blob preflight, where
    /// the fork holds its source's guard and has committed nothing yet.
    #[derive(Default)]
    struct GatedBlobStore {
        inner: MemoryBlobStore,
        armed: std::sync::atomic::AtomicBool,
        reached: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait]
    impl BlobStore for GatedBlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            data: &str,
        ) -> Result<meerkat_core::BlobRef, BlobStoreError> {
            self.inner.put_image(media_type, data).await
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                self.reached.notify_one();
                self.release.notified().await;
            }
            self.inner.get(blob_id).await
        }

        async fn attest_address(
            &self,
            blob_id: &BlobId,
            payload: &BlobPayload,
        ) -> Result<BlobAddressAttestation, BlobStoreError> {
            self.inner.attest_address(blob_id, payload).await
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.inner.delete(blob_id).await
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    /// An event log that keeps its rows as they were appended and reads
    /// them back, and offers no transcript rewrite audit of its own: a
    /// service over it reconciles a session's rewrites from the logged
    /// rows, as it does over any event store without that capability.
    #[derive(Default)]
    struct RowLogEventStore {
        rows: std::sync::Mutex<std::collections::HashMap<SessionId, Vec<StoredEvent>>>,
    }

    #[async_trait]
    impl EventStore for RowLogEventStore {
        async fn append_envelopes(
            &self,
            session_id: &SessionId,
            envelopes: &[meerkat_core::EventEnvelope<meerkat_core::AgentEvent>],
        ) -> Result<u64, EventStoreError> {
            let mut rows = self.rows.lock().expect("event rows");
            let log = rows.entry(session_id.clone()).or_default();
            for envelope in envelopes {
                let seq = log.len() as u64 + 1;
                log.push(StoredEvent {
                    seq,
                    schema_version: meerkat_session::event_store::EVENT_SCHEMA_VERSION,
                    timestamp: std::time::SystemTime::now(),
                    source: envelope.source.clone(),
                    mob_id: envelope.mob_id.clone(),
                    stream_seq: envelope.seq,
                    event: envelope.payload.clone(),
                });
            }
            Ok(log.len() as u64)
        }

        async fn append_transcript_rewrite_receipt_exact(
            &self,
            _session_id: &SessionId,
            _receipt: &meerkat_core::TranscriptRewriteAuditReceiptBatch,
            _final_assistant_text: Option<&str>,
        ) -> Result<
            meerkat_session::event_store::ExactTranscriptRewriteReceiptAppend,
            EventStoreError,
        > {
            Err(EventStoreError::Store(
                "this row log publishes no transcript rewrite receipts".to_string(),
            ))
        }

        async fn record_projection_halt(
            &self,
            _session_id: &SessionId,
            _reason: &str,
        ) -> Result<(), EventStoreError> {
            Ok(())
        }

        async fn projection_halt(
            &self,
            _session_id: &SessionId,
        ) -> Result<Option<meerkat_session::event_store::EventProjectionHaltMarker>, EventStoreError>
        {
            Ok(None)
        }

        async fn read_from(
            &self,
            session_id: &SessionId,
            from_seq: u64,
        ) -> Result<Vec<StoredEvent>, EventStoreError> {
            Ok(self
                .rows
                .lock()
                .expect("event rows")
                .get(session_id)
                .map(|log| {
                    log.iter()
                        .filter(|row| row.seq >= from_seq)
                        .cloned()
                        .collect()
                })
                .unwrap_or_default())
        }

        async fn last_seq(&self, session_id: &SessionId) -> Result<u64, EventStoreError> {
            Ok(self
                .rows
                .lock()
                .expect("event rows")
                .get(session_id)
                .map_or(0, |log| log.len() as u64))
        }
    }

    /// A blob store that also holds blobs under an address recipe of its
    /// own, as a store written by an earlier address recipe does: it reads
    /// them back under that address and attests it. Everything else is the
    /// in-memory store's.
    #[derive(Default)]
    struct StoreAddressedBlobStore {
        inner: MemoryBlobStore,
        own: std::sync::Mutex<std::collections::HashMap<BlobId, (String, String)>>,
    }

    impl StoreAddressedBlobStore {
        /// Hold `data` under this store's own address `blob_id`.
        fn hold_under_own_address(&self, blob_id: &BlobId, media_type: &str, data: &str) {
            self.own
                .lock()
                .expect("own blobs")
                .insert(blob_id.clone(), (media_type.to_string(), data.to_string()));
        }

        fn own(&self, blob_id: &BlobId) -> Option<(String, String)> {
            self.own.lock().expect("own blobs").get(blob_id).cloned()
        }
    }

    #[async_trait]
    impl BlobStore for StoreAddressedBlobStore {
        async fn put_image(
            &self,
            media_type: &str,
            data: &str,
        ) -> Result<meerkat_core::BlobRef, BlobStoreError> {
            self.inner.put_image(media_type, data).await
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            match self.own(blob_id) {
                Some((media_type, data)) => Ok(BlobPayload {
                    blob_id: blob_id.clone(),
                    media_type,
                    data,
                }),
                None => self.inner.get(blob_id).await,
            }
        }

        async fn attest_address(
            &self,
            blob_id: &BlobId,
            payload: &BlobPayload,
        ) -> Result<BlobAddressAttestation, BlobStoreError> {
            Ok(match self.own(blob_id) {
                Some((media_type, data))
                    if media_type == payload.media_type && data == payload.data =>
                {
                    BlobAddressAttestation::StoreAddress
                }
                _ => BlobAddressAttestation::Unattested,
            })
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.inner.delete(blob_id).await
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    /// One booted persistent service and the stores under it.
    struct Booted {
        service: PersistentSessionService<DummyBuilder>,
        session_store: Arc<dyn SessionStore>,
        runtime_store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        /// The durable event log, when the service keeps one.
        event_store: Option<Arc<dyn EventStore>>,
        /// The runtime authority that retires a session's runtime when the
        /// session is archived.
        machine: meerkat_runtime::MeerkatMachine,
        /// Holds a turn of this service's sessions open when armed.
        turn_gate: Arc<TurnGate>,
    }

    /// The event log a booted service keeps.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum EventLog {
        /// None: reads replay no transcript rewrite audit.
        None,
        /// [`RowLogEventStore`].
        Rows,
        /// The file event store, with its own transcript rewrite audit and
        /// receipts.
        File,
    }

    fn boot() -> Booted {
        boot_over(Arc::new(MemoryBlobStore::new()), EventLog::None)
    }

    /// [`boot`] with a durable event log, so a read replays the transcript
    /// rewrite audit.
    fn boot_with_event_projection() -> Booted {
        boot_over(Arc::new(MemoryBlobStore::new()), EventLog::Rows)
    }

    fn boot_over(blob_store: Arc<dyn BlobStore>, event_log: EventLog) -> Booted {
        let store: Arc<dyn SessionStore> = Arc::new(MemoryStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
        let machine = meerkat_runtime::MeerkatMachine::persistent(
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        )
        .expect("the runtime authority");
        let turn_gate = Arc::new(TurnGate::default());
        let service = PersistentSessionService::new(
            DummyBuilder {
                turn_gate: Arc::clone(&turn_gate),
            },
            4,
            Arc::clone(&store),
            Arc::clone(&runtime_store),
            Arc::clone(&blob_store),
        );
        let (service, event_store) = if event_log == EventLog::None {
            (service, None)
        } else {
            let root = tempfile::tempdir().expect("tempdir").keep();
            let event_store: Arc<dyn EventStore> = match event_log {
                EventLog::File => Arc::new(FileEventStore::new(root.join("events"))),
                _ => Arc::new(RowLogEventStore::default()),
            };
            (
                service.with_event_projection(
                    Arc::clone(&event_store),
                    Arc::new(SessionProjector::new(root.join(".rkat"))),
                ),
                Some(event_store),
            )
        };
        Booted {
            service,
            session_store: store,
            runtime_store,
            blob_store,
            event_store,
            machine,
            turn_gate,
        }
    }

    /// A service over the booted stores that keeps no event log, as a host
    /// that commits a transcript rewrite without one.
    fn service_without_event_log(booted: &Booted) -> PersistentSessionService<DummyBuilder> {
        PersistentSessionService::new(
            DummyBuilder::default(),
            4,
            Arc::clone(&booted.session_store),
            Arc::clone(&booted.runtime_store),
            Arc::clone(&booted.blob_store),
        )
    }

    /// A durable session of `binding` (of no member when `None`) whose
    /// committed transcript rewrite has no audit row in the booted
    /// service's event log: the rewrite was committed by a service that
    /// kept none, which is returned with it.
    async fn rewritten_without_audit(
        booted: &Booted,
        binding: Option<&MobMemberBinding>,
    ) -> (PersistentSessionService<DummyBuilder>, SessionId) {
        let writer = service_without_event_log(booted);
        let mut session = answered_turns();
        match binding {
            Some(binding) => bind(&mut session, binding),
            None => bind_to_no_member(&mut session),
        }
        let source = durable_in(&writer, session).await;
        let before = writer
            .load_durable_session_body(&source)
            .await
            .expect("load the source")
            .expect("the source is durable");
        writer
            .rewrite_session_transcript(
                &source,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                        start: 1,
                        end: 2,
                    },
                    replacement: vec![assistant("a shorter first answer")],
                    reason: meerkat_core::TranscriptRewriteReason::new("compaction"),
                    actor: Some("member-rewind-row".to_string()),
                    expected_parent_revision: Some(
                        before
                            .transcript_revision()
                            .expect("the revision before the rewrite"),
                    ),
                    running_behavior: meerkat_core::TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("commit a transcript rewrite");
        (writer, source)
    }

    /// The last sequence number of `id`'s event log.
    async fn event_log_end(booted: &Booted, id: &SessionId) -> u64 {
        booted
            .event_store
            .as_ref()
            .expect("the booted service keeps an event log")
            .last_seq(id)
            .await
            .expect("event log end")
    }

    /// Run `while_active` while a turn of `id` is held open, so that the
    /// session reads active throughout.
    async fn while_a_turn_runs<T>(
        booted: &Booted,
        id: &SessionId,
        while_active: impl std::future::Future<Output = T>,
    ) -> T {
        booted
            .turn_gate
            .armed
            .store(true, std::sync::atomic::Ordering::SeqCst);
        booted
            .machine
            .register_session(id.clone())
            .await
            .expect("register the session with its runtime");
        let admission = booted
            .service
            .reserve_runtime_turn_admission(id)
            .await
            .expect("reserve the turn's admission");
        let turn = async {
            booted
                .service
                .run_machine_committed_live_turn(
                    meerkat_session::MachineServiceTurnCommitProtocol::from_machine(
                        &booted.machine,
                    ),
                    id,
                    meerkat_core::service::StartTurnRequest {
                        injected_context: Vec::new(),
                        prompt: "a turn that is held open".to_string().into(),
                        system_prompt: None,
                        event_tx: None,
                        runtime: meerkat_core::service::StartTurnRuntimeSemantics::default(),
                    },
                    admission,
                )
                .await
                .map(|_| ())
                .map_err(|(error, _admission)| error)
        };
        tokio::pin!(turn);
        // The turn is driven until its agent waits at the gate.
        tokio::select! {
            biased;
            ended = &mut turn => panic!(
                "premise: the turn reaches its agent and is held there: {ended:?}"
            ),
            () = booted.turn_gate.entered.notified() => {}
            () = tokio::time::sleep(std::time::Duration::from_secs(30)) => panic!(
                "premise: the turn reaches its agent"
            ),
        }
        let observed = async {
            assert!(
                booted
                    .service
                    .observe_live_session_view(id)
                    .await
                    .expect("observe the running session")
                    .is_some_and(|view| view.state.is_active),
                "premise: the session reads active while its turn runs"
            );
            let result = while_active.await;
            booted.turn_gate.release.notify_one();
            result
        };
        let (_turn, result) = both_within_the_rows_bound(turn, observed).await;
        result
    }

    /// Archive `id` as a runtime-backed surface does.
    async fn archive(booted: &Booted, id: &SessionId) {
        booted
            .service
            .archive_with_machine_protocol(
                id,
                MachineSessionArchiveProtocol::from_machine(&booted.machine),
            )
            .await
            .expect("archive the session");
        assert!(
            matches!(
                booted.service.read(id).await,
                Err(SessionError::NotFound { .. })
            ),
            "an archived session has no session read"
        );
    }

    /// The member every row rewinds.
    fn member() -> MobMemberBinding {
        MobMemberBinding {
            mob_id: "mob-1".to_string(),
            role: "member".to_string(),
            member: "researcher".to_string(),
        }
    }

    /// Another member of the same mob.
    fn another_member() -> MobMemberBinding {
        MobMemberBinding {
            mob_id: "mob-1".to_string(),
            role: "member".to_string(),
            member: "archivist".to_string(),
        }
    }

    /// The canonical metadata of a session bound to `binding`, as a mob
    /// build writes it: the comms name is the binding's.
    fn member_metadata(binding: &MobMemberBinding) -> meerkat_core::SessionMetadata {
        let realm = meerkat_core::RealmId::parse("global").expect("realm");
        let tooling = meerkat_core::SessionTooling {
            tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::AllowList(
                ["read_file".to_string()].into_iter().collect(),
            )),
            ..meerkat_core::SessionTooling::default()
        };
        meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::session_metadata_schema_version(),
            model: "test".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            tooling,
            keep_alive: false,
            comms_name: Some(binding.comms_name().expect("comms name").to_string()),
            peer_meta: None,
            realm_id: Some(realm.clone()),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm,
                binding: meerkat_core::connection::BindingId::parse("anthropic").expect("binding"),
                profile: None,
                origin: meerkat_core::connection::BindingOrigin::default(),
            }),
            mob_member_binding: Some(binding.clone()),
        }
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn assistant(text: &str) -> Message {
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: text.to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
        ))
    }

    /// Give `session` the execution configuration of a session of `binding`.
    fn bind(session: &mut Session, binding: &MobMemberBinding) {
        session
            .set_session_metadata(member_metadata(binding))
            .expect("member metadata");
        session
            .set_build_state(meerkat_core::SessionBuildState {
                app_context: Some(serde_json::json!({"surface": "member-rewind"})),
                ..meerkat_core::SessionBuildState::default()
            })
            .expect("build state");
    }

    /// Give `session` canonical session metadata and build state that name
    /// no mob member: a session that is nobody's member session.
    fn bind_to_no_member(session: &mut Session) {
        bind(session, &member());
        let mut metadata = session
            .try_session_metadata()
            .expect("metadata decodes")
            .expect("metadata present");
        metadata.mob_member_binding = None;
        metadata.comms_name = None;
        session
            .set_session_metadata(metadata)
            .expect("metadata of no member");
    }

    /// Two answered turns of the member: four messages, every even count a
    /// complete boundary whose tail is answered.
    fn answered_turns() -> Session {
        let mut session = Session::new();
        session.push(user("first question"));
        session.push(assistant("first answer"));
        session.push(user("second question"));
        session.push(assistant("second answer"));
        bind(&mut session, &member());
        session
    }

    /// Make `session` durable in the booted service, without running it.
    async fn durable(booted: &Booted, session: Session) -> SessionId {
        durable_in(&booted.service, session).await
    }

    async fn durable_in(
        service: &PersistentSessionService<DummyBuilder>,
        session: Session,
    ) -> SessionId {
        let id = session.id().clone();
        let request = CreateSessionRequest {
            injected_context: Vec::new(),
            model: "test".to_string(),
            prompt: "seed".to_string().into(),
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            system_prompt: SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            build: Some(meerkat_core::service::SessionBuildOptions {
                resume_session: Some(session),
                ..meerkat_core::service::SessionBuildOptions::default()
            }),
            labels: None,
        };
        service
            .create_session(request)
            .await
            .expect("create the durable session");
        id
    }

    /// The committed body of `id`.
    async fn body(booted: &Booted, id: &SessionId) -> Session {
        booted
            .service
            .load_durable_session_body(id)
            .await
            .expect("load the durable body")
            .expect("the session is durable")
    }

    /// The bytes the runtime store keeps for `id`, or `None` for a session
    /// it holds no row of.
    async fn stored_row(booted: &Booted, id: &SessionId) -> Option<Arc<Vec<u8>>> {
        booted
            .runtime_store
            .load_session_snapshot(&LogicalRuntimeId::for_session(id))
            .await
            .expect("read the stored session row")
    }

    /// Every session the runtime store holds a row of.
    async fn stored_sessions(booted: &Booted) -> BTreeSet<String> {
        booted
            .runtime_store
            .list_runtime_session_catalog_entries(SessionFilter::default())
            .await
            .expect("list the stored sessions")
            .into_iter()
            .map(|entry| entry.session_id().to_string())
            .collect()
    }

    /// A member rewind fork of `source` at `message_count` into a fresh
    /// planned child.
    fn rewind_of(
        source: &SessionId,
        message_count: usize,
        read: MemberRewindSource,
    ) -> MemberRewindForkRequest {
        MemberRewindForkRequest {
            source_session_id: source.clone(),
            message_count,
            planned_child_session_id: SessionId::new(),
            member_binding: member(),
            source: read,
        }
    }

    /// The fork target of an ordinary durable fork bound to `binding` under
    /// `policy`: how the rows build a fork child with today's fork.
    fn fork_target(
        binding: MobMemberBinding,
        policy: ForkInstructionPolicy,
    ) -> DurableSessionForkTarget {
        DurableSessionForkTarget {
            member_binding: binding,
            cache_identity: None,
            source_admission: DurableForkSourceAdmission::Quiescent,
            inherited_instructions: policy,
            declared_instructions: policy,
        }
    }

    fn activation(key: &str, body: &str, activation: &str) -> InstructionActivationRequest {
        InstructionActivationRequest {
            revision: InstructionRevisionRef {
                namespace: InstructionNamespace::new("app.example").expect("namespace"),
                key: InstructionKey::new(key).expect("key"),
                revision_id: InstructionRevisionId::new(format!("rev-{activation}"))
                    .expect("revision"),
                content_sha256: InstructionContentDigest::for_body(body),
            },
            activation_id: InstructionActivationId::new(activation).expect("activation id"),
            expectation: InstructionActivationExpectation::Absent,
            supersedes: None,
            body: body.to_string(),
        }
    }

    /// A member session that activated `keys` and then answered a turn.
    fn session_with_activated_keys(keys: &[&str]) -> Session {
        let mut session = Session::new();
        session.push(user("first question"));
        session.push(assistant("first answer"));
        for key in keys {
            session
                .activate_instruction(activation(
                    key,
                    &format!("policy {key}"),
                    &format!("act-{key}"),
                ))
                .expect("activate the key");
        }
        session.push(user("second question"));
        session.push(assistant("second answer"));
        bind(&mut session, &member());
        session
    }

    /// What the fork applies to the planned prefix of `source` at
    /// `message_count`, computed on a clone as the fork computes it.
    fn fork_instruction_answer(
        source: &Session,
        message_count: usize,
    ) -> Result<usize, meerkat_core::ForkInstructionRefusal> {
        let mut clone = source
            .fork_at_complete_boundary(message_count)
            .expect("a complete boundary");
        clone.apply_fork_instruction_policies(
            source.id(),
            ForkInstructionPolicy::ReactivateInherited,
            ForkInstructionPolicy::ReactivateInherited,
        )
    }

    /// A refused preflight left nothing: the planned child does not exist,
    /// no session row was added, and the source's row is byte-identical.
    async fn assert_nothing_written(
        booted: &Booted,
        request: &MemberRewindForkRequest,
        sessions_before: &BTreeSet<String>,
        source_row_before: &Option<Arc<Vec<u8>>>,
    ) {
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "a refused preflight leaves no planned child"
        );
        assert_eq!(
            &stored_sessions(booted).await,
            sessions_before,
            "a refused preflight adds no session row"
        );
        assert_eq!(
            &stored_row(booted, &request.source_session_id).await,
            source_row_before,
            "a refused preflight leaves the source's row as it was"
        );
    }

    #[tokio::test]
    async fn preflight_refuses_a_head_copied_from_an_earlier_session() {
        let booted = boot();
        let earlier = durable(&booted, session_with_activated_keys(&["primary"])).await;
        // A fork under the explicit-activation policy copies the activation
        // row and re-activates nothing: the head of the key in the child is
        // the earlier session's row.
        let source = booted
            .service
            .fork_durable_session(
                &earlier,
                None,
                None,
                Some(fork_target(
                    member(),
                    ForkInstructionPolicy::RequireExplicitActivation,
                )),
            )
            .await
            .expect("fork the earlier session")
            .session_id;
        let source_body = body(&booted, &source).await;
        let message_count = source_body.messages().len();
        let expected = fork_instruction_answer(&source_body, message_count)
            .expect_err("the copied head is not the source's own");
        assert_eq!(
            expected.cause,
            ForkInstructionRefusalCause::NotEffectiveInSource,
            "the row's source holds a head copied from an earlier session"
        );

        let request = rewind_of(&source, message_count, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a head copied from an earlier session refuses the preflight");
        match refused {
            SessionError::ForkInstructionRefused(refusal) => assert_eq!(
                refusal, expected,
                "the preflight answers what the fork answers for the same prefix"
            ),
            other => panic!("expected the fork instruction refusal, got {other:?}"),
        }
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn preflight_refuses_a_boundary_inside_a_reactivation_suffix() {
        let booted = boot();
        let earlier = durable(&booted, session_with_activated_keys(&["alpha", "zeta"])).await;
        let earlier_len = body(&booted, &earlier).await.messages().len();
        // A fork under the inheriting policy ends with one re-activation row
        // per key, in key order: alpha, then zeta.
        let fork = booted
            .service
            .fork_durable_session(
                &earlier,
                None,
                None,
                Some(fork_target(
                    member(),
                    ForkInstructionPolicy::ReactivateInherited,
                )),
            )
            .await
            .expect("fork the earlier session");
        assert_eq!(
            fork.reactivated_instruction_count, 2,
            "the source ends with a re-activation suffix of two rows"
        );
        let source = fork.session_id;
        let source_body = body(&booted, &source).await;
        assert_eq!(source_body.messages().len(), earlier_len + 2);
        // One row into the suffix: alpha is re-activated by the source,
        // zeta's head is still the earlier session's row.
        let inside = earlier_len + 1;
        let expected = fork_instruction_answer(&source_body, inside)
            .expect_err("a boundary inside the suffix leaves a copied head");

        let request = rewind_of(&source, inside, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a boundary inside a re-activation suffix refuses the preflight");
        match refused {
            SessionError::ForkInstructionRefused(refusal) => assert_eq!(
                refusal, expected,
                "the preflight answers what the fork answers for the same prefix"
            ),
            other => panic!("expected the fork instruction refusal, got {other:?}"),
        }
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn preflight_refuses_a_pending_input_boundary() {
        let booted = boot();
        let mut session = answered_turns();
        session.push(user("a question the member has not answered"));
        let source = durable(&booted, session).await;

        // The source's end is an input its next turn answers.
        let request = rewind_of(&source, 5, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a pending input boundary refuses the preflight");
        assert!(
            matches!(&refused, SessionError::Busy { id } if id == &source),
            "the busy answer, naming the source: {refused:?}"
        );
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;

        // An earlier, answered boundary of the same source is the caller's
        // own choice and is not refused.
        booted
            .service
            .preflight_member_rewind_fork(&rewind_of(&source, 4, MemberRewindSource::Current))
            .await
            .expect("an answered boundary before the pending input preflights");
    }

    #[tokio::test]
    async fn preflight_refuses_a_split_tool_group() {
        let booted = boot();
        let mut session = Session::new();
        session.push(user("look it up"));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string())
                    .expect("tool args"),
                meta: None,
            }],
            StopReason::ToolUse,
        )));
        session.push(Message::ToolResults {
            results: vec![ToolResult::new(
                "call-1".to_string(),
                "the file".to_string(),
                false,
            )],
            created_at: meerkat_core::types::message_timestamp_now(),
        });
        session.push(assistant("here it is"));
        bind(&mut session, &member());
        let split = 2;
        let expected = session
            .fork_at_complete_boundary(split)
            .expect_err("the boundary separates the tool call from its results")
            .into_session_error();
        let source = durable(&booted, session).await;

        let request = rewind_of(&source, split, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a split tool group refuses the preflight");
        assert!(
            matches!(&refused, SessionError::Agent(AgentError::ConfigError(_))),
            "the durable fork's boundary refusal: {refused:?}"
        );
        assert_eq!(
            refused.to_string(),
            expected.to_string(),
            "the preflight answers what the fork answers for the same boundary"
        );
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn preflight_refuses_a_prefix_blob_missing_from_the_store() {
        let booted = boot();
        let missing = meerkat_core::blob::content_blob_id("image/png", "iVBORw0KGgo=");
        let mut session = Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: missing.clone(),
                },
            },
        ])));
        session.push(assistant("a picture"));
        bind(&mut session, &member());
        let source = durable(&booted, session).await;
        assert!(
            matches!(
                meerkat_core::BlobStore::get(booted.blob_store.as_ref(), &missing).await,
                Err(meerkat_core::BlobStoreError::NotFound(_))
            ),
            "the prefix names a blob the store does not hold"
        );

        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a prefix blob missing from the store refuses the preflight");
        assert!(
            matches!(&refused, SessionError::Store(_)),
            "the fork's store error: {refused:?}"
        );
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn preflight_refuses_a_source_without_canonical_metadata() {
        let booted = boot();
        let mut session = Session::new();
        session.push(user("first question"));
        session.push(assistant("first answer"));
        let source = durable(&booted, session).await;
        assert!(
            body(&booted, &source)
                .await
                .try_session_metadata()
                .expect("metadata decodes")
                .is_none(),
            "the source holds no canonical session metadata"
        );

        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        // A session without canonical metadata is nobody's member session.
        // Its existence does not show: both doors answer it as a session
        // that does not exist, as the lineage read does.
        for (door, refused) in [
            (
                "the preflight",
                booted
                    .service
                    .preflight_member_rewind_fork(&request)
                    .await
                    .map(|_| ()),
            ),
            (
                "the fork",
                booted
                    .service
                    .fork_member_rewind_child(&request)
                    .await
                    .map(|_| ()),
            ),
        ] {
            assert!(
                matches!(&refused, Err(SessionError::NotFound { id }) if id == &source),
                "{door} answers a source without canonical metadata not found: {refused:?}"
            );
        }
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn preflight_refuses_a_comms_name_that_is_not_the_member_bindings() {
        let booted = boot();
        let mut session = answered_turns();
        let mut metadata = member_metadata(&member());
        metadata.comms_name = Some(
            another_member()
                .comms_name()
                .expect("comms name")
                .to_string(),
        );
        session
            .set_session_metadata(metadata)
            .expect("member metadata");
        let source = durable(&booted, session).await;

        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("a comms name that is not the binding's refuses the preflight");
        // A corrupt binding, not a caller's error: an internal error.
        assert!(
            matches!(&refused, SessionError::Agent(AgentError::InternalError(_))),
            "{refused:?}"
        );
        let text = refused.to_string();
        assert!(
            !text.contains("archivist") && !text.contains("researcher"),
            "the refusal names no member: {text}"
        );
        assert_nothing_written(&booted, &request, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn a_refused_preflight_writes_no_session_row() {
        let booted = boot();
        let mut session = answered_turns();
        session.push(user("a question the member has not answered"));
        let source = durable(&booted, session).await;
        let sessions_before = stored_sessions(&booted).await;
        let source_row_before = stored_row(&booted, &source).await;
        assert!(source_row_before.is_some(), "the source has a stored row");

        // Refused: the pending end.
        let refused = rewind_of(&source, 5, MemberRewindSource::Current);
        booted
            .service
            .preflight_member_rewind_fork(&refused)
            .await
            .expect_err("the pending end refuses the preflight");
        assert_nothing_written(&booted, &refused, &sessions_before, &source_row_before).await;

        // Accepted: an answered boundary. It returns the evidence and
        // writes no session row either.
        let accepted = rewind_of(&source, 2, MemberRewindSource::Current);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&accepted)
            .await
            .expect("an answered boundary preflights");
        assert_eq!(evidence.prefix_message_count, 2);
        let metadata = member_metadata(&member());
        assert_eq!(
            evidence.tool_access_policy,
            metadata.tooling.tool_access_policy
        );
        assert_eq!(evidence.auth_binding, metadata.auth_binding);
        assert_eq!(evidence.realm_id, metadata.realm_id);
        assert_nothing_written(&booted, &accepted, &sessions_before, &source_row_before).await;
    }

    #[tokio::test]
    async fn the_preflight_digest_is_the_committed_childs_prefix_digest_for_an_inline_image() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        // A document written with its image inline, as a store written
        // before images were moved out of session rows holds it: the fork
        // moves the image into the blob store in the child it commits.
        let mut inline = body(&booted, &source).await;
        inline.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "iVBORw0KGgo=".to_string(),
                },
            },
        ])));
        inline.push(assistant("a picture"));
        booted
            .runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&source),
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(
                        serde_json::to_vec(&inline).expect("serialize the inline document"),
                    ),
                },
            )
            .await
            .expect("store the inline document");
        let source_body = body(&booted, &source).await;
        let message_count = source_body.messages().len();
        assert_eq!(message_count, 6);
        assert!(
            matches!(
                &source_body.messages()[4],
                Message::User(user) if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image { data: ImageData::Inline { .. }, .. }]
                )
            ),
            "the stored source holds its image inline"
        );

        let request = rewind_of(&source, message_count, MemberRewindSource::Current);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("the inline image prefix preflights");

        // Today's planned fork commits the child.
        booted
            .service
            .fork_durable_session_with_planned_identity(
                &source,
                Some(message_count),
                request.planned_child_session_id.clone(),
                None,
                Some(fork_target(
                    member(),
                    ForkInstructionPolicy::ReactivateInherited,
                )),
                ForkInstructionPolicies {
                    inherited: ForkInstructionPolicy::ReactivateInherited,
                    declared: ForkInstructionPolicy::ReactivateInherited,
                },
            )
            .await
            .expect("fork the planned child");
        let child = body(&booted, &request.planned_child_session_id).await;
        assert!(
            matches!(
                &child.messages()[4],
                Message::User(user) if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image { data: ImageData::Blob { .. }, .. }]
                )
            ),
            "the committed child names its image by blob"
        );
        assert_eq!(
            evidence.prefix_digest,
            child
                .transcript_prefix_digest(message_count)
                .expect("digest the child prefix"),
            "the preflight digest is the committed child's prefix digest"
        );

        // The digest does not tell an inline image from its blob, so the
        // case above holds for either. A prefix whose image is held under
        // the blob store's own address is where the fork's normalization
        // changes the digest: the child names the image by its content
        // address, and the evidence must be over that prefix.
        let blobs = Arc::new(StoreAddressedBlobStore::default());
        let store_address = BlobId::new(format!("sha256:{}", "ab".repeat(32)));
        blobs.hold_under_own_address(&store_address, "image/png", "iVBORw0KGgo=");
        let booted = boot_over(blobs, EventLog::None);
        let mut session = Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: store_address.clone(),
                },
            },
        ])));
        session.push(assistant("a picture"));
        bind(&mut session, &member());
        let source = durable(&booted, session).await;
        let source_digest = body(&booted, &source)
            .await
            .transcript_prefix_digest(2)
            .expect("digest the source prefix");

        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("the store-addressed image prefix preflights");
        assert_ne!(
            evidence.prefix_digest, source_digest,
            "the evidence is over the normalized prefix, not the source's stored one"
        );
        let observed = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("fork the planned child");
        assert_eq!(evidence.verify_child(&member(), &observed), Ok(()));
        let child = body(&booted, &request.planned_child_session_id).await;
        assert!(
            matches!(
                &child.messages()[0],
                Message::User(user) if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image { data: ImageData::Blob { blob_id }, .. }]
                        if blob_id == &meerkat_core::blob::content_blob_id("image/png", "iVBORw0KGgo=")
                )
            ),
            "the committed child names its image by content address"
        );
        assert_eq!(
            evidence.prefix_digest,
            child
                .transcript_prefix_digest(2)
                .expect("digest the child prefix"),
            "the preflight digest is the committed child's prefix digest"
        );
    }

    #[tokio::test]
    async fn a_planned_member_fork_is_born_bound_to_the_member() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        let source_before = body(&booted, &source).await;
        let source_row_before = stored_row(&booted, &source).await;

        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("preflight");
        let observed = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the planned member fork");
        assert_eq!(
            observed.session_id, request.planned_child_session_id,
            "the observation names the session it was read from"
        );
        assert_eq!(
            observed.member_binding,
            Some(member()),
            "the child is born bound to the member"
        );
        assert!(!observed.archived);
        assert_eq!(
            evidence.verify_child(&member(), &observed),
            Ok(()),
            "the committed child is the child the preflight described"
        );

        let child = body(&booted, &request.planned_child_session_id).await;
        assert_eq!(child.id(), &request.planned_child_session_id);
        assert_eq!(
            child.messages(),
            &source_before.messages()[..2],
            "the child holds the exact prefix"
        );
        let child_metadata = child
            .try_session_metadata()
            .expect("child metadata decodes")
            .expect("child metadata present");
        assert_eq!(child_metadata.mob_member_binding, Some(member()));
        assert_eq!(
            child_metadata.comms_name,
            Some(member().comms_name().expect("comms name").to_string()),
            "the child carries the member's comms name"
        );
        let source_metadata = member_metadata(&member());
        assert_eq!(
            child_metadata.tooling.tool_access_policy, source_metadata.tooling.tool_access_policy,
            "the child keeps the source's effective tool access policy"
        );

        // The observation is the same read at any later time.
        assert_eq!(
            booted
                .service
                .observe_member_rewind_child(&request.planned_child_session_id)
                .await
                .expect("observe the child"),
            Some(observed)
        );
        assert_eq!(
            booted
                .service
                .observe_member_rewind_child(&SessionId::new())
                .await
                .expect("observe an absent child"),
            None,
            "a planned child that was never forked is no such child"
        );
        assert_eq!(
            stored_row(&booted, &source).await,
            source_row_before,
            "the fork leaves its source as it was"
        );

        // A source that activated an instruction key before the boundary:
        // the child re-activates it after the prefix, and its evidence is
        // still the prefix's.
        let keyed = session_with_activated_keys(&["primary"]);
        let boundary = 3;
        assert!(
            matches!(&keyed.messages()[2], Message::System(_)),
            "the key's activation row lies before the boundary"
        );
        let keyed_source = durable(&booted, keyed.clone()).await;
        let keyed_request = MemberRewindForkRequest {
            source_session_id: keyed_source,
            message_count: boundary,
            planned_child_session_id: SessionId::new(),
            member_binding: member(),
            source: MemberRewindSource::Current,
        };
        // An activation row is no answered turn's end, but it is no pending
        // input either, and the boundary is before the source's end.
        let keyed_evidence = booted
            .service
            .preflight_member_rewind_fork(&keyed_request)
            .await
            .expect("preflight the keyed source");
        let keyed_observed = booted
            .service
            .fork_member_rewind_child(&keyed_request)
            .await
            .expect("fork the keyed source");
        assert_eq!(keyed_observed.evidence.prefix_message_count, boundary);
        assert_eq!(
            keyed_evidence.verify_child(&member(), &keyed_observed),
            Ok(())
        );
        let keyed_child = body(&booted, &keyed_request.planned_child_session_id).await;
        assert_eq!(
            keyed_child.messages().len(),
            boundary + 1,
            "the child ends with the key's re-activation row"
        );
        assert_eq!(
            &keyed_child.messages()[..boundary],
            &keyed.messages()[..boundary]
        );
    }

    #[tokio::test]
    async fn a_retried_planned_member_fork_reaches_the_same_child() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        let request = rewind_of(&source, 2, MemberRewindSource::Current);

        let first = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the first fork");
        let sessions_after_first = stored_sessions(&booted).await;
        let child_row_after_first = stored_row(&booted, &request.planned_child_session_id).await;
        assert!(
            child_row_after_first.is_some(),
            "the child has a stored row"
        );

        let second = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the retried fork");
        assert_eq!(second, first, "the retry observes the same child");
        assert_eq!(
            stored_sessions(&booted).await,
            sessions_after_first,
            "the retry adds no session"
        );
        assert_eq!(
            stored_row(&booted, &request.planned_child_session_id).await,
            child_row_after_first,
            "the retry leaves the child's row as it was"
        );

        // Once the child exists the source is not read again: a retry after
        // the source was archived still reaches the child.
        archive(&booted, &source).await;
        let third = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the retry after the source was archived");
        assert_eq!(third, first);
    }

    /// The undelivered fork seed a session's stored row holds, if any. A
    /// session's first run delivers a fork notice from this seed and from
    /// nothing else.
    fn fork_seed(session: &Session) -> Option<meerkat_core::fork_notice::ForkSeed> {
        session
            .metadata()
            .get(meerkat_core::fork_notice::SESSION_FORK_SEED_KEY)
            .map(|seed| serde_json::from_value(seed.clone()).expect("the fork seed decodes"))
    }

    #[tokio::test]
    async fn a_member_rewind_child_holds_no_fork_seed_and_an_ordinary_fork_still_does() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;

        // The member continues on the child: it was not forked, so its
        // first run has no fork notice to deliver.
        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the planned member fork");
        let child = body(&booted, &request.planned_child_session_id).await;
        assert_eq!(
            fork_seed(&child),
            None,
            "a member rewind child holds no fork seed"
        );
        assert!(
            !meerkat_core::fork_notice::holds_fork_notice_for(child.messages(), child.id()),
            "a member rewind child holds no fork notice"
        );

        // A source that still holds an undelivered fork seed of its own (an
        // ordinary fork child of the member that never ran): the rewind
        // child does not inherit that seed either.
        let seeded = booted
            .service
            .fork_durable_session(
                &source,
                Some(4),
                None,
                Some(fork_target(
                    member(),
                    ForkInstructionPolicy::ReactivateInherited,
                )),
            )
            .await
            .expect("an ordinary fork bound to the member")
            .session_id;
        assert!(
            fork_seed(&body(&booted, &seeded).await).is_some(),
            "the ordinary fork child holds its undelivered seed"
        );
        let request = rewind_of(&seeded, 2, MemberRewindSource::Current);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("the preflight accepts the seeded source");
        let observed = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the member rewind fork of the seeded source");
        assert_eq!(evidence.verify_child(&member(), &observed), Ok(()));
        assert_eq!(
            fork_seed(&body(&booted, &request.planned_child_session_id).await),
            None,
            "a member rewind child holds no fork seed, its source's included"
        );
        assert!(
            fork_seed(&body(&booted, &seeded).await).is_some(),
            "the source keeps its own seed"
        );

        // An ordinary durable fork of the same source still records its
        // seed, minted or planned child alike.
        let minted = booted
            .service
            .fork_durable_session(
                &source,
                Some(2),
                None,
                Some(fork_target(
                    another_member(),
                    ForkInstructionPolicy::ReactivateInherited,
                )),
            )
            .await
            .expect("an ordinary durable fork")
            .session_id;
        let planned = SessionId::new();
        booted
            .service
            .fork_durable_session_with_planned_identity(
                &source,
                Some(2),
                planned.clone(),
                None,
                None,
                ForkInstructionPolicies {
                    inherited: ForkInstructionPolicy::ReactivateInherited,
                    declared: ForkInstructionPolicy::ReactivateInherited,
                },
            )
            .await
            .expect("an ordinary planned durable fork");
        for ordinary in [minted, planned] {
            let seed = fork_seed(&body(&booted, &ordinary).await)
                .expect("an ordinary durable fork child holds its fork seed");
            assert_eq!(
                seed.fork,
                meerkat_core::fork_notice::ForkNoticeKind::Transcript
            );
            assert_eq!(seed.source.session_id, Some(source.clone()));
            assert_eq!(
                seed.source.member,
                Some(member()),
                "the seed names the source the child was forked from"
            );
        }
    }

    #[test]
    fn verification_names_the_first_field_that_differs() {
        let meerkat_core::SessionMetadata {
            tooling,
            auth_binding,
            realm_id,
            ..
        } = member_metadata(&member());
        let recorded = MemberRewindForkEvidence {
            prefix_message_count: 4,
            prefix_digest: "digest-of-the-prefix".to_string(),
            tool_access_policy: tooling.tool_access_policy,
            auth_binding,
            realm_id,
        };
        let matching = MemberRewindChildObservation {
            session_id: SessionId::new(),
            evidence: recorded.clone(),
            member_binding: Some(member()),
            archived: false,
        };
        assert_eq!(recorded.verify_child(&member(), &matching), Ok(()));

        let differs = |change: fn(&mut MemberRewindChildObservation)| {
            let mut observed = matching.clone();
            change(&mut observed);
            recorded
                .verify_child(&member(), &observed)
                .expect_err("a changed field fails the verification")
                .field
        };
        assert_eq!(
            differs(|observed| observed.evidence.prefix_message_count = 3),
            MemberRewindEvidenceField::PrefixMessageCount
        );
        assert_eq!(
            differs(|observed| observed.evidence.prefix_digest = "another-digest".to_string()),
            MemberRewindEvidenceField::PrefixDigest
        );
        assert_eq!(
            differs(|observed| observed.evidence.tool_access_policy = None),
            MemberRewindEvidenceField::ToolAccessPolicy
        );
        assert_eq!(
            differs(|observed| observed.evidence.auth_binding = None),
            MemberRewindEvidenceField::AuthBinding
        );
        assert_eq!(
            differs(|observed| observed.evidence.realm_id = None),
            MemberRewindEvidenceField::RealmId
        );
        assert_eq!(
            differs(|observed| observed.member_binding = Some(another_member())),
            MemberRewindEvidenceField::MemberBinding
        );
        assert_eq!(
            differs(|observed| observed.member_binding = None),
            MemberRewindEvidenceField::MemberBinding,
            "an unbound child is not the member's"
        );
        // Several differences: the first in the comparison order is named.
        assert_eq!(
            differs(|observed| {
                observed.member_binding = None;
                observed.evidence.realm_id = None;
                observed.evidence.prefix_digest = "another-digest".to_string();
            }),
            MemberRewindEvidenceField::PrefixDigest
        );
        // Whether the child is archived is not evidence.
        let mut archived = matching.clone();
        archived.archived = true;
        assert_eq!(recorded.verify_child(&member(), &archived), Ok(()));
    }

    /// A member session as a release that kept transcript rewrites in the
    /// event log left it: the stored row holds the rewritten transcript and
    /// no rewrite graph, and the event log holds the rewrite's full record.
    /// A read of it materializes the rewrite projection from replay.
    async fn source_with_a_projection_owed_from_replay(booted: &Booted) -> SessionId {
        // The rewrite is committed by a service without an event log, so
        // the log holds nothing of it yet.
        let writer = service_without_event_log(booted);
        let source = durable_in(&writer, answered_turns()).await;
        let before = writer
            .load_durable_session_body(&source)
            .await
            .expect("load the source")
            .expect("the source is durable");
        writer
            .rewrite_session_transcript(
                &source,
                meerkat_core::SessionTranscriptRewriteRequest {
                    selection: meerkat_core::TranscriptRewriteSelection::MessageRange {
                        start: 1,
                        end: 2,
                    },
                    replacement: vec![assistant("a shorter first answer")],
                    reason: meerkat_core::TranscriptRewriteReason::new("compaction"),
                    actor: Some("member-rewind-row".to_string()),
                    expected_parent_revision: Some(
                        before
                            .transcript_revision()
                            .expect("the revision before the rewrite"),
                    ),
                    running_behavior: meerkat_core::TranscriptEditRunningBehavior::Reject,
                },
            )
            .await
            .expect("commit a transcript rewrite");
        let rewritten = writer
            .load_durable_session_body(&source)
            .await
            .expect("load the rewritten source")
            .expect("the rewritten source is durable");
        let commit = rewritten
            .validated_transcript_history_state()
            .expect("the rewrite graph validates")
            .expect("the rewritten source holds its rewrite graph")
            .last_commit()
            .expect("the rewrite graph holds the commit")
            .clone();

        // The row as the older release wrote it: the rewritten transcript,
        // without the rewrite graph.
        let row = stored_row(booted, &source)
            .await
            .expect("the rewritten source has a stored row");
        let mut document: serde_json::Value =
            serde_json::from_slice(&row).expect("the stored row is a JSON document");
        let metadata = document
            .get_mut("metadata")
            .and_then(serde_json::Value::as_object_mut)
            .expect("the stored row holds session metadata");
        assert!(
            metadata
                .remove(meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY)
                .is_some(),
            "the rewritten row recorded its rewrite graph"
        );
        metadata.remove(meerkat_core::SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
        booted
            .runtime_store
            .commit_session_snapshot(
                &LogicalRuntimeId::for_session(&source),
                SerializedSessionSnapshot {
                    session_snapshot: Arc::new(
                        serde_json::to_vec(&document).expect("serialize the older row"),
                    ),
                },
            )
            .await
            .expect("store the older row");
        // The rewrite as that release logged it: the full record.
        let record = meerkat_core::TranscriptRewriteRecord {
            parent_body: meerkat_core::TranscriptRevisionBody {
                revision: commit.parent_revision.clone(),
                parent_revision: None,
                messages: before.messages().to_vec(),
                created_at: before.updated_at(),
            },
            revision_body: meerkat_core::TranscriptRevisionBody {
                revision: commit.revision.clone(),
                parent_revision: Some(commit.parent_revision.clone()),
                messages: rewritten.messages().to_vec(),
                created_at: rewritten.updated_at(),
            },
            commit,
            digest_format: 0,
        };
        booted
            .event_store
            .as_ref()
            .expect("the booted service keeps an event log")
            .append(
                &source,
                &[meerkat_core::AgentEvent::TranscriptRewriteCommitted {
                    session_id: source.clone(),
                    record,
                }],
            )
            .await
            .expect("log the rewrite record");
        source
    }

    #[tokio::test]
    async fn a_lineage_fork_read_of_an_archived_source_leaves_its_stored_rows_unchanged() {
        let booted = boot_with_event_projection();
        // The row's premise, shown on a twin: today's fork read of such a
        // source writes the projection it materialized back onto it.
        let twin = source_with_a_projection_owed_from_replay(&booted).await;
        let twin_row_before = stored_row(&booted, &twin).await;
        booted
            .service
            .fork_durable_session(
                &twin,
                Some(2),
                None,
                Some(fork_target(
                    member(),
                    ForkInstructionPolicy::ReactivateInherited,
                )),
            )
            .await
            .expect("fork the twin with today's fork read");
        assert_ne!(
            stored_row(&booted, &twin).await,
            twin_row_before,
            "today's fork read persists the replayed projection onto its source"
        );

        let source = source_with_a_projection_owed_from_replay(&booted).await;
        archive(&booted, &source).await;
        let source_row_before = stored_row(&booted, &source).await;
        assert!(source_row_before.is_some());
        let source_events_before = booted
            .service
            .event_log_read_from(&source, 1)
            .await
            .expect("read the source's event log");

        let request = rewind_of(&source, 2, MemberRewindSource::ArchivedLineage);
        let evidence = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("the lineage preflight reads the archived source");
        assert_eq!(
            stored_row(&booted, &source).await,
            source_row_before,
            "the preflight writes nothing to the archived source"
        );
        let observed = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the lineage fork reads the archived source");
        assert_eq!(evidence.verify_child(&member(), &observed), Ok(()));
        assert_eq!(
            body(&booted, &request.planned_child_session_id)
                .await
                .messages()
                .len(),
            2,
            "the child holds the prefix of the archived source"
        );
        assert_eq!(
            stored_row(&booted, &source).await,
            source_row_before,
            "the fork read writes nothing to the archived source"
        );
        assert_eq!(
            booted
                .service
                .event_log_read_from(&source, 1)
                .await
                .expect("read the source's event log again")
                .map(|events| events.len()),
            source_events_before.map(|events| events.len()),
            "the fork read appends nothing to the archived source's event log"
        );
    }

    /// Whether `refused` is the typed cause of an archived lineage source
    /// whose transcript rewrite has no audit receipt: the kind, the source
    /// the caller named, and the count of commits without a receipt.
    fn audit_incomplete(refused: &SessionError, source: &SessionId) -> bool {
        matches!(
            refused,
            SessionError::FailedWithData { data, .. }
                if data.get("kind").and_then(serde_json::Value::as_str)
                    == Some("member_rewind_source_audit_incomplete")
                    && data.get("source_session_id").and_then(serde_json::Value::as_str)
                        == Some(source.to_string().as_str())
                    && data.get("missing_audit_receipts").and_then(serde_json::Value::as_u64)
                        == Some(1)
                    && data.as_object().is_some_and(|data| data.len() == 3)
        )
    }

    /// The answer of a seam call made while a turn is held open: the seam
    /// answers at once and never waits for the turn.
    async fn answer_at_once<T>(
        call: impl std::future::Future<Output = Result<T, SessionError>>,
    ) -> Result<T, SessionError> {
        tokio::time::timeout(std::time::Duration::from_secs(5), call)
            .await
            .expect("the seam answers while a turn runs; it does not wait for the turn")
    }

    #[tokio::test]
    async fn a_lineage_fork_read_never_repairs_an_archived_sources_rewrite_audit() {
        let booted = boot_over(Arc::new(MemoryBlobStore::new()), EventLog::File);
        // A source whose committed transcript rewrite has no audit row in
        // the event log.
        let (writer, source) = rewritten_without_audit(&booted, Some(&member())).await;
        writer
            .archive_with_machine_protocol(
                &source,
                MachineSessionArchiveProtocol::from_machine(&booted.machine),
            )
            .await
            .expect("archive the source");
        assert_eq!(
            event_log_end(&booted, &source).await,
            0,
            "the event log holds no audit row of the rewrite"
        );
        let source_row_before = stored_row(&booted, &source).await;

        // The read of an archived source verifies the audit and stops with
        // a typed cause: it appends nothing to the archived session's event
        // log. Nothing can repair an archived session's audit, so this
        // source is refused for good.
        let request = rewind_of(&source, 2, MemberRewindSource::ArchivedLineage);
        let refused = booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect_err("an archived source with an unaudited rewrite refuses the preflight");
        assert!(
            audit_incomplete(&refused, &source),
            "the preflight answers the typed audit cause: {refused:?}"
        );
        let refused = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect_err("an archived source with an unaudited rewrite refuses the fork");
        assert!(
            audit_incomplete(&refused, &source),
            "the fork answers the typed audit cause: {refused:?}"
        );
        assert_eq!(
            event_log_end(&booted, &source).await,
            0,
            "nothing was appended to the archived session's event log"
        );
        assert_eq!(stored_row(&booted, &source).await, source_row_before);
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "a refused lineage fork leaves no child"
        );
    }

    #[tokio::test]
    async fn the_read_of_a_current_source_repairs_a_missing_rewrite_audit_receipt() {
        let booted = boot_over(Arc::new(MemoryBlobStore::new()), EventLog::File);
        let (_writer, source) = rewritten_without_audit(&booted, Some(&member())).await;
        assert_eq!(
            event_log_end(&booted, &source).await,
            0,
            "the event log holds no audit row of the rewrite"
        );
        let source_row_before = stored_row(&booted, &source).await;

        // A current source is the member's live session: its missing
        // receipt is appended, as any guarded read of that session appends
        // it, and the preflight goes on. No session row is written.
        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        booted
            .service
            .preflight_member_rewind_fork(&request)
            .await
            .expect("a current source with an unaudited rewrite preflights");
        assert_eq!(
            event_log_end(&booted, &source).await,
            1,
            "the read appended the missing audit receipt"
        );
        assert_eq!(
            stored_row(&booted, &source).await,
            source_row_before,
            "the repair writes no session row"
        );
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "the preflight leaves no child"
        );
    }

    #[tokio::test]
    async fn another_members_session_is_not_found_before_its_audit_is_repaired() {
        // Another member's session whose rewrite has no audit receipt: the
        // answer is not found, and its event log gains nothing.
        let booted = boot_over(Arc::new(MemoryBlobStore::new()), EventLog::File);
        let (_writer, unaudited) = rewritten_without_audit(&booted, Some(&another_member())).await;
        let request = rewind_of(&unaudited, 2, MemberRewindSource::Current);
        for (door, refused) in [
            (
                "the preflight",
                booted
                    .service
                    .preflight_member_rewind_fork(&request)
                    .await
                    .map(|_| ()),
            ),
            (
                "the fork",
                booted
                    .service
                    .fork_member_rewind_child(&request)
                    .await
                    .map(|_| ()),
            ),
        ] {
            assert!(
                matches!(&refused, Err(SessionError::NotFound { id }) if id == &unaudited),
                "{door} answers another member's session not found: {refused:?}"
            );
        }
        assert_eq!(
            event_log_end(&booted, &unaudited).await,
            0,
            "another member's event log gains no audit receipt"
        );
    }

    #[tokio::test]
    async fn another_members_running_session_is_not_found_and_never_busy() {
        // The member check comes before the busy answer: a caller learns
        // nothing about a session that is not its member's, not even that
        // it is running a turn.
        let booted = boot();
        let mut session = answered_turns();
        bind(&mut session, &another_member());
        let running = durable(&booted, session).await;
        let request = rewind_of(&running, 2, MemberRewindSource::Current);
        let (preflight, fork) = while_a_turn_runs(&booted, &running, async {
            (
                answer_at_once(booted.service.preflight_member_rewind_fork(&request))
                    .await
                    .map(|_| ()),
                answer_at_once(booted.service.fork_member_rewind_child(&request))
                    .await
                    .map(|_| ()),
            )
        })
        .await;
        for (door, refused) in [("the preflight", preflight), ("the fork", fork)] {
            assert!(
                matches!(&refused, Err(SessionError::NotFound { id }) if id == &running),
                "{door} answers another member's running session not found: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_session_of_no_member_is_not_found_before_the_busy_answer_and_the_audit_step() {
        // A session that names no member is nobody's member session: it is
        // answered as a session that does not exist, before the busy
        // answer and before its audit is looked at.
        let booted = boot_over(Arc::new(MemoryBlobStore::new()), EventLog::File);
        let (_writer, unaudited) = rewritten_without_audit(&booted, None).await;
        let request = rewind_of(&unaudited, 2, MemberRewindSource::Current);
        for (door, refused) in [
            (
                "the preflight",
                booted
                    .service
                    .preflight_member_rewind_fork(&request)
                    .await
                    .map(|_| ()),
            ),
            (
                "the fork",
                booted
                    .service
                    .fork_member_rewind_child(&request)
                    .await
                    .map(|_| ()),
            ),
        ] {
            assert!(
                matches!(&refused, Err(SessionError::NotFound { id }) if id == &unaudited),
                "{door} answers a session of no member not found: {refused:?}"
            );
        }
        assert_eq!(
            event_log_end(&booted, &unaudited).await,
            0,
            "the event log of a session of no member gains no audit receipt"
        );

        let booted = boot();
        let mut bound_to_nobody = answered_turns();
        bind_to_no_member(&mut bound_to_nobody);
        // Without canonical session metadata a session is nobody's member
        // session too: the same answer, also while it runs a turn.
        let mut without_metadata = Session::new();
        without_metadata.push(user("first question"));
        without_metadata.push(assistant("first answer"));
        for (whose, session) in [
            ("a running session of no member", bound_to_nobody),
            (
                "a running session without canonical metadata",
                without_metadata,
            ),
        ] {
            let running = durable(&booted, session).await;
            let request = rewind_of(&running, 2, MemberRewindSource::Current);
            let (preflight, fork) = while_a_turn_runs(&booted, &running, async {
                (
                    answer_at_once(booted.service.preflight_member_rewind_fork(&request))
                        .await
                        .map(|_| ()),
                    answer_at_once(booted.service.fork_member_rewind_child(&request))
                        .await
                        .map(|_| ()),
                )
            })
            .await;
            for (door, refused) in [("the preflight", preflight), ("the fork", fork)] {
                assert!(
                    matches!(&refused, Err(SessionError::NotFound { id }) if id == &running),
                    "{door} answers {whose} not found: {refused:?}"
                );
            }
        }
    }

    /// A session of `binding` (of no member when `None`) whose last message
    /// is an instruction activation of its own: the state of a session
    /// between an instruction delivery and its next turn, and of a fresh
    /// member session before its first turn.
    fn ends_with_its_own_activation(binding: Option<&MobMemberBinding>) -> Session {
        let mut session = Session::new();
        session.push(user("first question"));
        session.push(assistant("first answer"));
        session
            .activate_instruction(activation("primary", "policy primary", "act-primary"))
            .expect("activate the key");
        match binding {
            Some(binding) => bind(&mut session, binding),
            None => bind_to_no_member(&mut session),
        }
        session
    }

    #[tokio::test]
    async fn a_planned_child_id_that_names_another_session_is_not_found_and_left_as_it_is() {
        // The fork returns an existing session under the planned child id
        // only when it is bound to the request's member. Any other session
        // under that id is answered as a session that does not exist, and
        // nothing is forked over it. Whose the session is is decided before
        // anything of it is read, so the answer is the same in whatever
        // state that session is: also between an instruction delivery and
        // its next turn, when its transcript ends with activations of its
        // own.
        let mut others = answered_turns();
        bind(&mut others, &another_member());
        let mut nobodys = answered_turns();
        bind_to_no_member(&mut nobodys);
        for (whose, session) in [
            ("another member's session", others),
            ("a session of no member", nobodys),
            (
                "another member's session that ends with its own activation",
                ends_with_its_own_activation(Some(&another_member())),
            ),
            (
                "a session of no member that ends with its own activation",
                ends_with_its_own_activation(None),
            ),
        ] {
            let booted = boot();
            let source = durable(&booted, answered_turns()).await;
            let taken = durable(&booted, session).await;
            let row_before = stored_row(&booted, &taken).await;
            let mut request = rewind_of(&source, 2, MemberRewindSource::Current);
            request.planned_child_session_id = taken.clone();
            let refused = booted
                .service
                .fork_member_rewind_child(&request)
                .await
                .map(|_| ());
            assert!(
                matches!(&refused, Err(SessionError::NotFound { id }) if id == &taken),
                "a planned child id that names {whose} is answered not found: {refused:?}"
            );
            assert_eq!(
                stored_row(&booted, &taken).await,
                row_before,
                "{whose} is left as it is"
            );
        }
    }

    #[tokio::test]
    async fn preflight_and_fork_refuse_a_source_that_is_running_a_turn() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let (preflight, fork) = while_a_turn_runs(&booted, &source, async {
            (
                answer_at_once(booted.service.preflight_member_rewind_fork(&request))
                    .await
                    .map(|_| ()),
                answer_at_once(booted.service.fork_member_rewind_child(&request))
                    .await
                    .map(|_| ()),
            )
        })
        .await;
        for (door, refused) in [("the preflight", preflight), ("the fork", fork)] {
            assert!(
                matches!(&refused, Err(SessionError::Busy { id }) if id == &source),
                "{door} answers the busy refusal for a running source: {refused:?}"
            );
        }
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "a refused fork leaves no child"
        );
    }

    #[tokio::test]
    async fn a_planned_member_fork_refuses_a_pending_input_boundary() {
        let booted = boot();
        let mut session = answered_turns();
        session.push(user("a question the member has not answered"));
        let source = durable(&booted, session).await;
        let source_row_before = stored_row(&booted, &source).await;

        // The fork decides the pending end itself, under its own guard on
        // the source: a fork that is driven again without a fresh preflight
        // is refused as the preflight refuses it.
        let request = rewind_of(&source, 5, MemberRewindSource::Current);
        let refused = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect_err("a pending input boundary refuses the fork");
        assert!(
            matches!(&refused, SessionError::Busy { id } if id == &source),
            "the busy answer, naming the source: {refused:?}"
        );
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "a refused fork leaves no child"
        );
        assert_eq!(stored_row(&booted, &source).await, source_row_before);
    }

    #[tokio::test]
    async fn the_observation_says_when_the_child_is_archived() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let committed = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the planned member fork");
        assert!(!committed.archived, "a committed child is not archived");

        // A child that was abandoned is archived by its owner; the
        // observation says so, and its evidence is unchanged.
        let child = &request.planned_child_session_id;
        booted
            .service
            .archive_with_machine_protocol(
                child,
                MachineSessionArchiveProtocol::from_machine(&booted.machine),
            )
            .await
            .expect("archive the child");
        let observed = booted
            .service
            .observe_member_rewind_child(child)
            .await
            .expect("observe the archived child")
            .expect("an archived child is still observed");
        assert!(
            observed.archived,
            "the observation says the child is archived"
        );
        assert_eq!(observed.evidence, committed.evidence);
        assert_eq!(observed.member_binding, committed.member_binding);
        // A fork driven again reaches the archived child and forks nothing.
        let again = booted
            .service
            .fork_member_rewind_child(&request)
            .await
            .expect("the fork observes the archived child");
        assert_eq!(again, observed);
    }

    /// A member session whose prefix holds an inline image, so that a fork
    /// of it reads the blob store: a [`GatedBlobStore`] can hold the fork
    /// there.
    fn session_with_an_inline_image() -> Session {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "iVBORw0KGgo=".to_string(),
                },
            },
        ])));
        session.push(assistant("a picture"));
        bind(&mut session, &member());
        session
    }

    /// Two calls that wait for each other, bounded: when one of them
    /// answers something else than the row expects, the other can wait for
    /// a point that is never reached, and the row must fail, not hang.
    async fn both_within_the_rows_bound<A, B>(
        first: impl std::future::Future<Output = A>,
        second: impl std::future::Future<Output = B>,
    ) -> (A, B) {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::join!(first, second)
        })
        .await
        .expect("both calls of the row finish")
    }

    #[tokio::test]
    async fn two_concurrent_planned_member_forks_commit_one_child() {
        let blobs = Arc::new(GatedBlobStore::default());
        let booted = boot_over(blobs.clone(), EventLog::None);
        let source = durable(&booted, session_with_an_inline_image()).await;

        // The seam bounds the second call's wait for the source's guard
        // (one second, then the busy answer). The gate is released as soon
        // as the second call waits, so the first call has that whole bound
        // to commit. A host that still stalls it past the bound gives no
        // verdict on the second call's answer: the scenario runs again
        // with a new planned child.
        for _attempt in 0..5 {
            let request = rewind_of(&source, 2, MemberRewindSource::Current);
            let child_id = request.planned_child_session_id.clone();

            // The first call is held inside its fork: it holds the
            // source's guard and has committed nothing. The second call
            // starts meanwhile and finds no child yet.
            blobs.armed.store(true, std::sync::atomic::Ordering::SeqCst);
            let first = async {
                let observed = booted
                    .service
                    .fork_member_rewind_child(&request)
                    .await
                    .expect("the first fork commits the child");
                let row = stored_row(&booted, &child_id).await;
                (observed, row)
            };
            let second = async {
                blobs.reached.notified().await;
                assert!(
                    stored_row(&booted, &child_id).await.is_none(),
                    "premise: the first fork is held before it committed the child"
                );
                let call = booted.service.fork_member_rewind_child(&request);
                tokio::pin!(call);
                // Started while the first call holds the source's guard:
                // it must wait for that guard. A call that answers here
                // forked beside the first.
                for _ in 0..16 {
                    tokio::select! {
                        biased;
                        early = &mut call => panic!(
                            "the second call answered while the first held the source's guard: {early:?}"
                        ),
                        () = tokio::task::yield_now() => {}
                    }
                }
                blobs.release.notify_one();
                call.await
            };
            let ((first_observed, row_after_first), second_result) =
                both_within_the_rows_bound(first, second).await;
            assert!(
                row_after_first.is_some(),
                "the first fork committed the child"
            );
            assert_eq!(
                stored_row(&booted, &child_id).await,
                row_after_first,
                "the second call left the first call's child as it was"
            );
            match second_result {
                Ok(second_observed) => {
                    assert_eq!(
                        second_observed, first_observed,
                        "the second call observes the first call's child"
                    );
                    return;
                }
                // The wait ran out of the seam's bound: one child still,
                // and no verdict on what the second call observes.
                Err(SessionError::Busy { id }) if id == source => {}
                Err(other) => panic!("the second call reaches the child: {other:?}"),
            }
        }
        panic!("the second call ran out of the seam's wait bound in every attempt");
    }

    #[tokio::test]
    async fn a_concurrent_planned_member_fork_never_truncates_a_child_that_gained_a_turn() {
        let blobs = Arc::new(GatedBlobStore::default());
        let booted = boot_over(blobs.clone(), EventLog::None);
        let source = durable(&booted, session_with_an_inline_image()).await;
        let request = rewind_of(&source, 2, MemberRewindSource::Current);
        let child_id = request.planned_child_session_id.clone();

        // As above, but the second call is held back until the member has
        // run a turn on the child the first call committed: it was started
        // before the child existed and reaches the fork after the turn.
        blobs.armed.store(true, std::sync::atomic::Ordering::SeqCst);
        let turn_committed = tokio::sync::Notify::new();
        let first = async {
            booted
                .service
                .fork_member_rewind_child(&request)
                .await
                .expect("the first fork commits the child");
            let mut ran = body(&booted, &child_id).await;
            ran.push(user("third question"));
            ran.push(assistant("third answer"));
            booted
                .runtime_store
                .commit_session_snapshot(
                    &LogicalRuntimeId::for_session(&child_id),
                    SerializedSessionSnapshot {
                        session_snapshot: Arc::new(
                            serde_json::to_vec(&ran).expect("serialize the extended child"),
                        ),
                    },
                )
                .await
                .expect("store the member's turn on the child");
            turn_committed.notify_one();
        };
        let second = async {
            blobs.reached.notified().await;
            let call = booted.service.fork_member_rewind_child(&request);
            tokio::pin!(call);
            // Started while the first call holds the source's guard: it
            // runs until it waits for that guard.
            for _ in 0..16 {
                tokio::select! {
                    biased;
                    early = &mut call => panic!(
                        "the second call answered while the first held the source's guard: {early:?}"
                    ),
                    () = tokio::task::yield_now() => {}
                }
            }
            blobs.release.notify_one();
            // Not polled again until the member's turn is on the child.
            turn_committed.notified().await;
            call.await
        };
        let ((), second_result) = both_within_the_rows_bound(first, second).await;
        assert_eq!(
            body(&booted, &child_id).await.messages().len(),
            4,
            "the child keeps the member's turn"
        );
        // The second call observed the child as it is now; it forked
        // nothing over it. Its count is the child's length, which the
        // caller's verification refuses.
        let observed = second_result.expect("the second call observes the child");
        assert_eq!(observed.evidence.prefix_message_count, 4);
    }

    #[tokio::test]
    async fn the_generic_fork_and_read_still_refuse_an_archived_source() {
        let booted = boot();
        let source = durable(&booted, answered_turns()).await;
        archive(&booted, &source).await;
        let not_found = |result: Result<(), SessionError>, door: &str| {
            assert!(
                matches!(&result, Err(SessionError::NotFound { id }) if id == &source),
                "{door} refuses the archived source as not found: {result:?}"
            );
        };

        not_found(
            booted.service.read(&source).await.map(|_| ()),
            "the session read",
        );
        not_found(
            booted
                .service
                .fork_durable_session(
                    &source,
                    Some(2),
                    None,
                    Some(fork_target(
                        member(),
                        ForkInstructionPolicy::ReactivateInherited,
                    )),
                )
                .await
                .map(|_| ()),
            "the durable fork",
        );
        not_found(
            booted
                .service
                .fork_durable_session_with_planned_identity(
                    &source,
                    Some(2),
                    SessionId::new(),
                    None,
                    Some(fork_target(
                        member(),
                        ForkInstructionPolicy::ReactivateInherited,
                    )),
                    ForkInstructionPolicies {
                        inherited: ForkInstructionPolicy::ReactivateInherited,
                        declared: ForkInstructionPolicy::ReactivateInherited,
                    },
                )
                .await
                .map(|_| ()),
            "the planned durable fork",
        );
        // The member rewind fork of a current source is a generic read too:
        // only a lineage source may be archived.
        not_found(
            booted
                .service
                .preflight_member_rewind_fork(&rewind_of(&source, 2, MemberRewindSource::Current))
                .await
                .map(|_| ()),
            "the preflight of a current source",
        );
        not_found(
            booted
                .service
                .fork_member_rewind_child(&rewind_of(&source, 2, MemberRewindSource::Current))
                .await
                .map(|_| ()),
            "the member rewind fork of a current source",
        );
        // The lineage reads do read it.
        booted
            .service
            .preflight_member_rewind_fork(&rewind_of(
                &source,
                2,
                MemberRewindSource::ArchivedLineage,
            ))
            .await
            .expect("the lineage preflight reads the archived source");
    }

    #[tokio::test]
    async fn a_lineage_read_refuses_a_session_bound_to_another_member() {
        let booted = boot();
        let mut others = answered_turns();
        bind(&mut others, &another_member());
        let others_session = durable(&booted, others).await;
        let own_session = durable(&booted, answered_turns()).await;
        let own_body = body(&booted, &own_session).await;
        archive(&booted, &others_session).await;
        archive(&booted, &own_session).await;

        // The member's own archived session reads.
        let read = booted
            .service
            .read_member_rewind_lineage_session(&own_session, &member(), 2)
            .await
            .expect("the member's own lineage session reads");
        assert_eq!(read.message_count, 4);
        assert_eq!(
            read.prefix_digest,
            Some(
                own_body
                    .transcript_prefix_digest(2)
                    .expect("digest the prefix")
            )
        );
        assert_eq!(
            booted
                .service
                .read_member_rewind_lineage_session(&own_session, &member(), 4)
                .await
                .expect("a count at the end reads")
                .prefix_digest,
            Some(
                own_body
                    .transcript_prefix_digest(4)
                    .expect("digest the whole session")
            ),
            "the whole session is a prefix of itself"
        );
        assert_eq!(
            booted
                .service
                .read_member_rewind_lineage_session(&own_session, &member(), 5)
                .await
                .expect("a count past the end reads")
                .prefix_digest,
            None,
            "a session shorter than the asked prefix has no such digest"
        );

        // Another member's session does not, through any lineage door, and
        // the answer names the asked session only.
        let undisclosed = |refused: SessionError, door: &str| {
            assert!(
                matches!(&refused, SessionError::NotFound { id } if id == &others_session),
                "{door} answers the other member's session not found: {refused:?}"
            );
            let text = refused.to_string();
            assert!(
                !text.contains("archivist") && !text.contains("researcher"),
                "{door} names no member: {text}"
            );
        };
        undisclosed(
            booted
                .service
                .read_member_rewind_lineage_session(&others_session, &member(), 2)
                .await
                .expect_err("the lineage read refuses another member's session"),
            "the lineage session read",
        );
        let request = rewind_of(&others_session, 2, MemberRewindSource::ArchivedLineage);
        undisclosed(
            booted
                .service
                .preflight_member_rewind_fork(&request)
                .await
                .expect_err("the lineage preflight refuses another member's session"),
            "the lineage preflight",
        );
        undisclosed(
            booted
                .service
                .fork_member_rewind_child(&request)
                .await
                .expect_err("the lineage fork refuses another member's session"),
            "the lineage fork",
        );
        assert!(
            booted
                .service
                .load_durable_session_body(&request.planned_child_session_id)
                .await
                .expect("look for the planned child")
                .is_none(),
            "a refused lineage fork leaves no child"
        );
    }
}
