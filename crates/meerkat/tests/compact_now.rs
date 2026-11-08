//! A host's compaction request on the runtime-backed facade path
//! (`MeerkatMachine::request_compaction`), beyond the idle-session
//! acceptance row in `compaction_ownership_gaps.rs`: a request made while a
//! turn runs compacts at that turn's next model request, two requests before
//! a check compact once, and a check that consumes a request with nothing to
//! compact (before a fresh session's first model request, or inside a run
//! with nothing to fold) compacts nothing and publishes
//! `requested_compaction_skipped` with its reason.

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
    use meerkat_core::lifecycle::RunId;
    use meerkat_core::{AgentEvent, Message, RequestedCompactionSkipReason, SessionBuildOptions};
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{CompactionRequestReceipt, Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    const COMPACTION_MARKER: &str = "CONTEXT COMPACTION";
    /// A prompt that makes the scripted model ask for one `memory_search`
    /// call before it answers.
    const TOOL_PROMPT: &str = "look something up";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RequestKind {
        Compaction,
        Ordinary,
    }

    /// The host request the scripted model makes while it serves a model
    /// request of a running turn: the machine, the session, and the receipt
    /// the request got.
    struct InTurnRequest {
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
        receipt: Arc<Mutex<Option<Result<CompactionRequestReceipt, String>>>>,
    }

    /// Scripted model. A compaction request answers with the user texts it
    /// summarizes; the first request of a turn whose last user text is
    /// [`TOOL_PROMPT`] asks for one `memory_search` call (and, when armed,
    /// first makes the host's compaction request while that turn runs);
    /// every other request answers `ok`. Records the kind of every request.
    struct ScriptedClient {
        served: Mutex<Vec<RequestKind>>,
        in_turn_request: Mutex<Option<InTurnRequest>>,
    }

    impl ScriptedClient {
        fn new() -> Self {
            Self {
                served: Mutex::new(Vec::new()),
                in_turn_request: Mutex::new(None),
            }
        }

        fn served(&self) -> Vec<RequestKind> {
            self.served.lock().expect("served requests").clone()
        }

        fn compactions(&self) -> usize {
            self.served()
                .iter()
                .filter(|kind| **kind == RequestKind::Compaction)
                .count()
        }
    }

    /// Whether `request` is a compaction's summary request: one of its user
    /// rows carries the compaction prompt.
    fn is_compaction_request(request: &LlmRequest) -> bool {
        request.messages.iter().rev().any(|message| {
            matches!(message, Message::User(user) if user.text_content().contains(COMPACTION_MARKER))
        })
    }

    /// Whether `request` opens a turn on [`TOOL_PROMPT`]: its newest user or
    /// tool-result row is that prompt (a request after the tool's result
    /// ends with the result instead).
    fn opens_the_tool_turn(request: &LlmRequest) -> bool {
        request
            .messages
            .iter()
            .rev()
            .find(|message| matches!(message, Message::User(_) | Message::ToolResults { .. }))
            .is_some_and(|message| {
                matches!(message, Message::User(user) if user.text_content() == TOOL_PROMPT)
            })
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedClient {
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
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let mut in_turn_request = None;
            let events = if compaction {
                let summary = request
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(user) if !user.text_content().contains(COMPACTION_MARKER) => {
                            Some(user.text_content())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![
                    LlmEvent::TextDelta {
                        delta: summary,
                        meta: None,
                    },
                    usage,
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                    },
                ]
            } else if opens_the_tool_turn(request) {
                in_turn_request = self.in_turn_request.lock().expect("in-turn request").take();
                vec![
                    LlmEvent::ToolCallComplete {
                        id: "toolu_compact_now_lookup".to_string(),
                        name: "memory_search".to_string(),
                        args: serde_json::json!({ "query": "anything" }),
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
            // The host's request lands while this model request of the
            // running turn is in flight: after the turn's check for it ran.
            let request_first = futures::stream::once(async move {
                if let Some(InTurnRequest {
                    adapter,
                    session_id,
                    receipt,
                }) = in_turn_request
                {
                    let answer = adapter
                        .request_compaction(&session_id)
                        .await
                        .map_err(|error| error.to_string());
                    *receipt.lock().expect("in-turn receipt") = Some(answer);
                }
            })
            .filter_map(|()| async { None::<Result<LlmEvent, LlmError>> });
            Box::pin(request_first.chain(futures::stream::iter(events.into_iter().map(Ok))))
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
        /// the memory tools on (so the agent has `memory_search`) and the
        /// default compaction policy.
        async fn open(root: &Path, client: Arc<ScriptedClient>) -> Self {
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                root,
                "compact-now-realm",
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

        async fn create(&self) -> SessionId {
            let session = Session::new();
            let id = session.id().clone();
            let request = CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.4".to_string(),
                prompt: meerkat_core::ContentInput::Text(String::new()),
                system_prompt: meerkat::SystemPromptOverride::Set("compact now".to_string()),
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(SessionBuildOptions {
                    override_memory: meerkat_core::ToolCategoryOverride::Enable,
                    ..SessionBuildOptions::default()
                }),
                labels: None,
            };
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
            id
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

        async fn compaction_summaries(&self, id: &SessionId) -> usize {
            self.service
                .load_authoritative_session(id)
                .await
                .expect("load the authoritative session")
                .expect("the session exists")
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

        fn compactions_started(&self) -> usize {
            self.count(|event| matches!(event, AgentEvent::CompactionStarted { .. }))
        }

        fn compactions_completed(&self) -> usize {
            self.count(|event| matches!(event, AgentEvent::CompactionCompleted { .. }))
        }

        /// Every `requested_compaction_skipped`, in delivery order: the run
        /// whose check consumed the request, and the reason.
        fn requested_compactions_skipped(&self) -> Vec<(RunId, RequestedCompactionSkipReason)> {
            self.events
                .lock()
                .expect("event log")
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::RequestedCompactionSkipped { run_id, reason } => {
                        Some((run_id.clone(), *reason))
                    }
                    _ => None,
                })
                .collect()
        }

        /// The run id of every `run_started`, in delivery order.
        fn started_runs(&self) -> Vec<RunId> {
            self.events
                .lock()
                .expect("event log")
                .iter()
                .filter_map(|event| match event {
                    AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
                    _ => None,
                })
                .collect()
        }

        /// Give the forwarding task a moment to drain what the actor already
        /// published.
        async fn settle(&self) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// A request made while a turn runs (its model request in flight, after
    /// that request's check) leaves the turn running, and the turn's next
    /// model request, after its tool result, is preceded by the compaction:
    /// the history before the turn is folded while the turn keeps its own
    /// request and newest round.
    #[tokio::test]
    async fn a_request_made_during_a_turn_compacts_before_that_turns_next_model_request() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(ScriptedClient::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let id = host.create().await;
        host.run_prompt(&id, "first turn fact").await;
        host.run_prompt(&id, "second turn").await;
        assert_eq!(
            client.compactions(),
            0,
            "setup: the default policy does not compact"
        );
        let receipt = Arc::new(Mutex::new(None));
        *client.in_turn_request.lock().expect("in-turn request") = Some(InTurnRequest {
            adapter: Arc::clone(&host.adapter),
            session_id: id.clone(),
            receipt: Arc::clone(&receipt),
        });
        let served_before = client.served().len();

        host.run_prompt(&id, TOOL_PROMPT).await;
        assert_eq!(
            receipt.lock().expect("in-turn receipt").clone(),
            Some(Ok(CompactionRequestReceipt::Requested)),
            "the running turn's registration records the request"
        );
        assert_eq!(
            client.served()[served_before..],
            [
                RequestKind::Ordinary,
                RequestKind::Compaction,
                RequestKind::Ordinary
            ],
            "the turn's model request that follows its tool result is preceded by the compaction"
        );
        assert_eq!(host.compaction_summaries(&id).await, 1);

        host.run_prompt(&id, "turn after the in-turn compaction")
            .await;
        assert_eq!(
            client.compactions(),
            1,
            "the request was consumed once (served: {:?})",
            client.served()
        );
        host.stand_down().await;
    }

    /// Two requests before a check give one compaction: the second is
    /// already requested, and the run after the compacting one does not
    /// compact again. The compaction publishes the ordinary events and no
    /// `requested_compaction_skipped`.
    #[tokio::test]
    async fn two_requests_before_a_check_compact_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(ScriptedClient::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let id = host.create().await;
        let log = host.record_events(&id).await;
        host.run_prompt(&id, "first turn fact").await;
        host.run_prompt(&id, "second turn").await;

        assert_eq!(
            host.adapter.request_compaction(&id).await.unwrap(),
            CompactionRequestReceipt::Requested
        );
        assert_eq!(
            host.adapter.request_compaction(&id).await.unwrap(),
            CompactionRequestReceipt::AlreadyRequested
        );
        let served_before = client.served().len();
        host.run_prompt(&id, "turn after two requests").await;
        host.run_prompt(&id, "the turn after").await;
        assert_eq!(
            client.served()[served_before..],
            [
                RequestKind::Compaction,
                RequestKind::Ordinary,
                RequestKind::Ordinary
            ],
            "one compaction, before the first model request after the requests"
        );
        assert_eq!(host.compaction_summaries(&id).await, 1);
        log.settle().await;
        assert_eq!(
            (log.compactions_started(), log.compactions_completed()),
            (1, 1)
        );
        assert_eq!(
            log.requested_compactions_skipped(),
            Vec::new(),
            "a check that compacts at the request publishes no skip"
        );
        host.stand_down().await;
    }

    /// The session's first-ever model request has nothing to compact: a
    /// request made before it is consumed by its check, which compacts
    /// nothing and publishes `requested_compaction_skipped` for that run
    /// with the reason `first_model_request`. The next turn does not compact
    /// either.
    #[tokio::test]
    async fn a_request_before_a_fresh_sessions_first_model_request_compacts_nothing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(ScriptedClient::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let id = host.create().await;
        let log = host.record_events(&id).await;

        assert_eq!(
            host.adapter.request_compaction(&id).await.unwrap(),
            CompactionRequestReceipt::Requested
        );
        host.run_prompt(&id, "first turn of a fresh session").await;
        host.run_prompt(&id, "second turn").await;
        assert_eq!(
            client.served(),
            [RequestKind::Ordinary, RequestKind::Ordinary],
            "neither the first model request nor the next compacts"
        );
        assert_eq!(host.compaction_summaries(&id).await, 0);
        log.settle().await;
        let runs = log.started_runs();
        assert_eq!(runs.len(), 2, "setup: both runs are observed");
        assert_eq!(
            log.requested_compactions_skipped(),
            vec![(
                runs[0].clone(),
                RequestedCompactionSkipReason::FirstModelRequest
            )],
            "the first run's check says that it consumed the request and compacted nothing"
        );
        assert_eq!(log.compactions_started(), 0);
        assert_eq!(
            host.adapter.request_compaction(&id).await.unwrap(),
            CompactionRequestReceipt::Requested,
            "the first turn's check consumed the request"
        );
        host.stand_down().await;
    }

    /// A request made during a fresh session's first turn is consumed by
    /// the turn's next check, after its tool result. The open run keeps its
    /// request and its newest round, and nothing else is there to fold, so
    /// the check compacts nothing and publishes `requested_compaction_skipped`
    /// with the reason `nothing_to_fold`.
    #[tokio::test]
    async fn a_request_during_a_fresh_sessions_first_turn_has_nothing_to_fold() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(ScriptedClient::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let id = host.create().await;
        let log = host.record_events(&id).await;
        let receipt = Arc::new(Mutex::new(None));
        *client.in_turn_request.lock().expect("in-turn request") = Some(InTurnRequest {
            adapter: Arc::clone(&host.adapter),
            session_id: id.clone(),
            receipt: Arc::clone(&receipt),
        });

        host.run_prompt(&id, TOOL_PROMPT).await;
        assert_eq!(
            receipt.lock().expect("in-turn receipt").clone(),
            Some(Ok(CompactionRequestReceipt::Requested)),
            "the running turn's registration records the request"
        );
        assert_eq!(
            client.served(),
            [RequestKind::Ordinary, RequestKind::Ordinary],
            "the turn's model request after its tool result is not preceded by a compaction"
        );
        assert_eq!(host.compaction_summaries(&id).await, 0);
        log.settle().await;
        let runs = log.started_runs();
        assert_eq!(runs.len(), 1, "setup: the run is observed");
        assert_eq!(
            log.requested_compactions_skipped(),
            vec![(
                runs[0].clone(),
                RequestedCompactionSkipReason::NothingToFold
            )],
            "the turn's next check says that it consumed the request and compacted nothing"
        );
        assert_eq!(log.compactions_started(), 0);
        assert_eq!(
            host.adapter.request_compaction(&id).await.unwrap(),
            CompactionRequestReceipt::Requested,
            "the turn's check consumed the request"
        );
        host.stand_down().await;
    }
}
