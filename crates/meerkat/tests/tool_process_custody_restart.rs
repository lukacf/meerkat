//! Abrupt gateway death mid-tool on a plain (non-mob) runtime-backed session.
//!
//! A child "gateway" process creates a session whose scripted model calls the
//! shell tool with a delayed file effect, and is SIGKILLed while the tool
//! runs. The run's first round commit (taken before any tool of the round
//! runs) stands as its provisional tail, so the run is open at a committed
//! round, and the next incarnation (this test process) resumes it when it
//! materializes the session. Durable process custody and the resumed round
//! must:
//!
//! - kill the orphaned tool before the session serves, so its delayed effect
//!   never happens;
//! - answer the interrupted call, whose process may have run, with an
//!   outcome-uncertain result instead of running it again: the call never
//!   runs twice;
//! - complete the resumed run under its own run id, consuming its input once,
//!   with no interrupted-run notice (the call's own result tells the model),
//!   and acknowledge the custody evidence.
//!
//! The scripted tool appends a line to `runs` on every execution and signals
//! the parent through `started.fifo` on its first execution only, so a call
//! that runs again shows as a second line (and as its effect) instead of
//! blocking on the FIFO.
//!
//! The crash-window rows kill an intermediate incarnation that resumed the
//! run: after the resumed round answered the call but before the run ended,
//! and after the run ended but before its evidence was acknowledged. The
//! final incarnation never runs the call again and tells the model about it
//! once, through the call's result.
//!
//! The idle-session row owes a notice at attach for an ended run whose
//! interrupted call the transcript does not answer, and pins that the idle
//! session records it exactly once when its loop checked for notices before
//! the hand-off.
//!
//! Every row runs on a HeadCanonical (SQLite) realm and, with `jsonl-store`,
//! on a WholeBlob (JSONL) realm.

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
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use meerkat::surface::{
        build_runtime_backed_service, default_persistent_executor, materialize_session,
    };
    use meerkat::{
        AgentFactory, Config, CreateSessionRequest, FactoryAgentBuilder, PersistentSessionService,
        Session,
    };
    use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::tool_process::{
        InterruptedRunInputs, InterruptedToolCall, InterruptedToolEvidence,
        InterruptedToolEvidenceError, InterruptedToolEvidenceSource, InterruptedToolRunDisposition,
        InterruptedToolSettlement, ToolProcessCessation, ToolProcessSpawner,
    };
    use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};
    use meerkat_core::{Message, SessionBuildOptions};
    use meerkat_runtime::{
        AcceptOutcome, Input, InputLifecycleState, InputTerminalOutcome, MeerkatMachine,
        PromptInput, RuntimeSessionPersistenceProfile, SessionServiceRuntimeExt,
    };
    use tokio::time::Duration;

    const PHASE_ENV: &str = "MEERKAT_CUSTODY_RESTART_PHASE";
    const ROOT_ENV: &str = "MEERKAT_CUSTODY_RESTART_ROOT";
    const CHILD_TEST: &str = "tests::custody_restart_child";
    const TOOL_CALL_ID: &str = "call-crash";
    const INTERRUPTED_PROMPT: &str = "create the effect file";
    const NEXT_PROMPT: &str = "what happened?";
    const ANSWER: &str = "noted";
    const PHASE_GATEWAY: &str = "gateway";
    /// Resume the run, then hold its next model call (the resumed round's
    /// answer is committed) and wait to be killed.
    const PHASE_RESUME_THEN_DIE: &str = "resume-then-die";
    /// Resume the run and let it end while its custody evidence cannot be
    /// acknowledged, then wait to be killed.
    const PHASE_RESUME_WITHOUT_ACK: &str = "resume-without-ack";
    /// Create an idle session and wait to be killed.
    const PHASE_CREATE: &str = "create";
    /// The realm profile of a row, read by every incarnation.
    const PROFILE_FILE: &str = "profile";
    /// Bound on a child incarnation reaching its signal; a child that exits
    /// first fails the row at once.
    const CHILD_BOUND: Duration = Duration::from_secs(120);
    /// Bound on a resumed run's end.
    const RUN_BOUND: Duration = Duration::from_secs(60);

    /// The realm's session persistence.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Profile {
        /// A SQLite realm: HeadCanonical sessions.
        HeadCanonical,
        /// A JSONL realm: WholeBlob sessions.
        #[cfg_attr(not(feature = "jsonl-store"), allow(dead_code))]
        WholeBlob,
    }

    fn write_profile(root: &Path, profile: Profile) {
        let name = match profile {
            Profile::HeadCanonical => "head-canonical",
            Profile::WholeBlob => "whole-blob",
        };
        std::fs::write(root.join(PROFILE_FILE), name).expect("write the row's profile");
    }

    fn read_profile(root: &Path) -> Profile {
        match std::fs::read_to_string(root.join(PROFILE_FILE))
            .expect("the row's profile")
            .as_str()
        {
            "head-canonical" => Profile::HeadCanonical,
            "whole-blob" => Profile::WholeBlob,
            other => panic!("unknown profile {other:?}"),
        }
    }

    /// What the scripted model does when a request already holds the tool's
    /// result or the interrupted-run notice.
    #[derive(Clone)]
    enum OnAnswered {
        /// Answer at once.
        Answer,
        /// Signal the parent through `phase.fifo`, then never answer.
        SignalAndHold(PathBuf),
        /// Make the custody scope unwritable (acknowledging its evidence then
        /// fails), then answer.
        LockScopeThenAnswer(PathBuf),
    }

    /// Scripted model: on a fresh request (no tool results, no
    /// interrupted-run notice) it calls the shell tool; otherwise it answers.
    struct ScriptedShellClient {
        command: String,
        on_answered: OnAnswered,
        tool_calls: AtomicUsize,
        requests: AtomicUsize,
        requests_seeing_notice: AtomicUsize,
    }

    impl ScriptedShellClient {
        fn new(command: String) -> Self {
            Self::with(command, OnAnswered::Answer)
        }

        fn with(command: String, on_answered: OnAnswered) -> Self {
            Self {
                command,
                on_answered,
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
            if answered {
                match &self.on_answered {
                    OnAnswered::Answer => {}
                    OnAnswered::SignalAndHold(fifo) => {
                        let fifo = fifo.clone();
                        drop(tokio::task::spawn_blocking(move || {
                            std::fs::write(fifo, "answered\n")
                        }));
                        return Box::pin(futures::stream::pending::<Result<LlmEvent, LlmError>>());
                    }
                    OnAnswered::LockScopeThenAnswer(scope) => set_mode(scope, 0o500),
                }
            }
            let mut events = Vec::new();
            let stop_reason = if answered {
                events.push(LlmEvent::TextDelta {
                    delta: ANSWER.to_string(),
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
                outcome: LlmDoneOutcome::success(stop_reason),
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

    /// The scripted tool: one `runs` line per execution, the FIFO signal on
    /// the first execution only, then the delayed effect.
    fn tool_command(root: &Path) -> String {
        format!(
            "echo ran >> '{runs}'; if [ ! -e '{once}' ]; then : > '{once}'; echo started > '{fifo}'; fi; sleep 2; echo effect >> '{effect}'",
            runs = root.join("runs").display(),
            once = root.join("started-once").display(),
            fifo = root.join("started.fifo").display(),
            effect = root.join("effect").display()
        )
    }

    /// How many times the scripted tool has run.
    fn tool_runs(root: &Path) -> usize {
        std::fs::read_to_string(root.join("runs"))
            .map(|runs| runs.lines().count())
            .unwrap_or(0)
    }

    /// The runtime-backed service over the row's realm. Without `custody`
    /// the factory keeps no process custody (no runtime root), so the only
    /// interrupted-run evidence is what the row installs.
    async fn build_service_with(
        root: &Path,
        client: Arc<ScriptedShellClient>,
        custody: bool,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        let profile = read_profile(root);
        let backend = match profile {
            Profile::HeadCanonical => meerkat_store::RealmBackend::Sqlite,
            Profile::WholeBlob => meerkat_store::RealmBackend::Jsonl,
        };
        let (_manifest, persistence) = meerkat::open_realm_persistence_in(
            root,
            "custody-realm",
            Some(backend),
            Some(meerkat_store::RealmOrigin::Explicit),
        )
        .await
        .expect("open realm persistence");
        let expected = match profile {
            Profile::HeadCanonical => RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            Profile::WholeBlob => RuntimeSessionPersistenceProfile::WholeBlobV1,
        };
        assert_eq!(persistence.session_persistence_profile(), expected);
        let project = root.join("project");
        std::fs::create_dir_all(&project).expect("project dir");
        let mut factory = AgentFactory::new(root.join("sessions"))
            .project_root(&project)
            .shell(true);
        if custody {
            factory = factory.runtime_root(root.join("realm"));
        }
        let mut config = Config::default();
        config.shell.program = "sh".to_string();
        config.shell.security_mode = meerkat_core::types::SecurityMode::Unrestricted;
        let mut builder = FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(client);
        let (service, adapter) = build_runtime_backed_service(builder, 4, persistence)
            .expect("compose the runtime-backed service");
        (Arc::new(service), adapter)
    }

    async fn build_service(
        root: &Path,
        client: Arc<ScriptedShellClient>,
    ) -> (
        Arc<PersistentSessionService<FactoryAgentBuilder>>,
        Arc<MeerkatMachine>,
    ) {
        build_service_with(root, client, true).await
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

    /// Write a line into a FIFO the parent blocks on, off the runtime thread.
    async fn signal_parent(root: &Path, name: &str, line: &str) {
        let path = root.join(name);
        let line = format!("{line}\n");
        tokio::task::spawn_blocking(move || std::fs::write(path, line))
            .await
            .expect("join signal")
            .expect("signal parent");
    }

    fn child_log_path(root: &Path, phase: &str) -> PathBuf {
        root.join(format!("child-{phase}.log"))
    }

    fn child_log(root: &Path, phase: &str) -> String {
        std::fs::read_to_string(child_log_path(root, phase)).unwrap_or_default()
    }

    fn spawn_child(root: &Path, phase: &str) -> std::process::Child {
        let log = std::fs::File::create(child_log_path(root, phase)).expect("child log");
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
            .env(PHASE_ENV, phase)
            .env(ROOT_ENV, root)
            .env("MEERKAT_DISABLE_GRAPH_DECODE_MEMO", "1")
            .stdout(log.try_clone().expect("child log"))
            .stderr(log)
            .spawn()
            .expect("spawn child incarnation")
    }

    /// Wait for a line in the FIFO `name`, written by `child` or by a tool it
    /// runs. A child that exits first, or never signals within the bound,
    /// fails the row at once with its log.
    async fn child_signal(
        root: &Path,
        child: &mut std::process::Child,
        phase: &str,
        name: &str,
    ) -> String {
        let fifo = root.join(name);
        // A plain thread, not a runtime blocking task, so a read that never
        // ends cannot hold the test runtime's shutdown.
        let reader = std::thread::spawn({
            let fifo = fifo.clone();
            move || std::fs::read_to_string(fifo)
        });
        let started = std::time::Instant::now();
        loop {
            if reader.is_finished() {
                return reader
                    .join()
                    .expect("join the FIFO reader")
                    .expect("read the FIFO");
            }
            let failure = match child.try_wait().expect("poll the child") {
                Some(status) => Some(format!("exited before {name} was written ({status})")),
                None if started.elapsed() > CHILD_BOUND => Some(format!("never wrote {name}")),
                None => None,
            };
            if let Some(failure) = failure {
                let _ = child.kill();
                let _ = child.wait();
                // End the blocked read: a writer that opens and closes the
                // FIFO gives it end of file. Detached, since its open blocks
                // when the reader has already finished.
                drop(std::thread::spawn(move || {
                    std::fs::OpenOptions::new().write(true).open(fifo)
                }));
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !reader.is_finished() && std::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                if reader.is_finished() {
                    let line = reader
                        .join()
                        .expect("join the FIFO reader")
                        .unwrap_or_default();
                    if !line.is_empty() {
                        return line;
                    }
                }
                panic!("child phase {phase} {failure}: {}", child_log(root, phase));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Run a child incarnation until it writes `phase.fifo`, then SIGKILL it.
    async fn run_child_until(root: &Path, phase: &str, expected: &str) {
        let mut child = spawn_child(root, phase);
        let line = child_signal(root, &mut child, phase, "phase.fifo").await;
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            line.trim(),
            expected,
            "child phase {phase}: {}",
            child_log(root, phase)
        );
    }

    fn make_fifos(root: &Path) {
        for fifo in ["started.fifo", "phase.fifo"] {
            nix::unistd::mkfifo(&root.join(fifo), nix::sys::stat::Mode::S_IRWXU).expect("mkfifo");
        }
    }

    /// The first incarnation dies abruptly while its shell tool runs.
    /// Returns the session and the interrupted prompt's input id.
    async fn crash_gateway_mid_tool(
        root: &Path,
        profile: Profile,
    ) -> (meerkat::SessionId, InputId) {
        write_profile(root, profile);
        make_fifos(root);
        let mut gateway = spawn_child(root, PHASE_GATEWAY);
        // The tool itself reports it is running, and the gateway that it
        // recorded the accepted input.
        let started = child_signal(root, &mut gateway, PHASE_GATEWAY, "started.fifo").await;
        let accepted = child_signal(root, &mut gateway, PHASE_GATEWAY, "phase.fifo").await;
        // Abrupt gateway death mid-tool.
        gateway.kill().unwrap();
        gateway.wait().unwrap();
        assert_eq!(started.trim(), "started");
        assert_eq!(accepted.trim(), "accepted");
        assert_eq!(tool_runs(root), 1, "the gateway ran the tool once");
        (read_session_id(root), read_input_id(root))
    }

    /// The run of the session's provisional tail, if one stands.
    async fn tail_run(
        service: &PersistentSessionService<FactoryAgentBuilder>,
        session_id: &meerkat::SessionId,
    ) -> Option<RunId> {
        let runtime_store = service.runtime_store();
        let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
        match runtime_store.session_persistence_profile() {
            RuntimeSessionPersistenceProfile::HeadCanonicalV1 => runtime_store
                .load_head_canonical_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
                .map(|tail| tail.run_id().clone()),
            RuntimeSessionPersistenceProfile::WholeBlobV1 => runtime_store
                .load_whole_blob_provisional_tail(&runtime_id)
                .await
                .expect("read the provisional tail")
                .map(|tail| tail.authority().run_id().clone()),
            other => panic!("unexpected session persistence profile {other:?}"),
        }
    }

    /// Open the realm and materialize the session: a session whose run is
    /// open resumes that run here. The run is open, so the restart reads the
    /// session's committed snapshot under its provisional tail.
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
            .load_committed_session_tolerating_tail(session_id)
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
        match phase.as_str() {
            PHASE_GATEWAY => {
                // Create the session and submit a prompt whose turn runs the
                // delayed-effect shell tool.
                let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
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
            PHASE_RESUME_THEN_DIE => {
                // The resumed run's next model call signals the parent and
                // never answers: the round's answer is committed and the run
                // is still open.
                let session_id = read_session_id(&root);
                let client = Arc::new(ScriptedShellClient::with(
                    tool_command(&root),
                    OnAnswered::SignalAndHold(root.join("phase.fifo")),
                ));
                let (_service, _adapter) = recover(&root, client, &session_id).await;
            }
            PHASE_RESUME_WITHOUT_ACK => {
                // The resumed run ends while its custody evidence cannot be
                // acknowledged (its records cannot be removed), exactly as if
                // this host died right after the run ended.
                let session_id = read_session_id(&root);
                let input_id = read_input_id(&root);
                let client = Arc::new(ScriptedShellClient::with(
                    tool_command(&root),
                    OnAnswered::LockScopeThenAnswer(scope_dir(&root, &session_id)),
                ));
                let (service, adapter) = recover(&root, client, &session_id).await;
                wait_for_the_resumed_run_to_end(&root, &service, &adapter, &session_id, &input_id)
                    .await;
                signal_parent(&root, "phase.fifo", "ended").await;
            }
            PHASE_CREATE => {
                let client = Arc::new(ScriptedShellClient::new(tool_command(&root)));
                let (service, adapter) = build_service(&root, client).await;
                let session = Session::new();
                let session_id = session.id().clone();
                materialize(&service, &adapter, session).await;
                std::fs::write(root.join("session-id"), format!("{session_id}\n"))
                    .expect("session id");
                signal_parent(&root, "phase.fifo", "created").await;
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

    fn read_input_id(root: &Path) -> InputId {
        serde_json::from_str(&std::fs::read_to_string(root.join("input-id")).expect("input id"))
            .expect("valid input id")
    }

    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    /// Wait until the resumed run has ended: its input consumed and its
    /// provisional tail gone. A second run of the interrupted call fails the
    /// row at once.
    async fn wait_for_the_resumed_run_to_end(
        root: &Path,
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        input_id: &InputId,
    ) {
        let started = std::time::Instant::now();
        loop {
            assert_eq!(
                tool_runs(root),
                1,
                "the interrupted call ran again after the restart"
            );
            let consumed = adapter
                .input_state(session_id, input_id)
                .await
                .expect("input state")
                .is_some_and(|state| state.seed.phase == InputLifecycleState::Consumed);
            if consumed && tail_run(service, session_id).await.is_none() {
                return;
            }
            assert!(
                started.elapsed() < RUN_BOUND,
                "the resumed run did not end (input consumed: {consumed})"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The interrupted call was answered once and never run again: the input
    /// is consumed by the interrupted run itself; the transcript holds the
    /// interrupted prompt once, the call once, its one outcome-uncertain
    /// result and no interrupted-run notice; the tool ran once; the custody
    /// evidence is acknowledged; and the killed tool's delayed effect never
    /// happens.
    async fn assert_answered_once_without_replay(
        root: &Path,
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: &Arc<MeerkatMachine>,
        session_id: &meerkat::SessionId,
        input_id: &InputId,
        interrupted_run: &RunId,
    ) {
        let stored = adapter
            .input_state(session_id, input_id)
            .await
            .expect("input state")
            .expect("the interrupted input is known");
        assert_eq!(stored.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(
            stored.seed.last_run_id.as_ref(),
            Some(interrupted_run),
            "the interrupted run itself consumed its input: no other run replayed it"
        );

        let session = service
            .load_authoritative_session(session_id)
            .await
            .expect("load")
            .expect("session");
        let messages = session.messages();
        let prompts = messages
            .iter()
            .filter(|message| {
                matches!(message, Message::User(user)
                    if user.text_content().contains(INTERRUPTED_PROMPT))
            })
            .count();
        assert_eq!(prompts, 1, "the interrupted prompt appears exactly once");
        let calls = messages
            .iter()
            .filter(|message| {
                matches!(message, Message::BlockAssistant(answer)
                    if answer.tool_calls().any(|call| call.id == TOOL_CALL_ID))
            })
            .count();
        assert_eq!(calls, 1, "the model asked for the call exactly once");
        let results: Vec<String> = messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults { results, .. } => Some(results),
                _ => None,
            })
            .flatten()
            .filter(|result| result.tool_use_id == TOOL_CALL_ID)
            .map(meerkat_core::ToolResult::text_content)
            .collect();
        assert_eq!(
            results.len(),
            1,
            "the call has exactly one result: {results:?}"
        );
        assert!(
            results[0].contains("outcome_uncertain") && results[0].contains("It was not run again"),
            "the interrupted call is answered outcome-uncertain: {}",
            results[0]
        );
        let notices = messages
            .iter()
            .filter(|message| {
                matches!(message, Message::SystemNotice(notice)
                    if notice.kind == SystemNoticeKind::ToolProcessRecovery)
            })
            .count();
        assert_eq!(
            notices, 0,
            "the call's own result tells the model: no interrupted-run notice"
        );

        assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
        assert_eq!(
            record_count(&scope_dir(root, session_id)),
            0,
            "the custody evidence is acknowledged"
        );
        // The killed tool's delayed effect never happens.
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !root.join("effect").exists(),
            "the interrupted tool's effect happened"
        );
        assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
    }

    /// How many assistant messages give the scripted answer.
    async fn answers(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: &meerkat::SessionId,
    ) -> usize {
        service
            .load_authoritative_session(session_id)
            .await
            .expect("load")
            .expect("session")
            .messages()
            .iter()
            .filter(|message| {
                matches!(message, Message::BlockAssistant(answer)
                    if answer.text_blocks().collect::<String>() == ANSWER)
            })
            .count()
    }

    async fn gateway_sigkill_mid_tool(profile: Profile) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let (session_id, input_id) = crash_gateway_mid_tool(root, profile).await;

        // Next incarnation: the run is open at its committed round. It
        // resumes when the session materializes, with no new input; its
        // input stays bound to it, so nothing ends without a run.
        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = build_service(root, Arc::clone(&client)).await;
        let runless_terminal_tells = Arc::new(AtomicUsize::new(0));
        adapter.set_runless_input_terminal_hint(Arc::new({
            let tells = Arc::clone(&runless_terminal_tells);
            move || {
                tells.fetch_add(1, Ordering::SeqCst);
            }
        }));
        let interrupted_run = tail_run(&service, &session_id)
            .await
            .expect("the killed run's round commit stands as its provisional tail");
        let resume = service
            .load_committed_session_tolerating_tail(&session_id)
            .await
            .expect("load after restart")
            .expect("session survives the gateway");
        materialize(&service, &adapter, resume).await;
        wait_for_the_resumed_run_to_end(root, &service, &adapter, &session_id, &input_id).await;

        assert_eq!(
            client.tool_calls.load(Ordering::SeqCst),
            0,
            "the model never asked for the call again"
        );
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            1,
            "the resumed run made its one next model call"
        );
        assert_eq!(client.requests_seeing_notice.load(Ordering::SeqCst), 0);
        assert_answered_once_without_replay(
            root,
            &service,
            &adapter,
            &session_id,
            &input_id,
            &interrupted_run,
        )
        .await;
        assert_eq!(
            runless_terminal_tells.load(Ordering::SeqCst),
            0,
            "the resumed run consumed its input: nothing ended without a run"
        );

        // The next real turn: one more model call, which sees the answered
        // call and no notice.
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.requests.load(Ordering::SeqCst), 2);
        assert_eq!(client.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(client.requests_seeing_notice.load(Ordering::SeqCst), 0);
        assert_eq!(answers(&service, &session_id).await, 2);
        assert_eq!(tool_runs(root), 1, "the interrupted call ran again");
    }

    #[tokio::test]
    async fn gateway_sigkill_mid_tool_does_not_replay_the_interrupted_input() {
        gateway_sigkill_mid_tool(Profile::HeadCanonical).await;
    }

    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn gateway_sigkill_mid_tool_does_not_replay_the_interrupted_input_whole_blob() {
        gateway_sigkill_mid_tool(Profile::WholeBlob).await;
    }

    /// Process-custody evidence of one interrupted shell call of a run that
    /// ended before this incarnation and that the transcript does not answer
    /// (the run is not in it), kept as a durable store keeps it: until
    /// acknowledged.
    struct EndedRunEvidence {
        calls: Mutex<Vec<InterruptedToolCall>>,
    }

    impl EndedRunEvidence {
        fn owing(run_id: &RunId) -> Self {
            Self {
                calls: Mutex::new(vec![InterruptedToolCall {
                    entry_id: uuid::Uuid::new_v4(),
                    run_id: run_id.clone(),
                    tool_call_id: Some(TOOL_CALL_ID.to_string()),
                    spawner: ToolProcessSpawner::ShellCall,
                    cessation: ToolProcessCessation::KilledByRecovery { members: 1 },
                    settlement: InterruptedToolSettlement::Pending,
                }]),
            }
        }

        fn unacknowledged(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl InterruptedToolEvidence for EndedRunEvidence {
        async fn interrupted_calls(
            &self,
        ) -> Result<Vec<InterruptedToolCall>, InterruptedToolEvidenceError> {
            Ok(self.calls.lock().unwrap().clone())
        }

        async fn mark_inputs_settled(
            &self,
            _entry_ids: &[uuid::Uuid],
            _inputs: &InterruptedRunInputs,
        ) -> Result<(), InterruptedToolEvidenceError> {
            panic!("an ended run has no in-flight input to settle")
        }

        async fn acknowledge(
            &self,
            entry_ids: &[uuid::Uuid],
        ) -> Result<(), InterruptedToolEvidenceError> {
            self.calls
                .lock()
                .unwrap()
                .retain(|call| !entry_ids.contains(&call.entry_id));
            Ok(())
        }

        async fn run_ended(&self, _run_id: &RunId) -> Result<(), InterruptedToolEvidenceError> {
            Ok(())
        }
    }

    struct EndedRunEvidenceSource(Arc<EndedRunEvidence>);

    #[async_trait::async_trait]
    impl InterruptedToolEvidenceSource for EndedRunEvidenceSource {
        async fn settle_session(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<Option<Arc<dyn InterruptedToolEvidence>>, InterruptedToolEvidenceError>
        {
            Ok(Some(Arc::clone(&self.0) as Arc<dyn InterruptedToolEvidence>))
        }
    }

    /// The typed blocks of every interrupted-run notice in the session's
    /// committed transcript, one entry per notice.
    async fn interrupted_notice_blocks(
        service: &Arc<PersistentSessionService<FactoryAgentBuilder>>,
        session_id: &meerkat::SessionId,
    ) -> Vec<Vec<SystemNoticeBlock>> {
        service
            .load_authoritative_session(session_id)
            .await
            .expect("load")
            .expect("session")
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
            .collect()
    }

    /// The attach wakes its runtime loop when it publishes, and hands the
    /// owed notices to the session just after. The loop runs on its own
    /// runtime, so it can spend that wake on a pre-dequeue check that finds
    /// nothing yet and park. The idle session must still record the notice,
    /// exactly once, without new input and without a model call.
    ///
    /// The notice is owed for an ended run whose interrupted shell call the
    /// transcript does not answer (the run is not in it). A host crash no
    /// longer leaves such a run for a foreground call: the call's round is
    /// committed before its process starts, so the run resumes and answers
    /// the call. The evidence comes from an evidence source the row installs,
    /// on a service without process custody of its own.
    async fn idle_session_records_the_owed_notice_once(profile: Profile) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write_profile(root, profile);
        make_fifos(root);
        run_child_until(root, PHASE_CREATE, "created").await;
        let session_id = read_session_id(root);

        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = build_service_with(root, Arc::clone(&client), false).await;
        let ended_run = RunId::new();
        let evidence = Arc::new(EndedRunEvidence::owing(&ended_run));
        adapter.set_interrupted_tool_evidence_source(Arc::new(EndedRunEvidenceSource(Arc::clone(
            &evidence,
        ))));
        // Hold the attach between its publication and the notice hand-off,
        // and the loop at its first check that finds no notice.
        let (handoff_reached, release_handoff) =
            adapter.arm_attachment_interrupted_notice_handoff_test_hook(session_id.clone());
        let (check_reached, release_check) = adapter
            .arm_runtime_loop_after_empty_interrupted_notice_check_test_hook(session_id.clone());
        let mut parked = adapter.runtime_loop_parked();
        let resume = service
            .load_authoritative_session(&session_id)
            .await
            .expect("load after restart")
            .expect("session survives the restart");
        let interleave = async {
            tokio::time::timeout(Duration::from_secs(30), handoff_reached)
                .await
                .expect("the attach reaches its notice hand-off")
                .expect("hand-off gate");
            tokio::time::timeout(Duration::from_secs(30), check_reached)
                .await
                .expect("the loop checks for notices before the hand-off")
                .expect("check gate");
            // The gate holds the loop mid-lap, so the next park comes after
            // that lap, with every wake (the attach's included) spent.
            drop(release_check);
            tokio::time::timeout(
                Duration::from_secs(30),
                parked.wait_for(|sessions| sessions.contains(&session_id)),
            )
            .await
            .expect("the loop parks")
            .expect("park watch");
            drop(release_handoff);
        };
        tokio::join!(materialize(&service, &adapter, resume), interleave);

        let recorded = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !interrupted_notice_blocks(&service, &session_id)
                    .await
                    .is_empty()
                    && evidence.unacknowledged() == 0
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            recorded.is_ok(),
            "the idle session's notice waited for new input after a late hand-off \
             (unacknowledged evidence: {})",
            evidence.unacknowledged()
        );
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            0,
            "recording the notice is not a model turn"
        );
        let expected = vec![vec![SystemNoticeBlock::ToolProcessInterrupted {
            run_id: ended_run,
            tool_call_id: Some(TOOL_CALL_ID.to_string()),
            spawner: ToolProcessSpawner::ShellCall,
            cessation: ToolProcessCessation::KilledByRecovery { members: 1 },
            disposition: InterruptedToolRunDisposition::RunCompleted,
        }]];
        assert_eq!(
            interrupted_notice_blocks(&service, &session_id).await,
            expected,
            "exactly one notice, for the ended run's call"
        );

        // The next real turn: its one model call sees the notice, which is
        // still recorded once.
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(client.requests_seeing_notice.load(Ordering::SeqCst), 1);
        assert_eq!(client.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            interrupted_notice_blocks(&service, &session_id).await,
            expected
        );
    }

    #[tokio::test]
    async fn an_idle_session_records_the_notice_when_its_loop_checked_before_the_handoff() {
        idle_session_records_the_owed_notice_once(Profile::HeadCanonical).await;
    }

    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn an_idle_session_records_the_notice_when_its_loop_checked_before_the_handoff_whole_blob()
     {
        idle_session_records_the_owed_notice_once(Profile::WholeBlob).await;
    }

    /// Crash window: the incarnation that resumed the run committed the
    /// round's answer to the interrupted call and died at the run's next
    /// model call. The final incarnation resumes the run there: it never runs
    /// the call again, never answers it twice, and ends the run once.
    async fn crash_after_the_resumed_round_answered(profile: Profile) {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let (session_id, input_id) = crash_gateway_mid_tool(root, profile).await;
        run_child_until(root, PHASE_RESUME_THEN_DIE, "answered").await;

        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = build_service(root, Arc::clone(&client)).await;
        let interrupted_run = tail_run(&service, &session_id)
            .await
            .expect("the resumed run's answered round stands as its provisional tail");
        let resume = service
            .load_committed_session_tolerating_tail(&session_id)
            .await
            .expect("load after restart")
            .expect("session survives the crash");
        materialize(&service, &adapter, resume).await;
        wait_for_the_resumed_run_to_end(root, &service, &adapter, &session_id, &input_id).await;
        assert_eq!(client.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            1,
            "the run resumed at its next model call"
        );
        assert_eq!(client.requests_seeing_notice.load(Ordering::SeqCst), 0);
        assert_answered_once_without_replay(
            root,
            &service,
            &adapter,
            &session_id,
            &input_id,
            &interrupted_run,
        )
        .await;
        assert_eq!(answers(&service, &session_id).await, 1);
    }

    #[tokio::test]
    async fn a_crash_after_the_resumed_round_answered_does_not_run_the_call_again() {
        crash_after_the_resumed_round_answered(Profile::HeadCanonical).await;
    }

    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn a_crash_after_the_resumed_round_answered_does_not_run_the_call_again_whole_blob() {
        crash_after_the_resumed_round_answered(Profile::WholeBlob).await;
    }

    /// Crash window: the incarnation that resumed the run ended it but could
    /// not acknowledge the custody evidence before it died. The final
    /// incarnation settles that evidence for a run that has ended and whose
    /// call the transcript already answers: it acknowledges it without
    /// telling the model again, and never runs the call again.
    async fn crash_before_the_resumed_run_acknowledged(profile: Profile) {
        use std::os::unix::fs::MetadataExt as _;
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        if std::fs::metadata(root).expect("tempdir metadata").uid() == 0 {
            // Root ignores the directory mode that makes acknowledgement fail.
            return;
        }
        let (session_id, input_id) = crash_gateway_mid_tool(root, profile).await;
        run_child_until(root, PHASE_RESUME_WITHOUT_ACK, "ended").await;
        let scope = scope_dir(root, &session_id);
        assert_eq!(record_count(&scope), 1, "the evidence was not acknowledged");
        set_mode(&scope, 0o700);

        let client = Arc::new(ScriptedShellClient::new(tool_command(root)));
        let (service, adapter) = recover(root, Arc::clone(&client), &session_id).await;
        // The run ended before this incarnation: nothing resumes. A notice
        // the attach owed would be recorded ahead of the next prompt.
        prompt_and_wait(&adapter, &session_id, NEXT_PROMPT).await;
        assert_eq!(client.tool_calls.load(Ordering::SeqCst), 0);
        assert_eq!(client.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.requests_seeing_notice.load(Ordering::SeqCst),
            0,
            "the transcript already answers the call: no second tell"
        );
        let interrupted_run = adapter
            .input_state(&session_id, &input_id)
            .await
            .expect("input state")
            .expect("input")
            .seed
            .last_run_id
            .expect("the input names the run that consumed it");
        assert_answered_once_without_replay(
            root,
            &service,
            &adapter,
            &session_id,
            &input_id,
            &interrupted_run,
        )
        .await;
        assert_eq!(answers(&service, &session_id).await, 2);
    }

    #[tokio::test]
    async fn a_crash_before_the_resumed_run_acknowledged_does_not_run_the_call_again() {
        crash_before_the_resumed_run_acknowledged(Profile::HeadCanonical).await;
    }

    #[cfg(feature = "jsonl-store")]
    #[tokio::test]
    async fn a_crash_before_the_resumed_run_acknowledged_does_not_run_the_call_again_whole_blob() {
        crash_before_the_resumed_run_acknowledged(Profile::WholeBlob).await;
    }
}
