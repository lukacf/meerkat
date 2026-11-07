//! A round commit whose outcome stays unknown past its retry budget, end to
//! end. A real native agent runs a queued prompt over the store checkpointer
//! of a host store realm. Its first round commits; the store then answers the
//! first commit of its second round `Indeterminate`, and every retry of that
//! batch and every head read `Unavailable`, so nothing the writer asks
//! settles it. The settle ends at its retry budget, typed, the runtime's
//! scope is poisoned and the realm keeps its fence. The checkpoint answers
//! the teardown class: the live session is torn down for a reload from what
//! the store holds, and since the store keeps the run's input queued, its
//! waiter is told it stays queued for the next start. The process
//! ends as a crash ends it (its runtime is shut down, and nothing of it runs
//! or writes again), the store answers again, and a restart over the same
//! store opens the session: its registration resumes the run from the last
//! round commit the store holds, and the run completes once.

use super::*;

/// The retry budget of the rows' settles: a few retries, and short of the
/// realm's 30 s default, which the rows' real-time run would wait out.
const SETTLE_BUDGET: Duration = Duration::from_secs(4);

/// The call id of the run's second round, whose first commit the first
/// process's store leaves unsettled.
const SECOND_ROUND: &str = "probe-round-2";

/// What outlives a process: the host store, its control, and the stores
/// beside it.
struct Durable {
    host: Arc<dyn HostStore>,
    control: Arc<FenceControl>,
    store: Arc<MemoryStore>,
    blobs: Arc<MemoryBlobStore>,
    temp: TempDir,
}

impl Durable {
    fn new() -> Arc<Self> {
        let control = Arc::new(FenceControl::default());
        Arc::new(Self {
            host: Arc::new(FenceHost::new(Arc::clone(&control))),
            control,
            store: Arc::new(MemoryStore::new()),
            blobs: Arc::new(MemoryBlobStore::new()),
            temp: TempDir::new().expect("temp"),
        })
    }
}

/// One process over the row's host store: its realm, runtime store, runtime,
/// session service, and its own model and tools.
struct Process {
    realm: Arc<SpiRealm>,
    runtime_store: Arc<SpiRuntimeStore>,
    machine: Arc<MeerkatMachine>,
    service: Arc<PersistentSessionService<FenceBuilder>>,
    client: Arc<FenceModel>,
    tools: Arc<FenceTools>,
}

impl Process {
    async fn open(durable: &Durable, acquire: AcquireMode, script: Vec<ModelStep>) -> Self {
        let realm = SpiRealm::open(
            Arc::clone(&durable.host),
            SpiRealmConfig {
                acquire,
                renew_timeout: RENEW_TIMEOUT,
                commit_retry_budget: SETTLE_BUDGET,
            },
        )
        .await
        .expect("the realm opens");
        let runtime_store = Arc::new(
            SpiRuntimeStore::open(Arc::clone(&realm))
                .await
                .expect("the runtime store opens"),
        );
        let machine = Arc::new(
            MeerkatMachine::persistent(runtime_store.clone(), durable.blobs.clone())
                .expect("canonical machine"),
        );
        let client = Arc::new(FenceModel {
            requests: AtomicUsize::new(0),
            control: Arc::clone(&durable.control),
            closes_during: ClosesDuring::Never,
            script,
        });
        let tools = Arc::new(FenceTools {
            dispatched: AtomicUsize::new(0),
            control: Arc::clone(&durable.control),
            closes_during: ClosesDuring::Never,
        });
        let builder = FenceBuilder {
            factory: AgentFactory::new(durable.temp.path().join("sessions"))
                .session_store(durable.store.clone()),
            client: client.clone(),
            tools: tools.clone(),
            config: Config::default(),
        };
        let service = Arc::new(
            PersistentSessionService::new(
                builder,
                1,
                durable.store.clone(),
                runtime_store.clone(),
                durable.blobs.clone(),
            )
            .with_canonical_runtime_adapter(machine.clone()),
        );
        Self {
            realm,
            runtime_store,
            machine,
            service,
            client,
            tools,
        }
    }

    /// Materialize `session` with its executor, as a surface does: a new
    /// session, or the one a restart loaded from the store.
    async fn materialize(&self, session: meerkat_core::Session) -> meerkat_core::SessionId {
        let (request, _) =
            crate::surface::split_runtime_backed_eager_create_request(CreateSessionRequest {
                model: "claude-sonnet-4-5".into(),
                prompt: "unused".into(),
                injected_context: Vec::new(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: InitialTurnPolicy::Defer,
                deferred_prompt_policy: DeferredPromptPolicy::Discard,
                build: None,
                labels: None,
            });
        let created = Box::pin(crate::surface::materialize_session(
            &self.service,
            &self.machine,
            session,
            request,
            {
                let service = self.service.clone();
                let machine = self.machine.clone();
                move |id| crate::surface::default_persistent_executor(service, machine, id)
            },
        ))
        .await
        .expect("materialize the session and its executor");
        created.session_id
    }
}

/// What the first process did before it ended.
#[derive(Debug)]
struct FirstProcess {
    session_id: meerkat_core::SessionId,
    /// The input's waiter's answer, in `Debug` form, when one came.
    answer: Option<String>,
    model_requests: usize,
    dispatched: usize,
    /// Each poisoned scope, and why.
    poisoned_scopes: Vec<(String, String)>,
    realm_poisoned: Option<String>,
    lease_live: bool,
}

/// The first process, on a runtime of its own: it queues the prompt, and
/// reports once the input's waiter is answered (or its deadline passes) and
/// the poison is in place. It then lives on, writing nothing more of its
/// own accord, until its runtime is shut down.
async fn first_process(
    durable: Arc<Durable>,
    applied: bool,
    report: tokio::sync::oneshot::Sender<FirstProcess>,
) {
    let process = Process::open(
        &durable,
        AcquireMode::Exclusive,
        vec![
            ModelStep::Tools,
            ModelStep::NamedTools(SECOND_ROUND, Some(applied)),
        ],
    )
    .await;
    let session_id = process.materialize(meerkat_core::Session::new()).await;
    let (_, completion) = process
        .machine
        .accept_input_with_completion(
            &session_id,
            Input::Prompt(PromptInput::new("probe once", None)),
        )
        .await
        .expect("the runtime accepts the prompt");
    let completion = completion.expect("a completion waiter");
    let answer = tokio::time::timeout(ANSWER_DEADLINE, completion.wait())
        .await
        .ok()
        .map(|answer| format!("{answer:?}"));
    let poisoned_by = tokio::time::Instant::now() + ANSWER_DEADLINE;
    while process.realm.poisoned_scopes().is_empty() && tokio::time::Instant::now() < poisoned_by {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = report.send(FirstProcess {
        session_id,
        answer,
        model_requests: process.client.requests.load(Ordering::SeqCst),
        dispatched: process.tools.dispatched.load(Ordering::SeqCst),
        poisoned_scopes: process
            .realm
            .poisoned_scopes()
            .into_iter()
            .map(|scope| (scope.id.to_string(), scope.cause))
            .collect(),
        realm_poisoned: process.realm.realm_poisoned(),
        lease_live: process.realm.lease_is_live(),
    });
    std::future::pending::<()>().await;
}

/// Run the first process until its round commit is unsettled past its
/// budget, end it as a crash ends it, and restart: what the first process
/// did, and the restarted process with the session open again.
async fn unsettled_then_restarted(
    applied: bool,
    second_script: Vec<ModelStep>,
) -> (FirstProcess, Process, Arc<Durable>) {
    let durable = Durable::new();
    let first_runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the first process's runtime");
    let (report, reported) = tokio::sync::oneshot::channel();
    drop(first_runtime.spawn(first_process(Arc::clone(&durable), applied, report)));
    let first = tokio::time::timeout(TURN_DEADLINE, reported)
        .await
        .expect("the first process reports")
        .expect("the first process sends its report");
    // The crash: every task of the first process ends where it is.
    first_runtime.shutdown_background();
    eprintln!("first process: {first:#?}");
    // The store answers again.
    durable.control.settle_nothing();
    let second = Process::open(&durable, AcquireMode::Takeover, second_script).await;
    let runtime_id = LogicalRuntimeId::for_session(&first.session_id);
    let tail = second
        .runtime_store
        .load_whole_blob_provisional_tail(&runtime_id)
        .await
        .expect("the restart reads the run's tail")
        .map(|tail| tail.authority().run_id().clone());
    let phases = input_phases_in(second.runtime_store.as_ref(), &first.session_id).await;
    eprintln!("the store at the restart: tail of run {tail:?}, input phases {phases:?}");
    assert!(
        tail.is_some(),
        "the store holds the run's provisional tail from its last round commit that landed"
    );
    assert!(
        all_pending(&phases),
        "the store holds the run's input pending: {phases:?}"
    );
    let session = second
        .service
        .load_authoritative_session(&first.session_id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    second.materialize(session).await;
    (first, second, durable)
}

/// What the first process must have done: the settle ended at its budget
/// with the outcome unknown, the runtime's scope alone is poisoned, the
/// realm keeps its fence, and the checkpoint's teardown class tore the live
/// session down; the store keeps the run's input queued, so its waiter is
/// told it stays queued for the next start, as at a lost fence.
fn assert_unsettled_past_its_budget(first: &FirstProcess) {
    assert!(
        first
            .answer
            .as_deref()
            .is_some_and(|answer| answer.starts_with("Err(RequeuedAcrossStop")),
        "the waiter is told its input stays queued for the next start: {first:#?}"
    );
    assert_eq!(
        first.model_requests, 2,
        "the first round and the second, each asked once: {first:#?}"
    );
    assert_eq!(
        first.dispatched, 1,
        "the first round's call ran; the second round's commit never settled: {first:#?}"
    );
    assert!(
        matches!(first.poisoned_scopes.as_slice(), [(scope, cause)]
            if scope.starts_with("rt:") && cause.contains("unavailable")),
        "the runtime's scope alone is poisoned, by the unsettled batch: {first:#?}"
    );
    assert!(
        first.realm_poisoned.is_none(),
        "the realm keeps its fence: {first:#?}"
    );
    assert!(
        first.lease_live,
        "the writer's window stays open: {first:#?}"
    );
}

/// The batch never landed: the store holds the run's tail from its first
/// round, and the restart resumes the run there. The resumed run asks for
/// its second round again, runs that round's call once, and completes; the
/// input is consumed once and the transcript holds each round once.
#[tokio::test]
async fn a_run_whose_round_commit_never_landed_resumes_at_the_restart_from_its_last_round() {
    let (first, second, _durable) =
        unsettled_then_restarted(false, vec![ModelStep::NamedTools(SECOND_ROUND, None)]).await;
    assert_unsettled_past_its_budget(&first);
    let phases = wait_for_phases(&second, &first.session_id, InputLifecycleState::Consumed).await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the resumed run completes, and its input is consumed once"
    );
    assert_eq!(
        second.client.requests.load(Ordering::SeqCst),
        2,
        "the resumed run asks for its second round again, then for the answer"
    );
    assert_eq!(
        second.tools.dispatched.load(Ordering::SeqCst),
        1,
        "the second round's call runs once"
    );
    assert_eq!(
        committed_rows_of(&second.service, &first.session_id).await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the prompt and each round are in the transcript once"
    );
}

/// The batch landed: the store holds the run's tail from the second round's
/// first commit, and the restart resumes the run there, its second round's
/// call not yet run. The resumed run runs that call once and completes; the
/// input is consumed once and the transcript holds each round once.
#[tokio::test]
async fn a_run_whose_round_commit_landed_unknown_resumes_at_the_restart_from_that_commit() {
    let (first, second, _durable) = unsettled_then_restarted(true, Vec::new()).await;
    assert_unsettled_past_its_budget(&first);
    let phases = wait_for_phases(&second, &first.session_id, InputLifecycleState::Consumed).await;
    assert_eq!(
        phases,
        Some(vec![Some(InputLifecycleState::Consumed)]),
        "the resumed run completes, and its input is consumed once"
    );
    assert_eq!(
        second.tools.dispatched.load(Ordering::SeqCst),
        1,
        "the second round's call runs once"
    );
    assert_eq!(
        committed_rows_of(&second.service, &first.session_id).await,
        vec![
            "user probe once".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant probed".to_string(),
        ],
        "the prompt and each round are in the transcript once"
    );
    eprintln!(
        "the resumed run asked the model {} times",
        second.client.requests.load(Ordering::SeqCst)
    );
}

/// Wait, at most [`TURN_DEADLINE`], until the session's one input reads
/// `phase` in the restarted process's store; the last reading either way.
async fn wait_for_phases(
    process: &Process,
    session_id: &meerkat_core::SessionId,
    phase: InputLifecycleState,
) -> Option<Vec<Option<InputLifecycleState>>> {
    let started = tokio::time::Instant::now();
    loop {
        let phases = input_phases_in(process.runtime_store.as_ref(), session_id).await;
        if phases.as_deref() == Some([Some(phase)].as_slice()) || started.elapsed() >= TURN_DEADLINE
        {
            return phases;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
