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
//!
//! The crash-window tests kill an intermediate recovering incarnation after
//! it abandoned the input but before it told the model, and after it told the
//! model but before it acknowledged the evidence: the final incarnation must
//! still record exactly one notice and never replay the input.

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
    use meerkat_core::tool_process::{InterruptedToolRunDisposition, ToolProcessCessation};
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
    use meerkat_core::{Message, SessionBuildOptions};
    use meerkat_runtime::{
        AcceptOutcome, Input, InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
        MeerkatMachine, PromptInput, SessionServiceRuntimeExt,
    };
    use tokio::time::Duration;

    const PHASE_ENV: &str = "MEERKAT_CUSTODY_RESTART_PHASE";
    const ROOT_ENV: &str = "MEERKAT_CUSTODY_RESTART_ROOT";
    const CHILD_TEST: &str = "tests::custody_restart_child";
    const TOOL_CALL_ID: &str = "call-crash";
    const INTERRUPTED_PROMPT: &str = "create the effect file";
    const NEXT_PROMPT: &str = "what happened?";
    const PHASE_GATEWAY: &str = "gateway";
    /// Recover the session, then wait to be killed before any new input: the
    /// input is abandoned and the notice is owed but not yet recorded.
    const PHASE_RECOVER_THEN_DIE: &str = "recover-then-die";
    /// Recover the session and record the notice on a real turn while the
    /// evidence cannot be acknowledged, then wait to be killed.
    const PHASE_NOTIFY_WITHOUT_ACK: &str = "notify-without-ack";

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

    fn scope_dir(root: &Path, session_id: &meerkat::SessionId) -> PathBuf {
        root.join("realm")
            .join("tool_process_custody")
            .join(session_id.to_string())
    }

    fn record_count(dir: &Path) -> usize {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry.path().extension().and_then(|e| e.to_str()) == Some("json")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Block (no polling) until a child writes a line into `fifo`.
    async fn read_fifo(fifo: PathBuf) -> Option<String> {
        tokio::time::timeout(
            Duration::from_secs(120),
            tokio::task::spawn_blocking(move || std::fs::read_to_string(fifo)),
        )
        .await
        .ok()
        .map(|joined| joined.unwrap().unwrap())
    }

    /// Write a line into a FIFO the parent blocks on, off the runtime thread.
    async fn signal_parent(root: &Path, name: &str, line: &str) {
        let path = root.join(name);
        let line = format!("{line}\n");
        tokio::task::spawn_blocking(move || std::fs::write(path, line))
            .await
            .expect("join signal")
            .expect("signal parent");
    }

    fn spawn_child(root: &Path, phase: &str) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
            .env(PHASE_ENV, phase)
            .env(ROOT_ENV, root)
            .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child incarnation")
    }

    /// Run a child incarnation until it writes `fifo`, then SIGKILL it.
    async fn run_child_until(root: &Path, phase: &str, fifo: &str, expected: &str) {
        let mut child = spawn_child(root, phase);
        let signalled = read_fifo(root.join(fifo)).await;
        child.kill().unwrap();
        child.wait().unwrap();
        match signalled {
            Some(line) => assert_eq!(line.trim(), expected, "child phase {phase}"),
            None => panic!("child phase {phase} never reached its signal"),
        }
    }

    /// The first incarnation dies abruptly while its shell tool runs.
    /// Returns the session and the interrupted prompt's input id.
    async fn crash_gateway_mid_tool(
        root: &Path,
    ) -> (meerkat::SessionId, meerkat_core::lifecycle::InputId) {
        for fifo in ["started.fifo", "phase.fifo"] {
            nix::unistd::mkfifo(&root.join(fifo), nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
        }
        let mut gateway = spawn_child(root, PHASE_GATEWAY);
        // The tool itself reports it is running, and the gateway that it
        // recorded the accepted input.
        let started = read_fifo(root.join("started.fifo")).await;
        let accepted = match started {
            Some(_) => read_fifo(root.join("phase.fifo")).await,
            None => None,
        };
        // Abrupt gateway death mid-tool.
        gateway.kill().unwrap();
        gateway.wait().unwrap();
        assert_eq!(
            started.as_deref().map(str::trim),
            Some("started"),
            "the gateway's shell tool never started"
        );
        assert_eq!(accepted.as_deref().map(str::trim), Some("accepted"));
        let session_id = meerkat::SessionId::parse(
            std::fs::read_to_string(root.join("session-id"))
                .expect("session id")
                .trim(),
        )
        .expect("valid session id");
        let input_id: meerkat_core::lifecycle::InputId = serde_json::from_str(
            &std::fs::read_to_string(root.join("input-id")).expect("input id"),
        )
        .expect("valid input id");
        (session_id, input_id)
    }

    async fn recover(
        root: &Path,
        client: Arc<ScriptedShellClient>,
        session_id: &meerkat::SessionId,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let (service, adapter) = build_service(root, client).await;
        let resume = service
            .load_authoritative_session(session_id)
            .await
            .expect("load after restart")
            .expect("session survives the gateway");
        materialize(&service, &adapter, resume).await;
        (service, adapter)
    }

    async fn prompt_and_wait(
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        text: &str,
    ) {
        let (_outcome, handle) = adapter
            .accept_input_with_completion(session_id, Input::Prompt(PromptInput::new(text, None)))
            .await
            .expect("accept prompt");
        tokio::time::timeout(
            Duration::from_secs(60),
            handle.expect("completion handle").wait(),
        )
        .await
        .expect("turn completes")
        .expect("completion resolves");
    }

    /// Child incarnations. Inert unless launched with the phase environment.
    #[tokio::test]
    async fn custody_restart_child() {
        let Some(root) = std::env::var_os(ROOT_ENV).map(PathBuf::from) else {
            return;
        };
        let Some(phase) = std::env::var(PHASE_ENV).ok() else {
            return;
        };
        let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
        match phase.as_str() {
            PHASE_GATEWAY => {
                // Create the session and submit a prompt whose turn runs the
                // delayed-effect shell tool.
                let (service, adapter) = build_service(&root, client).await;
                let session = Session::new();
                let session_id = session.id().clone();
                materialize(&service, &adapter, session).await;
                std::fs::write(root.join("session-id"), format!("{session_id}\n"))
                    .expect("session id");
                let (outcome, _handle) = adapter
                    .accept_input_with_completion(
                        &session_id,
                        Input::Prompt(PromptInput::new(INTERRUPTED_PROMPT, None)),
                    )
                    .await
                    .expect("accept prompt");
                let AcceptOutcome::Accepted { input_id, .. } = outcome else {
                    panic!("prompt not accepted: {outcome:?}");
                };
                std::fs::write(
                    root.join("input-id"),
                    serde_json::to_string(&input_id).expect("encode input id"),
                )
                .expect("input id");
                // The turn now blocks in the tool until the parent kills this
                // process.
                signal_parent(&root, "phase.fifo", "accepted").await;
            }
            PHASE_RECOVER_THEN_DIE => {
                let session_id = read_session_id(&root);
                let (_service, _adapter) = recover(&root, client, &session_id).await;
                signal_parent(&root, "phase.fifo", "recovered").await;
            }
            PHASE_NOTIFY_WITHOUT_ACK => {
                let session_id = read_session_id(&root);
                let (_service, adapter) = recover(&root, client, &session_id).await;
                // The evidence cannot be acknowledged: removing its record
                // fails, exactly as if this host died right after recording
                // the notice.
                let scope = scope_dir(&root, &session_id);
                set_mode(&scope, 0o500);
                prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
                signal_parent(&root, "phase.fifo", "notified").await;
            }
            other => panic!("unknown child phase {other}"),
        }
        // Wait to be killed.
        std::future::pending::<()>().await;
    }

    fn read_session_id(root: &Path) -> meerkat::SessionId {
        meerkat::SessionId::parse(
            std::fs::read_to_string(root.join("session-id"))
                .expect("session id")
                .trim(),
        )
        .expect("valid session id")
    }

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    /// The interrupted input is settled exactly once and never re-run: its
    /// terminal is `Abandoned { ToolProcessInterrupted }`, it still names the
    /// interrupted run (no second run consumed it), the transcript holds the
    /// interrupted prompt once and exactly one typed notice for that run, the
    /// model never called the tool again, and the killed tool's delayed
    /// effect never happens. Returns the interrupted run.
    async fn assert_settled_without_replay(
        root: &Path,
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
        client: &ScriptedShellClient,
    ) -> meerkat_core::lifecycle::RunId {
        let stored = adapter
            .input_state(session_id, input_id)
            .await
            .expect("input state")
            .expect("the interrupted input is known");
        assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
        assert_eq!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::ToolProcessInterrupted,
            })
        );
        let interrupted_run = stored
            .seed
            .last_run_id
            .clone()
            .expect("the input names the run it was interrupted in");

        let session = service
            .load_authoritative_session(session_id)
            .await
            .expect("load")
            .expect("session");
        let prompts = session
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user)
                    if user.text_content().contains(INTERRUPTED_PROMPT))
            })
            .count();
        assert_eq!(prompts, 1, "the interrupted prompt appears exactly once");
        let notices = session
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::SystemNotice(notice)
                    if notice.kind == SystemNoticeKind::ToolProcessRecovery)
            })
            .count();
        assert_eq!(notices, 1, "exactly one interrupted-run notice");
        let blocks = interrupted_notice_blocks(&session);
        assert_eq!(blocks.len(), 1);
        match &blocks[0] {
            SystemNoticeBlock::ToolProcessInterrupted {
                run_id,
                tool_call_id,
                cessation,
                disposition,
                ..
            } => {
                assert_eq!(run_id, &interrupted_run);
                assert_eq!(tool_call_id.as_deref(), Some(TOOL_CALL_ID));
                assert!(
                    matches!(cessation, ToolProcessCessation::KilledByRecovery { .. }),
                    "{cessation:?}"
                );
                assert_eq!(
                    disposition,
                    &InterruptedToolRunDisposition::InputsSettled {
                        inputs: 1,
                        unrestored: Vec::new(),
                    }
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
        let next_prompt_position = session
            .messages()
            .iter()
            .position(|message| {
                matches!(message, Message::User(user)
                    if user.text_content().contains(NEXT_PROMPT))
            })
            .expect("next prompt in transcript");
        assert!(
            notice_position < next_prompt_position,
            "the notice precedes the next prompt"
        );

        // No replay: the interrupted input never reached the model again.
        assert_eq!(
            client.tool_calls.load(Ordering::SeqCst),
            0,
            "the interrupted input was replayed"
        );
        // The evidence is acknowledged once the notice is recorded.
        assert_eq!(record_count(&scope_dir(root, session_id)), 0);
        // The killed tool's delayed effect never happens.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !root.join("effect").exists(),
            "the interrupted tool's effect happened"
        );
        interrupted_run
    }

    #[tokio::test]
    async fn gateway_sigkill_mid_tool_does_not_replay_the_interrupted_input() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let (session_id, input_id) = crash_gateway_mid_tool(root).await;

        // Next incarnation: the scripted model would call the tool again if
        // the interrupted input were replayed.
        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = recover(root, Arc::clone(&client), &session_id).await;

        // An idle session gets the notice at attach, without queued input
        // and without a model call: the notice is not a turn of its own.
        let recorded = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let session = service
                    .load_authoritative_session(&session_id)
                    .await
                    .expect("load")
                    .expect("session");
                if !interrupted_notice_blocks(&session).is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            recorded.is_ok(),
            "the idle session's notice was not recorded"
        );
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            0,
            "recovery must not start a model turn"
        );
        let abandoned = adapter
            .input_state(&session_id, &input_id)
            .await
            .expect("input state")
            .expect("input");
        assert_eq!(abandoned.seed.phase, InputLifecycleState::Abandoned);

        // The next real turn: its one model call already sees the typed
        // notice, recorded ahead of the new prompt.
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.requests_seeing_notice.load(Ordering::SeqCst),
            1,
            "the next real turn sees the interrupted-run notice"
        );
        let interrupted_run = assert_settled_without_replay(
            root,
            &service,
            &adapter,
            &session_id,
            &input_id,
            &client,
        )
        .await;
        assert_eq!(
            abandoned.seed.last_run_id.as_ref(),
            Some(&interrupted_run),
            "no later run consumed the interrupted input"
        );
    }

    /// Crash window: the recovering incarnation abandoned the input and owes
    /// the notice, then dies before any new input records it.
    #[tokio::test]
    async fn a_crash_after_settling_before_the_notice_still_notifies_once() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let (session_id, input_id) = crash_gateway_mid_tool(root).await;
        run_child_until(root, PHASE_RECOVER_THEN_DIE, "phase.fifo", "recovered").await;

        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = recover(root, Arc::clone(&client), &session_id).await;
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(client.requests_seeing_notice.load(Ordering::SeqCst), 1);
        assert_settled_without_replay(root, &service, &adapter, &session_id, &input_id, &client)
            .await;
    }

    /// Crash window: the recovering incarnation recorded the notice but never
    /// acknowledged the evidence. The final incarnation owes the same typed
    /// notice again, which the transcript records as a duplicate.
    #[tokio::test]
    async fn a_crash_after_the_notice_before_acknowledgement_does_not_duplicate_it() {
        use std::os::unix::fs::MetadataExt as _;
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        if std::fs::metadata(root).expect("tempdir metadata").uid() == 0 {
            // Root ignores the directory mode that makes acknowledgement fail.
            return;
        }
        let (session_id, input_id) = crash_gateway_mid_tool(root).await;
        run_child_until(root, PHASE_NOTIFY_WITHOUT_ACK, "phase.fifo", "notified").await;
        let scope = scope_dir(root, &session_id);
        assert_eq!(record_count(&scope), 1, "the evidence was not acknowledged");
        set_mode(&scope, 0o700);

        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = recover(root, Arc::clone(&client), &session_id).await;
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.requests_seeing_notice.load(Ordering::SeqCst),
            1,
            "the notice recorded by the dead incarnation is in the transcript"
        );
        assert_settled_without_replay(root, &service, &adapter, &session_id, &input_id, &client)
            .await;
    }
}
