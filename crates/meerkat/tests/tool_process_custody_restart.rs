//! Abrupt gateway death mid-tool on a plain (non-mob) runtime-backed session.
//!
//! A child "gateway" process creates a session whose scripted model calls the
//! shell tool with a delayed file effect, and is SIGKILLed while the tool
//! runs. The next incarnation (this test process) reopens the realm and
//! materializes the session. Durable process custody must:
//!
//! - kill the orphaned tool before the session serves, so its delayed effect
//!   never happens;
//! - settle the interrupted run's recovered input as
//!   `ToolProcessInterrupted` instead of replaying it (the scripted model
//!   would call the tool again on a replay, repeating the effect);
//! - record a typed `ToolProcessInterrupted` notice in the transcript
//!   without a model call of its own: the model sees it on the next real
//!   turn.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

#[cfg(all(
    feature = "session-store",
    any(target_os = "linux", target_os = "macos")
))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session,
    };
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::tool_process::ToolProcessCessation;
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
    use meerkat_core::{Message, SessionBuildOptions};
    use meerkat_runtime::{Input, MeerkatMachine, PromptInput};
    use tokio::time::Duration;

    const PHASE_ENV: &str = "MEERKAT_CUSTODY_RESTART_PHASE";
    const ROOT_ENV: &str = "MEERKAT_CUSTODY_RESTART_ROOT";
    const CHILD_TEST: &str = "tests::custody_restart_gateway_child";
    const TOOL_CALL_ID: &str = "call-crash";

    /// Scripted model: on a fresh request (no tool results, no
    /// interrupted-run notice) it calls the shell tool; otherwise it answers.
    struct ScriptedShellClient {
        command: String,
        tool_calls: AtomicUsize,
        requests: AtomicUsize,
        requests_seeing_notice: AtomicUsize,
    }

    impl ScriptedShellClient {
        fn new(command: String) -> Self {
            Self {
                command,
                tool_calls: AtomicUsize::new(0),
                requests: AtomicUsize::new(0),
                requests_seeing_notice: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmClient for ScriptedShellClient {
        fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(&'a self, request: &'a LlmRequest) -> meerkat_llm_core::LlmStream<'a> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            let sees_notice = request.messages.iter().any(|message| {
                matches!(
                    message,
                    Message::SystemNotice(notice)
                        if notice.kind == SystemNoticeKind::ToolProcessRecovery
                )
            });
            if sees_notice {
                self.requests_seeing_notice.fetch_add(1, Ordering::SeqCst);
            }
            let answered = sees_notice
                || request
                    .messages
                    .iter()
                    .any(|message| matches!(message, Message::ToolResults { .. }));
            let mut events = Vec::new();
            let stop_reason = if answered {
                events.push(LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                });
                meerkat_core::StopReason::EndTurn
            } else {
                self.tool_calls.fetch_add(1, Ordering::SeqCst);
                events.push(LlmEvent::ToolCallComplete {
                    id: TOOL_CALL_ID.to_string(),
                    name: "shell".to_string(),
                    args: serde_json::json!({
                        "command": self.command,
                        "timeout_secs": 60,
                    }),
                    meta: None,
                });
                meerkat_core::StopReason::ToolUse
            };
            events.push(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::OpenAI,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            });
            events.push(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { stop_reason },
            });
            Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), LlmError> {
            Ok(())
        }
    }

    fn tool_command(root: &Path) -> String {
        format!(
            "echo started > '{}'; sleep 2; echo effect >> '{}'",
            root.join("started.fifo").display(),
            root.join("effect").display()
        )
    }

    async fn build_service(
        root: &Path,
        client: Arc<ScriptedShellClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "custody-realm",
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
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence);
        (Arc::new(service), adapter)
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "gpt-5.4".to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: meerkat::SystemPromptOverride::Set("custody restart".to_string()),
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions::default()),
            labels: None,
        }
    }

    async fn materialize(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session: Session,
    ) {
        let service_for_executor = Arc::clone(service);
        let adapter_for_executor = Arc::clone(adapter);
        Box::pin(materialize_session(
            service,
            adapter,
            session,
            create_request(),
            move |session_id| {
                default_persistent_executor(service_for_executor, adapter_for_executor, session_id)
            },
        ))
        .await
        .expect("materialize session");
    }

    fn interrupted_notice_blocks(session: &Session) -> Vec<SystemNoticeBlock> {
        session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice)
                    if notice.kind == SystemNoticeKind::ToolProcessRecovery =>
                {
                    Some(notice.blocks.clone())
                }
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The gateway role: create the session, submit a prompt whose turn runs
    /// the delayed-effect shell tool, and wait to be killed.
    #[tokio::test]
    async fn custody_restart_gateway_child() {
        let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
            return;
        };
        if std::env::var_os(PHASE_ENV).is_none() {
            return;
        }
        let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
        let (service, adapter) = build_service(&root, client).await;
        let session = Session::new();
        let session_id = session.id().clone();
        materialize(&service, &adapter, session).await;
        std::fs::write(root.join("session-id"), format!("{session_id}\n")).expect("session id");
        let _accepted = adapter
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(PromptInput::new("create the effect file", None)),
            )
            .await
            .expect("accept prompt");
        // The turn now blocks in the tool until the parent kills this process.
        std::future::pending::<()>().await;
    }

    #[tokio::test]
    async fn gateway_sigkill_mid_tool_does_not_replay_the_interrupted_input() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        nix::unistd::mkfifo(&root.join("started.fifo"), nix::sys::stat::Mode::S_IRWXU)
            .expect("mkfifo");

        let mut gateway = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
            .env(PHASE_ENV, "gateway")
            .env(ROOT_ENV, root)
            .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn gateway");

        // Block until the tool itself reports it is running.
        let fifo = root.join("started.fifo");
        let started = tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
        )
        .await;
        let started = match started {
            Ok(joined) => joined.unwrap().unwrap(),
            Err(_) => {
                let _ = gateway.kill();
                let _ = gateway.wait();
                panic!("the gateway's shell tool never started");
            }
        };
        assert_eq!(started.trim(), "started");

        // Abrupt gateway death mid-tool.
        gateway.kill().unwrap();
        gateway.wait().unwrap();

        let session_id = meerkat::SessionId::parse(
            std::fs::read_to_string(root.join("session-id"))
                .expect("session id")
                .trim(),
        )
        .expect("valid session id");

        // Next incarnation: the scripted model would call the tool again if
        // the interrupted input were replayed.
        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = build_service(root, Arc::clone(&client)).await;
        let resume = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load after restart")
            .expect("session survives the gateway");
        materialize(&service, &adapter, resume).await;

        // Settling the interrupted run costs no model call, and the notice
        // is not a turn of its own.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            0,
            "recovery must not start a model turn"
        );

        // The next real turn: its one model call already sees the typed
        // notice, recorded ahead of the new prompt.
        let (_outcome, handle) = adapter
            .accept_input_with_completion(
                &session_id,
                Input::Prompt(PromptInput::new("what happened?", None)),
            )
            .await
            .expect("accept prompt");
        tokio::time::timeout(
            Duration::from_secs(60),
            handle.expect("completion handle").wait(),
        )
        .await
        .expect("turn completes")
        .expect("completion resolves");
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.requests_seeing_notice.load(Ordering::SeqCst),
            1,
            "the next real turn sees the interrupted-run notice"
        );

        let session = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load")
            .expect("session");
        let blocks = interrupted_notice_blocks(&session);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            SystemNoticeBlock::ToolProcessInterrupted {
                tool_call_id,
                cessation,
                ..
            } => {
                assert_eq!(tool_call_id.as_deref(), Some(TOOL_CALL_ID));
                assert!(
                    matches!(cessation, ToolProcessCessation::KilledByRecovery { .. }),
                    "{cessation:?}"
                );
            }
            other => panic!("unexpected notice block {other:?}"),
        }
        let notice_position = session
            .messages()
            .iter()
            .position(|message| {
                matches!(message, Message::SystemNotice(notice)
                    if notice.kind == SystemNoticeKind::ToolProcessRecovery)
            })
            .expect("notice in transcript");
        let prompt_position = session
            .messages()
            .iter()
            .position(|message| {
                matches!(message, Message::User(user)
                    if user.text_content().contains("what happened?"))
            })
            .expect("prompt in transcript");
        assert!(
            notice_position < prompt_position,
            "the notice precedes the next prompt"
        );

        // No replay: the interrupted input never reached the model again.
        assert_eq!(
            client.tool_calls.load(Ordering::SeqCst),
            0,
            "the interrupted input was replayed"
        );
        // The killed tool's delayed effect never happens.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !root.join("effect").exists(),
            "the interrupted tool's effect happened"
        );
    }
}
