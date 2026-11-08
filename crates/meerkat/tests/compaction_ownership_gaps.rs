//! Compaction ownership gaps on the runtime-backed facade path.
//!
//! The runtime owns the compaction trigger, the transcript rewrite commit and
//! the memory projection. These tests pin three places where a host cannot
//! get what it needs from that ownership today:
//!
//! - the `CompactionCompleted` observation of a compaction whose durable
//!   memory projection is finalized by a later actor never reaches a session
//!   event stream;
//! - a per-build `auto_compact_threshold_override` is not recorded with the
//!   session, so the runtime's recovery lowering rebuilds the session without
//!   it;
//! - there is no typed request to compact a session now.
//!
//! Each gap is an `#[ignore]`d test stating the intended behaviour and, where
//! today's behaviour is observable, a passing `today_` sibling pinning it. The
//! siblings are deleted with the fix and the ignored tests un-ignored.
//!
//! Every "host lifetime" here is reconstructed inside one OS process and ends
//! with `stand_down`, as in `cold_restart_resume_after_compaction.rs`.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

#[cfg(all(
    feature = "session-store",
    feature = "session-compaction",
    feature = "memory-store-session",
    not(target_arch = "wasm32")
))]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use futures::StreamExt as _;
    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session, SessionId,
    };
    use meerkat_client::types::LlmStream;
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::comms::SessionEventCursor;
    use meerkat_core::{
        AgentEvent, MemoryEnumerationRequest, MemorySearchScope, MemoryStore as _, Message,
        SessionBuildOptions,
    };
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    const COMPACTION_MARKER: &str = "CONTEXT COMPACTION";

    fn is_compaction_request(request: &LlmRequest) -> bool {
        request.messages.iter().rev().any(|message| {
            matches!(message, Message::User(user) if user.text_content().contains(COMPACTION_MARKER))
        })
    }

    /// Scripted model: an ordinary request answers `ok`; a compaction request
    /// answers with the user texts it summarizes, so the summary (and the
    /// memory record of the discarded history) carries recognizable text.
    /// Counts the compaction requests it serves and records the kind of
    /// every request in the order it is served.
    struct SummarizingClient {
        compaction_requests: AtomicUsize,
        served: Mutex<Vec<RequestKind>>,
    }

    /// The kind of one model request the scripted client served.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RequestKind {
        Compaction,
        Ordinary,
    }

    impl SummarizingClient {
        fn new() -> Self {
            Self {
                compaction_requests: AtomicUsize::new(0),
                served: Mutex::new(Vec::new()),
            }
        }

        fn compactions(&self) -> usize {
            self.compaction_requests.load(Ordering::SeqCst)
        }

        /// Every request served so far, in order.
        fn served(&self) -> Vec<RequestKind> {
            self.served.lock().expect("served requests").clone()
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for SummarizingClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
            let compaction = is_compaction_request(request);
            self.served
                .lock()
                .expect("served requests")
                .push(if compaction {
                    RequestKind::Compaction
                } else {
                    RequestKind::Ordinary
                });
            let text = if compaction {
                self.compaction_requests.fetch_add(1, Ordering::SeqCst);
                request
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(user) if !user.text_content().contains(COMPACTION_MARKER) => {
                            Some(user.text_content())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                "ok".to_string()
            };
            let events = vec![
                LlmEvent::TextDelta {
                    delta: text,
                    meta: None,
                },
                LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        meerkat_core::Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                },
            ];
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct Host {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
    }

    impl Host {
        /// A runtime-backed service over a SQLite realm under `root`, with
        /// the factory's HNSW memory store (a `DurableStaged` compaction
        /// projection) under `root/sessions/memory`.
        async fn open(root: &Path, client: Arc<SummarizingClient>) -> Self {
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                root,
                "compaction-gaps-realm",
                Some(meerkat_store::RealmBackend::Sqlite),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open realm persistence");
            let factory = AgentFactory::new(root.join("sessions")).memory(true);
            let mut builder = FactoryAgentBuilder::new(factory, Config::default());
            builder.default_llm_client = Some(client);
            let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
            Self {
                service: Arc::new(service),
                adapter,
            }
        }

        /// End this host lifetime: unregister every runtime registration to
        /// terminal and shut the service down, so a same-process successor
        /// can host the session.
        async fn stand_down(self) {
            for registration in self.adapter.current_session_registration_witnesses().await {
                assert!(
                    self.adapter
                        .unregister_session_registration_until_terminal_if_current(&registration)
                        .await
                        .expect("unregister the lifetime's runtime registration"),
                    "the lifetime's current registration is torn down"
                );
            }
            self.service
                .try_shutdown()
                .await
                .expect("shut the lifetime's session service down");
        }

        async fn materialize(&self, session: Session, request: CreateSessionRequest) {
            let service = Arc::clone(&self.service);
            let adapter = Arc::clone(&self.adapter);
            Box::pin(materialize_session(
                &self.service,
                &self.adapter,
                session,
                request,
                move |session_id| default_persistent_executor(service, adapter, session_id),
            ))
            .await
            .expect("materialize session");
        }

        async fn authoritative(&self, id: &SessionId) -> Session {
            self.service
                .load_authoritative_session(id)
                .await
                .expect("load the authoritative session")
                .expect("the session exists")
        }

        /// Collect every event of the session's current actor, from its first
        /// retained envelope on.
        async fn record_events(&self, id: &SessionId) -> EventLog {
            let subscription = self
                .service
                .subscribe_session_events_from(id, SessionEventCursor::Earliest)
                .await
                .expect("subscribe to the session's events");
            let log = EventLog::default();
            let sink = Arc::clone(&log.events);
            let mut stream = subscription.stream;
            tokio::spawn(async move {
                while let Some(envelope) = stream.next().await {
                    sink.lock().expect("event log").push(envelope.payload);
                }
            });
            log
        }

        async fn run_prompt(&self, id: &SessionId, prompt: &str) {
            let (_outcome, handle) = self
                .adapter
                .accept_input_with_completion(id, Input::Prompt(PromptInput::new(prompt, None)))
                .await
                .expect("accept prompt input");
            let outcome = tokio::time::timeout(
                Duration::from_secs(20),
                handle.expect("completion handle").wait(),
            )
            .await
            .expect("the prompt completes in time")
            .expect("the completion waiter resolves");
            assert!(
                matches!(outcome, CompletionOutcome::Completed(_)),
                "unexpected completion outcome: {outcome:?}"
            );
        }
    }

    #[derive(Default, Clone)]
    struct EventLog {
        events: Arc<Mutex<Vec<AgentEvent>>>,
    }

    impl EventLog {
        fn count(&self, matches: impl Fn(&AgentEvent) -> bool) -> usize {
            self.events
                .lock()
                .expect("event log")
                .iter()
                .filter(|event| matches(event))
                .count()
        }

        fn started(&self) -> usize {
            self.count(|event| matches!(event, AgentEvent::CompactionStarted { .. }))
        }

        fn completed(&self) -> usize {
            self.count(|event| matches!(event, AgentEvent::CompactionCompleted { .. }))
        }

        /// Position of the first `CompactionCompleted`, and of every
        /// `RunStarted`, in delivery order.
        fn completion_and_run_starts(&self) -> (Option<usize>, Vec<usize>) {
            let events = self.events.lock().expect("event log");
            let completed = events
                .iter()
                .position(|event| matches!(event, AgentEvent::CompactionCompleted { .. }));
            let run_starts = events
                .iter()
                .enumerate()
                .filter(|(_, event)| matches!(event, AgentEvent::RunStarted { .. }))
                .map(|(index, _)| index)
                .collect();
            (completed, run_starts)
        }

        /// Give the forwarding task a moment to drain what the actor already
        /// published.
        async fn settle(&self) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// The create request a host sends: deferred first turn, the memory tools
    /// on (so the factory wires the HNSW store), and `threshold` as the
    /// per-build `auto_compact_threshold_override`.
    fn create_request(threshold: Option<u64>) -> CreateSessionRequest {
        let build = SessionBuildOptions {
            auto_compact_threshold_override: threshold.and_then(std::num::NonZeroU64::new),
            override_memory: meerkat_core::ToolCategoryOverride::Enable,
            ..SessionBuildOptions::default()
        };
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set(
                "compaction ownership gaps".to_string(),
            ),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(build),
            labels: None,
        }
    }

    /// The create request the runtime's own recovery lowering derives from the
    /// persisted session (`build_recovered_session`, the lowering
    /// `RecoveryContext::recovered_create_request*` runs): nothing the host
    /// supplied at create time beyond what the session records.
    fn recovered_request(session: Session) -> CreateSessionRequest {
        meerkat_core::build_recovered_session(
            session,
            &meerkat_core::SurfaceSessionRecoveryOverrides::default(),
            meerkat_core::SurfaceSessionRecoveryContext::default(),
        )
        .expect("lower the persisted session into a recovered create request")
        .into_deferred_create_request()
    }

    fn compaction_summaries(session: &Session) -> usize {
        session
            .messages()
            .iter()
            .filter(|message| {
                matches!(
                    message,
                    Message::User(user)
                        if user.transcript_role
                            == meerkat_core::types::TranscriptUserRole::CompactionSummary
                )
            })
            .count()
    }

    async fn compaction_memory_records(root: &Path, id: &SessionId, text: &str) -> usize {
        let store = meerkat_memory::HnswMemoryStore::open(root.join("sessions").join("memory"))
            .expect("open the HNSW memory store");
        let page = store
            .enumerate_scoped(
                &MemorySearchScope::for_session(id.clone()),
                MemoryEnumerationRequest {
                    limit: 64,
                    offset: 0,
                    source_overlap: None,
                    indexed_after: None,
                },
            )
            .await
            .expect("enumerate the session's memory");
        page.records
            .iter()
            .filter(|record| {
                record.content.contains(text)
                    && matches!(
                        record.metadata.source,
                        meerkat_core::MemorySource::Compaction { .. }
                    )
            })
            .count()
    }

    /// Run N on a fresh host: the session (threshold 1, HNSW memory) runs a
    /// first turn and then a second whose pre-LLM boundary compacts. Proves
    /// the compaction ran, its summary is committed and the discarded history
    /// is a visible (finalized) compaction memory record.
    async fn compact_during_run_n(
        root: &Path,
        client: &Arc<SummarizingClient>,
    ) -> (Host, SessionId, EventLog) {
        let host = Host::open(root, Arc::clone(client)).await;
        let session = Session::new();
        let id = session.id().clone();
        host.materialize(session, create_request(Some(1))).await;
        let log = host.record_events(&id).await;
        host.run_prompt(&id, "first turn fact").await;
        host.run_prompt(&id, "second turn compacts").await;
        log.settle().await;
        assert_eq!(
            log.started(),
            1,
            "setup: run N must start exactly one compaction"
        );
        assert_eq!(
            client.compactions(),
            1,
            "setup: run N must make the summary call"
        );
        assert_eq!(
            compaction_summaries(&host.authoritative(&id).await),
            1,
            "setup: the compacted transcript must be committed"
        );
        assert_eq!(
            compaction_memory_records(root, &id, "first turn fact").await,
            1,
            "setup: the memory projection of the discarded history must be finalized"
        );
        (host, id, log)
    }

    /// Recover the session on a fresh host through the runtime's recovery
    /// lowering and subscribe to the recovered actor's events, without
    /// running a turn.
    async fn recover(
        root: &Path,
        client: &Arc<SummarizingClient>,
        id: &SessionId,
        durable: Session,
    ) -> (Host, EventLog) {
        let host = Host::open(root, Arc::clone(client)).await;
        host.materialize(durable.clone(), recovered_request(durable))
            .await;
        let log = host.record_events(id).await;
        (host, log)
    }

    /// A compaction's `CompactionCompleted` reaches a session event stream
    /// even when the actor that ran it is replaced before the session's next
    /// run. Run N compacts and the runtime finalizes the memory projection at
    /// run N's boundary; the host then stands down and the session is
    /// recovered on a new host, which runs N+1. The completion is observed
    /// while the session is idle, before N+1 starts: on run N's stream
    /// (before the actor goes) or on the idle recovered actor's stream, ahead
    /// of its first `RunStarted`. An event held back until the next turn does
    /// not pass: the recovered stream is inspected before N+1 is started.
    #[tokio::test]
    async fn compaction_completed_is_observed_when_the_actor_is_recovered_before_the_next_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(SummarizingClient::new());
        let (host, id, run_n_log) = compact_during_run_n(temp.path(), &client).await;
        let durable = host.authoritative(&id).await;
        host.stand_down().await;
        let (host, recovered_log) = recover(temp.path(), &client, &id, durable).await;
        recovered_log.settle().await;
        assert_eq!(
            recovered_log.count(|event| matches!(event, AgentEvent::RunStarted { .. })),
            0,
            "setup: the recovered actor is idle (no run started yet)"
        );
        let observed_while_idle = run_n_log.completed() + recovered_log.completed();
        assert_eq!(
            observed_while_idle,
            1,
            "the compaction's CompactionCompleted must be observed exactly once on a session \
             event stream before the next run starts (run N's actor: {}, idle recovered \
             actor: {})",
            run_n_log.completed(),
            recovered_log.completed()
        );

        host.run_prompt(&id, "turn after recovery").await;
        recovered_log.settle().await;
        let (completed, run_starts) = recovered_log.completion_and_run_starts();
        assert_eq!(run_starts.len(), 1, "setup: the recovered actor ran N+1");
        if let Some(completed) = completed {
            assert!(
                completed < run_starts[0],
                "a completion on the recovered stream precedes N+1's RunStarted \
                 (completion at {completed}, run starts at {run_starts:?})"
            );
        }
        assert_eq!(
            run_n_log.completed() + recovered_log.completed(),
            1,
            "N+1 delivers no second CompactionCompleted"
        );
        host.stand_down().await;
    }

    /// Run run N (compacts at threshold 1), stand down, materialize the
    /// session on a fresh host with `request`, then run `turns` more turns.
    /// Returns the summary calls the recovered host made.
    async fn compactions_after_recovery(
        request: impl FnOnce(Session) -> CreateSessionRequest,
        turns: usize,
    ) -> usize {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(SummarizingClient::new());
        let (host, id, _log) = compact_during_run_n(temp.path(), &client).await;
        let durable = host.authoritative(&id).await;
        host.stand_down().await;

        let recovered_client = Arc::new(SummarizingClient::new());
        let host = Host::open(temp.path(), Arc::clone(&recovered_client)).await;
        host.materialize(durable.clone(), request(durable)).await;
        for turn in 0..turns {
            host.run_prompt(&id, &format!("turn {turn} after recovery"))
                .await;
        }
        host.stand_down().await;
        recovered_client.compactions()
    }

    /// Turns after recovery: enough for threshold 1 to compact again past
    /// `min_turns_between_compactions` (the re-supplied control below proves
    /// it).
    const TURNS_AFTER_RECOVERY: usize = 5;

    /// A per-build `auto_compact_threshold_override` is caller-scoped: it
    /// holds for the build that names it and is not recorded with the
    /// session (the session contracts reference, "Persistent mode"). So the
    /// runtime's recovery lowering leaves it unset, and the recovered session
    /// falls back to the model-aware default and does not compact in the same
    /// turns; the same turns with the threshold supplied again by the host
    /// do compact.
    #[tokio::test]
    async fn recovery_does_not_restore_a_per_build_compaction_threshold() {
        let lowered = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&lowered);
        let compactions = compactions_after_recovery(
            move |session| {
                let request = recovered_request(session);
                *observed.lock().expect("lowered threshold") = Some(
                    request
                        .build
                        .as_ref()
                        .map(|build| build.auto_compact_threshold_override),
                );
                request
            },
            TURNS_AFTER_RECOVERY,
        )
        .await;
        assert_eq!(
            *lowered.lock().expect("lowered threshold"),
            Some(Some(None)),
            "the recovery lowering leaves the per-build auto_compact_threshold_override unset"
        );
        assert_eq!(
            compactions, 0,
            "without the override the recovered session does not compact at the build's threshold"
        );

        let resupplied =
            compactions_after_recovery(|_| create_request(Some(1)), TURNS_AFTER_RECOVERY).await;
        assert!(
            resupplied >= 1,
            "control: the same turns compact when the host supplies threshold 1 again \
             (summary calls: {resupplied})"
        );
    }

    /// The typed compact-now request: `MeerkatMachine::request_compaction`
    /// (RPC `session/compact`) records a one-shot request on the session's
    /// registration, and the session's next compaction check consumes it.
    async fn request_compaction_now(host: &Host, id: &SessionId) -> Result<(), String> {
        match host.adapter.request_compaction(id).await {
            Ok(meerkat_runtime::CompactionRequestReceipt::Requested) => Ok(()),
            other => Err(format!(
                "the idle session's registration did not record the request: {other:?}"
            )),
        }
    }

    /// INTENDED (missing API): a host asks the runtime to compact an idle
    /// session now, without a rebuild and without lowering its threshold.
    /// The test asserts each part of that contract:
    ///
    /// - order: the summary call is served before the next ordinary model
    ///   request (the served-request log after the call reads exactly
    ///   `[Compaction, Ordinary]`), and the summary is committed;
    /// - no rebuild: the runtime registration current before the call is
    ///   still the current one afterwards and has not ended (a retire and
    ///   re-materialize gives the session a new registration incarnation),
    ///   and the event subscription opened before the call observes the
    ///   following turn (a rebuilt actor publishes on a new stream);
    /// - policy kept: the request is a one-shot force, so after it the
    ///   default policy still does not compact over
    ///   `TURNS_AFTER_RECOVERY` further turns, which is enough for
    ///   threshold 1 to compact again past `min_turns_between_compactions`
    ///   (the control in `today_recovered_session_loses_its_compaction_threshold`
    ///   shows it).
    ///
    /// There is no `today_` sibling: before `request_compaction` there was
    /// nothing to call.
    #[tokio::test]
    async fn compact_now_compacts_before_the_next_model_call_without_a_rebuild() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(SummarizingClient::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let session = Session::new();
        let id = session.id().clone();
        // Default policy: the model-aware threshold never fires on these turns.
        host.materialize(session, create_request(None)).await;
        let log = host.record_events(&id).await;
        host.run_prompt(&id, "first turn fact").await;
        host.run_prompt(&id, "second turn").await;
        log.settle().await;
        assert_eq!(
            client.compactions(),
            0,
            "setup: the default policy does not compact these turns"
        );
        let runs_completed =
            |log: &EventLog| log.count(|event| matches!(event, AgentEvent::RunCompleted { .. }));
        assert_eq!(
            runs_completed(&log),
            2,
            "setup: the subscription observes the session's turns"
        );
        let registrations = host.adapter.current_session_registration_witnesses().await;
        assert_eq!(
            registrations.len(),
            1,
            "setup: the session has exactly one current runtime registration"
        );
        let registration = registrations[0].clone();
        assert_eq!(registration.session_id(), &id);
        let served_before = client.served().len();

        request_compaction_now(&host, &id)
            .await
            .expect("request a compaction of the idle session");
        host.run_prompt(&id, "turn after compact now").await;
        log.settle().await;
        assert_eq!(
            client.served()[served_before..],
            [RequestKind::Compaction, RequestKind::Ordinary],
            "the requested compaction's summary call precedes the next ordinary model request"
        );
        assert_eq!(compaction_summaries(&host.authoritative(&id).await), 1);

        assert!(
            !registration.has_ended(),
            "compact-now must not retire the session's runtime registration"
        );
        assert_eq!(
            host.adapter.current_session_registration_witnesses().await,
            vec![registration],
            "compact-now must not re-materialize the session (same registration incarnation)"
        );
        assert_eq!(
            runs_completed(&log),
            3,
            "the subscription opened before compact-now observes the next turn (same actor)"
        );

        for turn in 0..TURNS_AFTER_RECOVERY {
            host.run_prompt(&id, &format!("turn {turn} after compact now"))
                .await;
        }
        assert_eq!(
            client.compactions(),
            1,
            "compact-now is a one-shot force: the default policy still does not compact \
             afterwards (served: {:?})",
            client.served()
        );
        host.stand_down().await;
    }
}
