//! The surfaces' live-session staleness judgement while a durable run's
//! provisional tail stands ahead of the committed boundary, on a SQLite
//! (HeadCanonical) realm.
//!
//! A run commits its tool rounds, so from its first tool round until its
//! boundary commit the store keeps the round as the run's provisional tail,
//! and a strict committed read refuses the boundary. The judgement reads its
//! facts body-free, so the tail never fails it:
//!
//! - the live actor that runs the open run continues that tail: it is not
//!   stale, and the judgement answers at once instead of waiting out the run;
//! - a host that restarts over the tail with no live actor finds the
//!   committed session present: stale, so the surface takes its cold path,
//!   where recovery decides the tail.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

#[cfg(all(
    feature = "session-store",
    any(target_os = "linux", target_os = "macos")
))]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;

    use futures::StreamExt as _;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::session_runtime::recovery::RecoveryContext;
    use meerkat::session_runtime::runtime_state::{LiveStalenessPosition, RuntimeStateOps};
    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session, SessionId,
    };
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::{Message, SessionBuildOptions};
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    use tokio::sync::Semaphore;
    use tokio::time::Duration;

    /// Scripted model: a fresh request calls the shell tool once; the
    /// request after the tool round waits until released, then answers.
    struct HeldAfterOneToolRound {
        requests: AtomicUsize,
        entered: Semaphore,
        release: Semaphore,
    }

    impl HeldAfterOneToolRound {
        fn new() -> Self {
            Self {
                requests: AtomicUsize::new(0),
                entered: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for HeldAfterOneToolRound {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let usage = LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            };
            let after_tool_round = request
                .messages
                .iter()
                .any(|message| matches!(message, Message::ToolResults { .. }));
            if !after_tool_round {
                return Box::pin(futures::stream::iter(
                    vec![
                        LlmEvent::ToolCallComplete {
                            id: "call-1".to_string(),
                            name: "shell".to_string(),
                            args: serde_json::json!({ "command": "true", "timeout_secs": 30 }),
                            meta: None,
                        },
                        usage,
                        LlmEvent::Done {
                            outcome: LlmDoneOutcome::success(meerkat_core::StopReason::ToolUse),
                        },
                    ]
                    .into_iter()
                    .map(Ok),
                ));
            }
            let events = vec![
                LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                },
                usage,
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
                },
            ];
            Box::pin(
                futures::stream::once(async move {
                    self.entered.add_permits(1);
                    self.release
                        .acquire()
                        .await
                        .expect("release the model")
                        .forget();
                    events
                })
                .flat_map(|events: Vec<LlmEvent>| {
                    futures::stream::iter(events.into_iter().map(Ok::<LlmEvent, LlmError>))
                }),
            )
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    struct Host {
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        staged_sessions: Arc<meerkat::StagedSessionRegistry>,
        staged_capacity_admissions: meerkat::session_runtime::admission::StagedCapacityAdmissions,
    }

    impl Host {
        async fn open(root: &Path, client: Arc<HeldAfterOneToolRound>) -> Self {
            let (_manifest, persistence) = meerkat::open_realm_persistence_in(
                root,
                "staleness-realm",
                Some(meerkat_store::RealmBackend::Sqlite),
                Some(meerkat_store::RealmOrigin::Explicit),
            )
            .await
            .expect("open realm persistence");
            let project = root.join("project");
            std::fs::create_dir_all(&project).expect("project dir");
            let factory = AgentFactory::new(root.join("sessions"))
                .runtime_root(root.join("realm"))
                .project_root(&project)
                .shell(true);
            let mut config = Config::default();
            config.shell.program = "sh".to_string();
            config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
            let mut builder = FactoryAgentBuilder::new(factory, config);
            builder.default_llm_client = Some(client);
            let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
                .expect("compose the runtime-backed service");
            Self {
                service: Arc::new(service),
                adapter,
                staged_sessions: Arc::new(meerkat::StagedSessionRegistry::new()),
                staged_capacity_admissions: Arc::new(std::sync::Mutex::new(HashMap::new())),
            }
        }

        async fn live_session_is_stale(
            &self,
            id: &SessionId,
        ) -> Result<bool, meerkat::SessionError> {
            let ops = RuntimeStateOps {
                service: &self.service,
                staged_sessions: &self.staged_sessions,
                staged_capacity_admissions: &self.staged_capacity_admissions,
                runtime_adapter: &self.adapter,
            };
            let recovery = RecoveryContext {
                service: &self.service,
                runtime_adapter: &self.adapter,
                realm_id: None,
                instance_id: None,
                backend: None,
                default_llm_client: None,
                agent_llm_client_decorator: None,
                external_tools: None,
                config_runtime: None,
            };
            ops.live_session_is_stale(id, &recovery, LiveStalenessPosition::OutsideTurnBoundary)
                .await
        }

        async fn tail_run(&self, id: &SessionId) -> Option<meerkat_core::lifecycle::RunId> {
            self.service
                .runtime_store()
                .load_head_canonical_provisional_tail(
                    &meerkat_runtime::LogicalRuntimeId::for_session(id),
                )
                .await
                .expect("read the provisional tail")
                .map(|tail| tail.run_id().clone())
        }
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set("staleness".to_string()),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    async fn materialize(host: &Host, session: Session) {
        let service = Arc::clone(&host.service);
        let adapter = Arc::clone(&host.adapter);
        Box::pin(materialize_session(
            &host.service,
            &host.adapter,
            session,
            create_request(),
            move |session_id| default_persistent_executor(service, adapter, session_id),
        ))
        .await
        .expect("materialize session");
    }

    /// Copy every file under `from` to `to`, as a power cut leaves the realm.
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("create the copied directory");
        for entry in std::fs::read_dir(from).expect("read the directory") {
            let entry = entry.expect("read a directory entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("read the entry type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).expect("copy a file");
            }
        }
    }

    /// A run that committed its tool round and waits on its next model call.
    async fn run_held_after_its_tool_round(
        host: &Host,
        client: &HeldAfterOneToolRound,
    ) -> (SessionId, meerkat_core::lifecycle::RunId) {
        let session = Session::new();
        let id = session.id().clone();
        materialize(host, session).await;
        let (_outcome, _completion) = host
            .adapter
            .accept_input_with_completion(&id, Input::Prompt(PromptInput::new("go", None)))
            .await
            .expect("accept the prompt");
        tokio::time::timeout(Duration::from_secs(60), client.entered.acquire())
            .await
            .expect("the run reaches its model call after the tool round")
            .expect("model gate")
            .forget();
        let run = host
            .tail_run(&id)
            .await
            .expect("the run's round commit stands as its provisional tail");
        (id, run)
    }

    /// The live actor that runs the open run continues its tail: the
    /// judgement answers at once, not stale, instead of waiting for the run.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_actor_of_an_open_run_is_not_stale_under_its_round_commit() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(HeldAfterOneToolRound::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let (id, run) = run_held_after_its_tool_round(&host, &client).await;

        let stale = tokio::time::timeout(Duration::from_secs(5), host.live_session_is_stale(&id))
            .await
            .expect("the judgement answers while the run waits on the model");
        assert!(
            !stale.expect("the judgement reads no committed body"),
            "the actor continuing its open run's tail is not stale"
        );

        client.release.add_permits(1);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        while host.tail_run(&id).await.as_ref() == Some(&run) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the run commits its boundary"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(client.requests.load(Ordering::SeqCst), 2);
    }

    /// A host restarted over the tail, with no live actor yet, finds the
    /// committed session present: stale, so the surface materializes it
    /// through recovery instead of failing on the tail.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_restarted_host_judges_a_session_over_a_round_commit_without_reading_its_body() {
        let temp = tempfile::tempdir().expect("tempdir");
        let client = Arc::new(HeldAfterOneToolRound::new());
        let host = Host::open(temp.path(), Arc::clone(&client)).await;
        let (id, run) = run_held_after_its_tool_round(&host, &client).await;

        // The power goes out while the run waits on the model.
        let after_the_cut = tempfile::tempdir().expect("tempdir");
        copy_tree(temp.path(), after_the_cut.path());
        let restarted =
            Host::open(after_the_cut.path(), Arc::new(HeldAfterOneToolRound::new())).await;
        assert_eq!(
            restarted.tail_run(&id).await,
            Some(run),
            "the round commit stands over the committed boundary"
        );

        let stale = restarted
            .live_session_is_stale(&id)
            .await
            .expect("the judgement reads no committed body");
        assert!(
            stale,
            "a committed session with no live actor sends the surface down its cold path"
        );
        client.release.add_permits(1);
    }
}
