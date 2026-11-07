//! A durable run's provisional tail as its hosts meet it, on both durable
//! persistence profiles: a host that opens the session the run left after a
//! power cut, and a host that mutates the session while the run holds a
//! round commit.
//!
//! A power cut leaves a run open after its round commit: the store keeps the
//! round as the run's provisional tail. The next host opens the session the
//! way `rkat resume` does (the session store's copy, then the surface's
//! materialization, with no prepared resume), its registration resumes the
//! run from that tail, and the resumed run reaches the model. An interrupt of
//! the resumed run then ends its inputs and closes its tail as an interrupted
//! run, so the next turn completes over it and nothing of the run runs again.
//! The power cut is a byte copy of the first host's database files, taken
//! while its run waits on the model, opened by a second host under another
//! path.

use super::interrupt_tail_tests::{
    DEADLINE, ScriptedBuilder, ScriptedModel, Start, Step, history_with_closed_run_a,
    run_a_held_after_a_round_commit,
};
use super::*;
use meerkat_core::service::{
    AppendSystemContextRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionControlError,
    SessionError, SessionService, SessionServiceControlExt,
};
use meerkat_core::{AssistantBlock, Message, RunId, SessionId};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input_state::InputLifecycleState;
use meerkat_runtime::store::{InputStateRow, RuntimeStore};
use meerkat_runtime::{Input, PromptInput};
use meerkat_session::PersistentSessionService;
use meerkat_store::MemoryBlobStore;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tempfile::TempDir;

/// The durable stores of one host, on disk: a HeadCanonical SQLite file that
/// is both the runtime and the session store, or a WholeBlob runtime database
/// beside a JSONL session store.
enum DiskStores {
    HeadCanonical { path: PathBuf },
    WholeBlob { dir: PathBuf },
}

impl DiskStores {
    fn new(head_canonical: bool, root: &TempDir) -> Self {
        if head_canonical {
            Self::HeadCanonical {
                path: root.path().join("sessions.sqlite3"),
            }
        } else {
            Self::WholeBlob {
                dir: root.path().join("realm"),
            }
        }
    }

    fn session_store(&self) -> Arc<dyn meerkat_store::SessionStore> {
        match self {
            Self::HeadCanonical { path } => Arc::new(
                meerkat_store::SqliteSessionStore::open(path.clone())
                    .expect("open the sqlite session store"),
            ),
            Self::WholeBlob { dir } => Arc::new(meerkat_store::JsonlStore::new(dir.join("jsonl"))),
        }
    }

    /// The runtime store, hosting its sessions across processes with
    /// OS-locked claims at `hosting` when given, as a realm opens it.
    fn runtime_store(
        &self,
        hosting: Option<meerkat_runtime::HostingPaths>,
    ) -> Arc<dyn RuntimeStore> {
        let store = match self {
            Self::HeadCanonical { path } => {
                meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path.clone())
                    .expect("open the head-canonical runtime store")
            }
            Self::WholeBlob { dir } => {
                std::fs::create_dir_all(dir).expect("create the realm directory");
                meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                    dir.join("runtime.sqlite3"),
                )
                .expect("open the whole-blob runtime store")
            }
        };
        let Some(hosting) = hosting else {
            return Arc::new(store);
        };
        let store: Arc<dyn RuntimeStore> = Arc::new(store.with_hosting_paths(hosting));
        assert!(
            store.hosting_capability().is_cross_process(),
            "the store hosts its sessions with OS-locked claims"
        );
        store
    }

    /// One process start over the stores.
    fn open(&self, root: &TempDir, client: &Arc<ScriptedModel>) -> Start {
        self.open_with(root, client, None)
    }

    /// One process start over the stores that hosts their sessions with
    /// cross-process claims, its own under `root`.
    fn open_hosted(&self, root: &TempDir, client: &Arc<ScriptedModel>) -> Start {
        self.open_with(root, client, Some(hosting_paths(root)))
    }

    fn open_with(
        &self,
        root: &TempDir,
        client: &Arc<ScriptedModel>,
        hosting: Option<meerkat_runtime::HostingPaths>,
    ) -> Start {
        let persistence = crate::PersistenceBundle::new(
            self.session_store(),
            self.runtime_store(hosting),
            Arc::new(MemoryBlobStore::new()),
        )
        .expect("construct runtime authority");
        let machine = persistence.runtime_adapter();
        let (sessions, runtime_store, blobs) = persistence.into_parts();
        let builder = ScriptedBuilder {
            factory: AgentFactory::new(root.path().join("sessions"))
                .session_store(sessions.clone()),
            client: client.clone(),
        };
        let service = Arc::new(
            PersistentSessionService::new(builder, 4, sessions, runtime_store.clone(), blobs)
                .with_canonical_runtime_adapter(machine.clone()),
        );
        Start {
            service,
            machine,
            runtime_store,
            head_canonical: matches!(self, Self::HeadCanonical { .. }),
        }
    }

    /// The durable state a power cut leaves now: a byte copy of the stores'
    /// files under `root`, opened by no host.
    fn power_cut_copy(&self, root: &TempDir) -> Self {
        match self {
            Self::HeadCanonical { path } => {
                let copy = root.path().join("sessions.sqlite3");
                copy_database(path, &copy);
                Self::HeadCanonical { path: copy }
            }
            Self::WholeBlob { dir } => {
                let copy = root.path().join("realm");
                std::fs::create_dir_all(&copy).expect("create the copied realm directory");
                copy_database(&dir.join("runtime.sqlite3"), &copy.join("runtime.sqlite3"));
                copy_tree(&dir.join("jsonl"), &copy.join("jsonl"));
                Self::WholeBlob { dir: copy }
            }
        }
    }

    /// The session the next host materializes. On HeadCanonical it is the
    /// session store's own copy, which `rkat resume` loads before it
    /// materializes the session: the strict committed read refuses the
    /// boundary while the run's tail stands. A WholeBlob realm keeps its
    /// committed document in the runtime database, read by the committed
    /// read.
    async fn stored_session(&self, start: &Start, id: &SessionId) -> meerkat_core::Session {
        match self {
            Self::HeadCanonical { .. } => self
                .session_store()
                .load(id)
                .await
                .expect("load the stored session")
                .expect("the session is stored"),
            Self::WholeBlob { .. } => start
                .service
                .load_authoritative_session(id)
                .await
                .expect("load the committed session")
                .expect("the session is committed"),
        }
    }
}

/// Materialize `session` through the surface's materialization, with no
/// first turn.
async fn materialize_session(start: &Start, session: meerkat_core::Session) -> SessionId {
    try_materialize_session(start, session)
        .await
        .expect("materialize the session and its executor")
}

/// [`materialize_session`], answering a refused materialization.
async fn try_materialize_session(
    start: &Start,
    session: meerkat_core::Session,
) -> Result<SessionId, crate::surface::SurfaceRuntimeMaterializeError> {
    let request = CreateSessionRequest {
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
    };
    let created = Box::pin(crate::surface::materialize_session(
        &start.service,
        &start.machine,
        session,
        request,
        {
            let service = start.service.clone();
            let machine = start.machine.clone();
            move |id| crate::surface::default_persistent_executor(service, machine, id)
        },
    ))
    .await?;
    Ok(created.session_id)
}

/// One whole turn.
async fn turn(start: &Start, id: &SessionId, prompt: &str) -> Option<CompletionOutcome> {
    let (_, completion) = start
        .machine
        .accept_input_with_completion(id, Input::Prompt(PromptInput::new(prompt, None)))
        .await
        .expect("the runtime accepts the prompt");
    let completion = completion.expect("a completion");
    tokio::time::timeout(DEADLINE, completion.wait())
        .await
        .ok()
        .and_then(Result::ok)
}

/// The run of the store's provisional tail, if one stands.
async fn tail_run(start: &Start, id: &SessionId) -> Option<RunId> {
    let runtime_id = LogicalRuntimeId::for_session(id);
    if start.head_canonical {
        start
            .runtime_store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .map(|tail| tail.run_id().clone())
    } else {
        start
            .runtime_store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .map(|tail| tail.authority().run_id().clone())
    }
}

/// The committed transcript after the system prompt, one label per row.
async fn committed_rows(start: &Start, id: &SessionId) -> Vec<String> {
    let session = start
        .service
        .load_authoritative_session(id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::System(_) => None,
            Message::User(user) => Some(format!("user {}", user.text_content())),
            Message::BlockAssistant(assistant) => Some(
                if assistant
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::ToolUse { .. }))
                {
                    "assistant tool use".to_string()
                } else {
                    format!("assistant {}", assistant.text_blocks().collect::<String>())
                },
            ),
            Message::ToolResults { .. } => Some("tool results".to_string()),
            Message::SystemNotice(notice) => Some(format!("notice {:?}", notice.kind)),
        })
        .collect()
}

/// Copy one SQLite database with its write-ahead log, as a power cut leaves
/// them.
fn copy_database(from: &Path, to: &Path) {
    std::fs::copy(from, to).expect("copy the database");
    let log = from.with_extension("sqlite3-wal");
    if log.exists() {
        std::fs::copy(&log, to.with_extension("sqlite3-wal")).expect("copy the write-ahead log");
    }
}

fn copy_tree(from: &Path, to: &Path) {
    if !from.exists() {
        return;
    }
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

async fn an_interrupt_of_a_run_a_restart_resumed_closes_it_and_the_next_turn_completes(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    let (_turn_a, run_a) = run_a_held_after_a_round_commit(&first, &client, &id).await;
    // The power goes out while run A waits on the model.
    let after_the_cut = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_cut);

    let resumed_client = Arc::new(ScriptedModel::new());
    resumed_client.push(Step::Held);
    let start = restarted.open(&after_the_cut, &resumed_client);
    materialize_session(&start, restarted.stored_session(&start, &id).await).await;
    tokio::time::timeout(DEADLINE, resumed_client.entered.acquire())
        .await
        .expect("the resumed run reaches the model")
        .expect("model gate")
        .forget();
    assert_eq!(
        tail_run(&start, &id).await,
        Some(run_a.clone()),
        "run A's tail stands while its resumed run waits on the model"
    );

    start
        .machine
        .hard_cancel_current_run(&id, "user interrupt")
        .await
        .expect("the interrupt reaches the resumed run");
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while tail_run(&start, &id).await.is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the cancelled resumed run's tail closes"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let rows = start
        .runtime_store
        .load_input_states(&LogicalRuntimeId::for_session(&id))
        .await
        .expect("read the input rows");
    assert!(!rows.is_empty(), "run A's input has a row");
    for row in &rows {
        let InputStateRow::Decoded(stored) = row else {
            panic!("an input row decodes: {row:?}");
        };
        assert!(
            matches!(
                stored.seed.phase,
                InputLifecycleState::Consumed
                    | InputLifecycleState::Superseded
                    | InputLifecycleState::Coalesced
                    | InputLifecycleState::Abandoned
            ),
            "no input of the resumed run waits to run again: {:?}",
            stored.seed
        );
    }

    let requests_before = resumed_client.requests.load(Ordering::SeqCst);
    resumed_client.push(Step::Text("after the resume"));
    let outcome = turn(&start, &id, "again").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "after the resume"),
        "the next turn completes: {outcome:?}"
    );
    assert_eq!(
        resumed_client.requests.load(Ordering::SeqCst),
        requests_before + 1,
        "only the new turn calls the model"
    );
    assert_eq!(
        committed_rows(&start, &id).await,
        history_with_closed_run_a("again", "after the resume"),
        "run A is closed once, and its input does not run again"
    );
}

#[tokio::test]
async fn an_interrupt_of_a_run_a_restart_resumed_closes_it_and_the_next_turn_completes_head_canonical()
 {
    an_interrupt_of_a_run_a_restart_resumed_closes_it_and_the_next_turn_completes(true).await;
}

#[tokio::test]
async fn an_interrupt_of_a_run_a_restart_resumed_closes_it_and_the_next_turn_completes_whole_blob()
{
    an_interrupt_of_a_run_a_restart_resumed_closes_it_and_the_next_turn_completes(false).await;
}

// ---------------------------------------------------------------------------
// A live mutation issued while the session's run holds a round commit.
// ---------------------------------------------------------------------------

/// A host stages a tool filter on a session whose run holds a round commit.
/// The mutation does not fail at once (0.9.4 queued it behind the turn): it
/// lands once the run has ended, and the session committed after the run
/// carries it, while the run completes once.
async fn a_mutation_issued_mid_run_lands_after_the_run(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let start = stores.open(&root, &client);
    let id = materialize_session(&start, meerkat_core::Session::new()).await;
    let (turn_a, _run_a) = run_a_held_after_a_round_commit(&start, &client, &id).await;

    // An empty allow-set (deny every tool) names no tool, so the
    // visibility machine's catalog witnesses never refuse it.
    let filter = meerkat_core::ToolFilter::Allow(std::iter::empty::<&str>().collect());
    let mutation = tokio::spawn({
        let service = start.service.clone();
        let id = id.clone();
        let filter = filter.clone();
        async move { service.set_session_tool_filter(&id, filter).await }
    });
    // The mutation meets the run's round commit before the run ends, and
    // waits behind the run's lap: it has not settled when the run is
    // released.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !mutation.is_finished(),
        "the mutation settled before the run ended"
    );
    client.release.add_permits(1);

    let outcome_a = tokio::time::timeout(DEADLINE, turn_a)
        .await
        .expect("run A ends")
        .expect("turn A task");
    assert!(
        matches!(outcome_a, Some(CompletionOutcome::Completed(ref run)) if run.text == "held answer"),
        "run A completes: {outcome_a:?}"
    );
    let mutated = tokio::time::timeout(DEADLINE, mutation)
        .await
        .expect("the mutation settles")
        .expect("the mutation task");
    assert!(
        mutated.is_ok(),
        "the mutation lands after the run: {mutated:?}"
    );
    let committed = start
        .service
        .load_authoritative_session(&id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    let visibility = committed
        .try_tool_visibility_state()
        .expect("the visibility state decodes")
        .expect("the visibility state is committed");
    assert!(
        visibility.staged_filter == filter || visibility.active_filter == filter,
        "the committed session carries the mutation: {visibility:?}"
    );
    assert_eq!(
        committed_rows(&start, &id).await,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant held answer".to_string(),
        ],
        "run A committed once"
    );
}

#[tokio::test]
async fn a_mutation_issued_mid_run_lands_after_the_run_head_canonical() {
    a_mutation_issued_mid_run_lands_after_the_run(true).await;
}

#[tokio::test]
async fn a_mutation_issued_mid_run_lands_after_the_run_whole_blob() {
    a_mutation_issued_mid_run_lands_after_the_run(false).await;
}

// ---------------------------------------------------------------------------
// A resumed run whose tool-process custody evidence cannot be read.
// ---------------------------------------------------------------------------

/// Custody evidence the attachment reads when it settles the session, and
/// that then cannot be read any more: the resumed dispatch meets a store
/// that fails.
struct EvidenceReadableOnce {
    reads: std::sync::atomic::AtomicUsize,
    acknowledged: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl meerkat_core::tool_process::InterruptedToolEvidence for EvidenceReadableOnce {
    async fn interrupted_calls(
        &self,
    ) -> Result<
        Vec<meerkat_core::tool_process::InterruptedToolCall>,
        meerkat_core::tool_process::InterruptedToolEvidenceError,
    > {
        if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Vec::new())
        } else {
            Err(meerkat_core::tool_process::InterruptedToolEvidenceError {
                reason: "the custody store cannot be read".to_string(),
            })
        }
    }

    async fn mark_inputs_settled(
        &self,
        _entry_ids: &[uuid::Uuid],
        _inputs: &meerkat_core::tool_process::InterruptedRunInputs,
    ) -> Result<(), meerkat_core::tool_process::InterruptedToolEvidenceError> {
        Ok(())
    }

    async fn acknowledge(
        &self,
        _entry_ids: &[uuid::Uuid],
    ) -> Result<(), meerkat_core::tool_process::InterruptedToolEvidenceError> {
        self.acknowledged.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn run_ended(
        &self,
        _run_id: &RunId,
    ) -> Result<(), meerkat_core::tool_process::InterruptedToolEvidenceError> {
        Ok(())
    }
}

struct EvidenceReadableOnceSource(Arc<EvidenceReadableOnce>);

#[async_trait]
impl meerkat_core::tool_process::InterruptedToolEvidenceSource for EvidenceReadableOnceSource {
    async fn settle_session(
        &self,
        _session_id: &SessionId,
    ) -> Result<
        Option<Arc<dyn meerkat_core::tool_process::InterruptedToolEvidence>>,
        meerkat_core::tool_process::InterruptedToolEvidenceError,
    > {
        Ok(Some(self.0.clone()))
    }
}

/// A restart resumes run A, whose round may have run tool calls, but the
/// session's tool-process custody evidence cannot be read when the resumed
/// round asks for it. The run is held, never replayed: the executor stops,
/// run A's provisional tail stands, its input stays with run A (the stop
/// ends it, nothing requeues it), and the model is not asked again, so
/// nothing the round did is repeated. The next start repairs the run from
/// its tail as interrupted.
async fn an_unreadable_custody_store_holds_the_resumed_run(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    let (_turn_a, run_a) = run_a_held_after_a_round_commit(&first, &client, &id).await;
    // The power goes out while run A waits on the model.
    let after_the_cut = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_cut);

    let resumed_client = Arc::new(ScriptedModel::new());
    // A replay of run A's input would reach this model.
    resumed_client.push(Step::Text("replayed"));
    let start = restarted.open(&after_the_cut, &resumed_client);
    let evidence = Arc::new(EvidenceReadableOnce {
        reads: std::sync::atomic::AtomicUsize::new(0),
        acknowledged: std::sync::atomic::AtomicUsize::new(0),
    });
    start
        .machine
        .set_interrupted_tool_evidence_source(Arc::new(EvidenceReadableOnceSource(
            evidence.clone(),
        )));
    materialize_session(&start, restarted.stored_session(&start, &id).await).await;

    // The resumed dispatch reads the evidence after the attachment did, fails
    // to, and stops the executor.
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while start
        .machine
        .session_has_live_executor_attachment(&id)
        .await
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the resumed run is held: its executor stops"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        evidence.reads.load(Ordering::SeqCst) >= 2,
        "the resumed dispatch asked for the evidence after the attachment"
    );
    assert_eq!(
        tail_run(&start, &id).await,
        Some(run_a.clone()),
        "run A's provisional tail is held, not discarded"
    );
    let rows = start
        .runtime_store
        .load_input_states(&LogicalRuntimeId::for_session(&id))
        .await
        .expect("read the input rows");
    assert!(!rows.is_empty(), "run A's input has a row");
    for row in &rows {
        let InputStateRow::Decoded(stored) = row else {
            panic!("an input row decodes: {row:?}");
        };
        assert!(
            matches!(
                stored.seed.phase,
                InputLifecycleState::Staged
                    | InputLifecycleState::Applied
                    | InputLifecycleState::AppliedPendingConsumption
                    | InputLifecycleState::Abandoned
            ) && stored.seed.last_run_id.as_ref() == Some(&run_a),
            "run A's input stays with run A: not requeued, and consumed by no replay: {:?}",
            stored.seed
        );
    }
    assert_eq!(
        resumed_client.requests.load(Ordering::SeqCst),
        0,
        "nothing replays run A's input: the model is not asked again"
    );
    assert_eq!(
        evidence.acknowledged.load(Ordering::SeqCst),
        0,
        "no evidence is acknowledged for a run that did not end"
    );
}

#[tokio::test]
async fn an_unreadable_custody_store_holds_the_resumed_run_head_canonical() {
    an_unreadable_custody_store_holds_the_resumed_run(true).await;
}

#[tokio::test]
async fn an_unreadable_custody_store_holds_the_resumed_run_whole_blob() {
    an_unreadable_custody_store_holds_the_resumed_run(false).await;
}

// ---------------------------------------------------------------------------
// A presence probe of a session whose resumed run waits on the model.
// ---------------------------------------------------------------------------

/// A host's presence probe meets a live actor that continues its open run's
/// provisional tail (a restart resumed run A, which now waits on the model).
/// The probe classifies live authority from bounded facts and reads no
/// committed body, so it answers at once: the session is live. It neither
/// waits out the run nor settles the tail of a run the runtime holds open.
async fn a_presence_probe_answers_while_a_resumed_run_waits_on_the_model(head_canonical: bool) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    let (_turn_a, run_a) = run_a_held_after_a_round_commit(&first, &client, &id).await;
    // The power goes out while run A waits on the model.
    let after_the_cut = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_cut);

    let resumed_client = Arc::new(ScriptedModel::new());
    resumed_client.push(Step::Held);
    let start = restarted.open(&after_the_cut, &resumed_client);
    materialize_session(&start, restarted.stored_session(&start, &id).await).await;
    tokio::time::timeout(DEADLINE, resumed_client.entered.acquire())
        .await
        .expect("the resumed run reaches the model")
        .expect("model gate")
        .forget();

    let live = tokio::time::timeout(Duration::from_secs(5), start.service.has_live_session(&id))
        .await
        .expect("the presence probe answers while the resumed run waits on the model")
        .expect("the presence probe reads no committed body");
    assert!(live, "the session's live actor runs its resumed run");
    assert_eq!(
        tail_run(&start, &id).await,
        Some(run_a.clone()),
        "the probe leaves the open run's tail to the run"
    );

    resumed_client.release.add_permits(1);
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while tail_run(&start, &id).await.is_some() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the resumed run commits its boundary"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        resumed_client.requests.load(Ordering::SeqCst),
        1,
        "the resumed run asked the model once"
    );
}

#[tokio::test]
async fn a_presence_probe_answers_while_a_resumed_run_waits_on_the_model_head_canonical() {
    a_presence_probe_answers_while_a_resumed_run_waits_on_the_model(true).await;
}

#[tokio::test]
async fn a_presence_probe_answers_while_a_resumed_run_waits_on_the_model_whole_blob() {
    a_presence_probe_answers_while_a_resumed_run_waits_on_the_model(false).await;
}

// ---------------------------------------------------------------------------
// The runtime-backed executor's session check with no actor over a tail.
// ---------------------------------------------------------------------------

/// A content turn's primitive, as the runtime hands one to an executor.
fn content_turn_primitive(note: &str) -> meerkat_core::lifecycle::run_primitive::RunPrimitive {
    use meerkat_core::lifecycle::run_primitive::{
        ConversationAppend, ConversationAppendRole, CoreRenderable, RunApplyBoundary, RunPrimitive,
        RuntimeTurnMetadata, StagedRunInput,
    };
    RunPrimitive::StagedInput(StagedRunInput {
        boundary: RunApplyBoundary::RunStart,
        appends: vec![ConversationAppend {
            runtime_source: None,
            role: ConversationAppendRole::SystemNotice,
            content: CoreRenderable::SystemNotice {
                kind: meerkat_core::types::SystemNoticeKind::Comms,
                body: Some(note.to_string()),
                blocks: Vec::new(),
            },
            identity: None,
        }],
        contributing_input_ids: vec![meerkat_core::lifecycle::InputId::new()],
        turn_metadata: Some(RuntimeTurnMetadata {
            execution_kind: Some(meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn),
            ..Default::default()
        }),
    })
}

/// A runtime-backed executor that applies a turn while no actor serves the
/// session, and while a run's round commit stands over the committed
/// snapshot (a host restarted over it, before anything materializes the
/// session), answers with the session-unavailable teardown it gives with no
/// tail. Its check that the session is still committed and not archived is
/// body-free. RED before, on HeadCanonical: the check read the committed body
/// strictly, met the revision conflict, and reported an apply failure.
async fn an_executor_without_an_actor_over_a_round_commit_reports_the_session_unavailable(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    let (_turn_a, run_a) = run_a_held_after_a_round_commit(&first, &client, &id).await;
    // The power goes out while run A waits on the model.
    let after_the_cut = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_cut);
    let start = restarted.open(&after_the_cut, &Arc::new(ScriptedModel::new()));
    assert_eq!(
        tail_run(&start, &id).await,
        Some(run_a),
        "run A's round commit stands over the committed snapshot"
    );

    let mut executor = crate::surface::default_persistent_executor(
        start.service.clone(),
        start.machine.clone(),
        id.clone(),
    );
    let refused = executor
        .apply(RunId::new(), content_turn_primitive("a turn with no actor"))
        .await
        .expect_err("no actor serves the session");
    assert!(
        matches!(
            refused,
            meerkat_core::lifecycle::core_executor::CoreExecutorError::TeardownRequired {
                reason:
                    meerkat_core::lifecycle::core_executor::CoreExecutorTeardownReason::SessionUnavailable,
                ..
            }
        ),
        "the committed session with no actor tears the executor down as unavailable: {refused:?}"
    );
}

#[tokio::test]
async fn an_executor_without_an_actor_over_a_round_commit_reports_the_session_unavailable_head_canonical()
 {
    an_executor_without_an_actor_over_a_round_commit_reports_the_session_unavailable(true).await;
}

#[tokio::test]
async fn an_executor_without_an_actor_over_a_round_commit_reports_the_session_unavailable_whole_blob()
 {
    an_executor_without_an_actor_over_a_round_commit_reports_the_session_unavailable(false).await;
}

// ---------------------------------------------------------------------------
// An earlier incarnation's open run, met under the session's hosting claim.
// ---------------------------------------------------------------------------

/// The OS-locked hosting paths of a host whose stores live under `root`, as
/// a realm's path authority lays them out.
fn hosting_paths(root: &TempDir) -> meerkat_runtime::HostingPaths {
    let hosting = root.path().join("hosting");
    let delivery = root.path().join("delivery");
    std::fs::create_dir_all(&hosting).expect("create the hosting lock directory");
    std::fs::create_dir_all(&delivery).expect("create the delivery directory");
    meerkat_runtime::HostingPaths {
        hosting_lock_dir: hosting,
        cold_delivery_lock: delivery.join("cold-delivery.lock"),
        database: None,
    }
}

/// The System message the store-only writer appends.
const WRITER_CONTEXT: &str = "context from a store-only writer";

/// A session's transcript after the system prompt, one label per row,
/// appended System messages included.
fn transcript_rows(session: &meerkat_core::Session) -> Vec<String> {
    session
        .messages()
        .iter()
        .enumerate()
        .filter_map(|(index, message)| match message {
            Message::System(_) if index == 0 => None,
            Message::System(system) => Some(format!("system {}", system.content)),
            Message::User(user) => Some(format!("user {}", user.text_content())),
            Message::BlockAssistant(assistant) => Some(
                if assistant
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::ToolUse { .. }))
                {
                    "assistant tool use".to_string()
                } else {
                    format!("assistant {}", assistant.text_blocks().collect::<String>())
                },
            ),
            Message::ToolResults { .. } => Some("tool results".to_string()),
            Message::SystemNotice(notice) => Some(format!("notice {:?}", notice.kind)),
        })
        .collect()
}

async fn committed_transcript(start: &Start, id: &SessionId) -> Vec<String> {
    let session = start
        .service
        .load_authoritative_session(id)
        .await
        .expect("read the committed session")
        .expect("the session is committed");
    transcript_rows(&session)
}

/// Run A's input rows, each with its phase.
async fn run_a_inputs(start: &Start, id: &SessionId, run_a: &RunId) -> Vec<InputLifecycleState> {
    let rows = start
        .runtime_store
        .load_input_states(&LogicalRuntimeId::for_session(id))
        .await
        .expect("read the input rows");
    assert!(!rows.is_empty(), "run A's input has a row");
    rows.iter()
        .map(|row| {
            let InputStateRow::Decoded(stored) = row else {
                panic!("an input row decodes: {row:?}");
            };
            assert_eq!(
                stored.seed.last_run_id.as_ref(),
                Some(run_a),
                "the only input is run A's: {:?}",
                stored.seed
            );
            stored.seed.phase
        })
        .collect()
}

/// A store-only write over the run an earlier incarnation left open. The
/// power goes out while run A waits on the model: its round commit stands as
/// the provisional tail, a run its next host continues. The next host never
/// resumes the session; it appends one System message to it.
///
/// Without proof that no earlier holder can still write (a host whose store
/// takes no cross-process claims), the write recovers the tail as a restart
/// does, which leaves run A to the host that resumes it: the write fails as
/// held, and settles nothing. A host holding the session's cross-process
/// hosting claim has that proof, and a store-only write cannot run run A,
/// so the write first commits run A as interrupted (its round kept, its
/// input consumed), then lands after it. The model is not asked, nothing of
/// run A runs again, and the next turn completes over both.
async fn a_store_only_write_over_an_earlier_incarnations_open_run_commits_it_as_interrupted_first(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    let (_turn_a, run_a) = run_a_held_after_a_round_commit(&first, &client, &id).await;
    // The power goes out while run A waits on the model; each host below
    // opens its own copy of what it left.
    let unproven_root = TempDir::new().expect("temp");
    let unproven_copy = stores.power_cut_copy(&unproven_root);
    let after_the_cut = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_cut);

    let unproven_client = Arc::new(ScriptedModel::new());
    let unproven = unproven_copy.open(&unproven_root, &unproven_client);
    let refused = unproven
        .service
        .append_system_context(&id, AppendSystemContextRequest::from_text(WRITER_CONTEXT))
        .await;
    assert!(
        matches!(
            refused,
            Err(SessionControlError::Session(
                SessionError::DurableTailHeldForRecovery { .. }
            ))
        ),
        "without proof that no earlier holder can still write, run A is left to its next host: {refused:?}"
    );
    assert_eq!(
        tail_run(&unproven, &id).await,
        Some(run_a.clone()),
        "the held write settles nothing"
    );

    let writer_client = Arc::new(ScriptedModel::new());
    writer_client.push(Step::Text("after the write"));
    let writer = restarted.open_hosted(&after_the_cut, &writer_client);
    writer
        .service
        .append_system_context(&id, AppendSystemContextRequest::from_text(WRITER_CONTEXT))
        .await
        .expect("the write commits run A as interrupted, then lands");
    assert_eq!(
        tail_run(&writer, &id).await,
        None,
        "run A's tail is settled"
    );
    assert_eq!(
        committed_transcript(&writer, &id).await,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "notice Generic".to_string(),
            format!("system {WRITER_CONTEXT}"),
        ],
        "run A is committed once as interrupted, and the write lands after it"
    );
    assert_eq!(
        run_a_inputs(&writer, &id, &run_a).await,
        vec![InputLifecycleState::Consumed],
        "run A's input is consumed with its interrupted run, never requeued"
    );
    assert_eq!(
        writer_client.requests.load(Ordering::SeqCst),
        0,
        "the write runs nothing of run A: the model is not asked"
    );

    materialize_session(&writer, restarted.stored_session(&writer, &id).await).await;
    let outcome = turn(&writer, &id, "again").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(ref run)) if run.text == "after the write"),
        "the next turn completes: {outcome:?}"
    );
    assert_eq!(
        writer_client.requests.load(Ordering::SeqCst),
        1,
        "only the new turn calls the model"
    );
    assert_eq!(
        committed_transcript(&writer, &id).await[4..],
        [
            format!("system {WRITER_CONTEXT}"),
            "user again".to_string(),
            "assistant after the write".to_string(),
        ],
        "the next turn follows the write"
    );
}

#[tokio::test]
async fn a_store_only_write_over_an_earlier_incarnations_open_run_commits_it_as_interrupted_first_head_canonical()
 {
    a_store_only_write_over_an_earlier_incarnations_open_run_commits_it_as_interrupted_first(true)
        .await;
}

#[tokio::test]
async fn a_store_only_write_over_an_earlier_incarnations_open_run_commits_it_as_interrupted_first_whole_blob()
 {
    a_store_only_write_over_an_earlier_incarnations_open_run_commits_it_as_interrupted_first(false)
        .await;
}

/// A store-only write to a session a live host serves. Run A waits on the
/// model in the first host, which holds the session's cross-process hosting
/// claim. A second runtime owner over the same stores asks to append a
/// System message: it takes the claim before it reads anything, and is
/// refused at once, typed. Run A's tail stands, and run A then completes
/// once, with no foreign message in its transcript.
async fn a_store_only_write_to_a_session_a_live_host_serves_is_refused_and_settles_nothing(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let host = stores.open_hosted(&root, &client);
    let id = materialize_session(&host, meerkat_core::Session::new()).await;
    let (turn_a, run_a) = run_a_held_after_a_round_commit(&host, &client, &id).await;

    let other_client = Arc::new(ScriptedModel::new());
    let other = stores.open_hosted(&root, &other_client);
    let refused = other
        .service
        .append_system_context(&id, AppendSystemContextRequest::from_text(WRITER_CONTEXT))
        .await;
    assert!(
        matches!(
            refused,
            Err(SessionControlError::Session(
                SessionError::ServedElsewhere { .. }
            ))
        ),
        "the live host keeps the session: {refused:?}"
    );
    assert_eq!(
        tail_run(&host, &id).await,
        Some(run_a),
        "the refused write settles nothing"
    );

    client.release.add_permits(1);
    let outcome_a = tokio::time::timeout(DEADLINE, turn_a)
        .await
        .expect("run A ends")
        .expect("turn A task");
    assert!(
        matches!(outcome_a, Some(CompletionOutcome::Completed(ref run)) if run.text == "held answer"),
        "run A completes: {outcome_a:?}"
    );
    assert_eq!(
        committed_transcript(&host, &id).await,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant held answer".to_string(),
        ],
        "run A committed once, with no foreign message"
    );
    assert_eq!(
        other_client.requests.load(Ordering::SeqCst),
        0,
        "the refused owner asked no model"
    );
}

#[tokio::test]
async fn a_store_only_write_to_a_session_a_live_host_serves_is_refused_and_settles_nothing_head_canonical()
 {
    a_store_only_write_to_a_session_a_live_host_serves_is_refused_and_settles_nothing(true).await;
}

#[tokio::test]
async fn a_store_only_write_to_a_session_a_live_host_serves_is_refused_and_settles_nothing_whole_blob()
 {
    a_store_only_write_to_a_session_a_live_host_serves_is_refused_and_settles_nothing(false).await;
}

/// The claim drive proves nothing for a runtime owner the claim does not
/// cover. Run A waits on the model in a host whose own runtime registers the
/// session and holds its cross-process hosting claim. An embedder runs a
/// writer's claim drive over the session, which would commit run A as
/// interrupted if the claim proved exclusive write authority:
/// - for a second runtime owner over the same stores, which registers
///   nothing, offered the host's claim;
/// - for the host's own runtime owner, which registers the session, with
///   its own claim.
///
/// Each drive decides as without the proof, as a restart would: run A is a
/// run its host continues, so the drive answers Continue and writes
/// nothing. Run A's tail stands, and run A then completes once, with no
/// interrupted-run notice.
async fn a_claim_drive_proves_nothing_against_its_own_runtimes_registration(head_canonical: bool) {
    use meerkat_runtime::recovery::{
        DurableTailDriveRole, DurableTailRecoveryOutcome, recover_durable_tail_under_hosting_claim,
    };
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let host = stores.open_hosted(&root, &client);
    let id = materialize_session(&host, meerkat_core::Session::new()).await;
    let (turn_a, run_a) = run_a_held_after_a_round_commit(&host, &client, &id).await;

    let claim = host
        .service
        .grant_session_hosting(&id)
        .expect("the host's own hosting claim");
    assert!(claim.is_cross_process(), "the claim is an OS lock");

    let other_client = Arc::new(ScriptedModel::new());
    let other = stores.open_hosted(&root, &other_client);
    let (outcome, _) = recover_durable_tail_under_hosting_claim(
        other.runtime_store.as_ref(),
        &id,
        &other.machine,
        &claim,
        DurableTailDriveRole::Writer,
    )
    .await
    .expect("the drive answers");
    assert!(
        matches!(outcome, DurableTailRecoveryOutcome::Continue { ref run_id, .. } if *run_id == run_a),
        "another runtime owner's claim proves nothing for this one: {outcome:?}"
    );

    let (outcome, _) = recover_durable_tail_under_hosting_claim(
        host.runtime_store.as_ref(),
        &id,
        &host.machine,
        &claim,
        DurableTailDriveRole::Writer,
    )
    .await
    .expect("the drive answers");
    drop(claim);
    assert!(
        matches!(outcome, DurableTailRecoveryOutcome::Continue { ref run_id, .. } if *run_id == run_a),
        "the claim proves nothing against its own runtime's registration: {outcome:?}"
    );
    assert_eq!(
        tail_run(&host, &id).await,
        Some(run_a),
        "run A's tail stands"
    );

    client.release.add_permits(1);
    let outcome_a = tokio::time::timeout(DEADLINE, turn_a)
        .await
        .expect("run A ends")
        .expect("turn A task");
    assert!(
        matches!(outcome_a, Some(CompletionOutcome::Completed(ref run)) if run.text == "held answer"),
        "run A completes: {outcome_a:?}"
    );
    assert_eq!(
        committed_transcript(&host, &id).await,
        vec![
            "user first".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant held answer".to_string(),
        ],
        "run A committed once, with no interrupted-run notice"
    );
    assert_eq!(
        other_client.requests.load(Ordering::SeqCst),
        0,
        "the second runtime owner asked no model"
    );
}

#[tokio::test]
async fn a_claim_drive_proves_nothing_against_its_own_runtimes_registration_head_canonical() {
    a_claim_drive_proves_nothing_against_its_own_runtimes_registration(true).await;
}

#[tokio::test]
async fn a_claim_drive_proves_nothing_against_its_own_runtimes_registration_whole_blob() {
    a_claim_drive_proves_nothing_against_its_own_runtimes_registration(false).await;
}

// ---------------------------------------------------------------------------
// The run of a prompt a graceful stop kept, cut by a power cut.
// ---------------------------------------------------------------------------

/// The max age of the graceful stop that keeps the prompt. It spans the
/// prompt's admission, the stop, host 1's shutdown, the file copy and host 2's
/// open up to its pre-serve age check, so a slow runner still runs the prompt
/// inside it; the start after the power cut waits it out, idle.
const KEPT_PROMPT_MAX_AGE: Duration = Duration::from_secs(30);

/// The kept prompt's row, as `start`'s runtime store holds it.
async fn kept_prompt_row(
    start: &Start,
    id: &SessionId,
    kept: &meerkat_core::InputId,
) -> meerkat_runtime::input_state::StoredInputState {
    start
        .runtime_store
        .load_input_state(&LogicalRuntimeId::for_session(id), kept)
        .await
        .expect("read the kept prompt's row")
        .expect("the kept prompt's row is durable")
}

/// A graceful stop keeps a queued prompt for the next start (the requeue
/// disposition), and the next start, inside the stop's max age, runs it as an
/// ordinary run, run B. The power goes out while run B waits on the model
/// after its tool round's commit, and the next host opens the session once
/// the stop's max age, counted from the prompt's admission, has passed. Run B
/// resumes from its tail: it is the prompt's own run, which started, not a run
/// the stop kept, so its crash is an ordinary one, not an expired stop. Run
/// B's start cleared the stop's mark from the prompt, so the power cut left
/// none. RED before: the mark stayed on the prompt while run B was open, and
/// the host ended run B's input as `RequeueExpired` and closed run B as
/// interrupted instead of resuming it.
async fn the_run_of_a_kept_prompt_resumes_after_a_power_cut_past_the_stops_max_age(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;
    client.push(Step::Text("first answer"));
    let outcome = turn(&first, &id, "first").await;
    assert!(
        matches!(outcome, Some(CompletionOutcome::Completed(_))),
        "the first turn commits: {outcome:?}"
    );

    // Run starts are held, so the prompt is still queued when the graceful
    // stop comes, as a prompt queued behind a running turn is.
    let hold = first
        .machine
        .hold_run_starts(&id, meerkat_runtime::RunStartHoldReason::MobStop)
        .await
        .expect("hold the session's run starts");
    assert_eq!(hold.current_run, None, "no run is in flight");
    let prompt = Input::Prompt(PromptInput::new("kept", None));
    let kept = prompt.id().clone();
    let (accepted, _kept_waiter) = first
        .machine
        .accept_input_with_completion(&id, prompt)
        .await
        .expect("the runtime admits the prompt");
    assert!(
        accepted.is_accepted(),
        "the prompt is admitted: {accepted:?}"
    );
    let registration = first
        .machine
        .current_session_registration_witnesses()
        .await
        .into_iter()
        .find(|registration| registration.session_id() == &id)
        .expect("the session's registration");
    let unregistered = first
        .machine
        .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
            &registration,
            meerkat_runtime::UnregisterQueuedInputs::Requeue {
                max_age: KEPT_PROMPT_MAX_AGE,
            },
        )
        .await
        .expect("the graceful stop");
    assert!(unregistered, "the graceful stop ends the registration");
    first.service.try_shutdown().await.expect("shutdown");
    let stopped = kept_prompt_row(&first, &id, &kept).await;
    let admitted_at = stopped
        .state
        .persisted_input
        .as_ref()
        .expect("the kept prompt keeps its payload")
        .header()
        .timestamp;
    let max_age_ms =
        u64::try_from(KEPT_PROMPT_MAX_AGE.as_millis()).expect("the max age fits in milliseconds");
    assert_eq!(
        (
            stopped.seed.phase,
            stopped.seed.terminal_outcome,
            stopped.state.stop_requeue.map(|mark| mark.max_age_ms),
        ),
        (InputLifecycleState::Queued, None, Some(max_age_ms)),
        "the stop keeps the prompt queued and marked for the next start"
    );

    // The next start, inside the max age, runs the prompt: run B commits its
    // tool round and waits on the model.
    let after_the_stop = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_stop);
    let run_b_client = Arc::new(ScriptedModel::new());
    run_b_client.push(Step::Tool);
    run_b_client.push(Step::Held);
    let second = restarted.open(&after_the_stop, &run_b_client);
    materialize_session(&second, restarted.stored_session(&second, &id).await).await;
    let second_open_row = kept_prompt_row(&second, &id, &kept).await;
    assert_ne!(
        second_open_row.seed.terminal_outcome,
        Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
            reason: meerkat_runtime::InputAbandonReason::RequeueExpired,
        }),
        "timing precondition: host 2 must open the session inside the stop's max age, counted \
         from the prompt's admission; it opened later and expired the prompt instead of running it"
    );
    tokio::time::timeout(DEADLINE, run_b_client.entered.acquire())
        .await
        .expect("the start inside the max age runs the kept prompt to its second model call")
        .expect("model gate")
        .forget();
    let run_b = tail_run(&second, &id)
        .await
        .expect("run B's round commit stands as the provisional tail");

    // The power goes out while run B waits on the model.
    let after_the_cut = TempDir::new().expect("temp");
    let cut = restarted.power_cut_copy(&after_the_cut);
    let resumed_client = Arc::new(ScriptedModel::new());
    resumed_client.push(Step::Held);
    let third = cut.open(&after_the_cut, &resumed_client);
    let left_by_the_cut = kept_prompt_row(&third, &id, &kept).await;
    assert_eq!(
        (
            left_by_the_cut.seed.last_run_id.as_ref(),
            left_by_the_cut.seed.terminal_outcome.as_ref(),
        ),
        (Some(&run_b), None),
        "the power cut leaves the prompt bound to run B"
    );

    // The next host opens the session once the stop's max age has passed.
    let past_the_max_age = admitted_at
        + chrono::Duration::from_std(KEPT_PROMPT_MAX_AGE + Duration::from_millis(500))
            .expect("the max age is a chrono duration");
    if let Ok(wait) = (past_the_max_age - chrono::Utc::now()).to_std() {
        tokio::time::sleep(wait).await;
    }
    let opened = try_materialize_session(&third, cut.stored_session(&third, &id).await).await;
    let opened_row = kept_prompt_row(&third, &id, &kept).await;
    assert_eq!(
        opened_row.seed.terminal_outcome, None,
        "run B is not settled as an expired stop: it started, so its crash is an ordinary one"
    );
    opened.expect("the next host opens the session");
    tokio::time::timeout(DEADLINE, resumed_client.entered.acquire())
        .await
        .expect("run B resumes and reaches the model")
        .expect("model gate")
        .forget();
    assert_eq!(
        tail_run(&third, &id).await,
        Some(run_b.clone()),
        "the resumed run continues run B's tail"
    );
    let resumed = kept_prompt_row(&third, &id, &kept).await;
    assert_eq!(
        resumed.seed.last_run_id.as_ref(),
        Some(&run_b),
        "the prompt stays bound to run B"
    );
    assert_eq!(
        (
            left_by_the_cut.state.stop_requeue,
            resumed.state.stop_requeue
        ),
        (None, None),
        "run B's start cleared the stop's mark from its input, so the power cut left none"
    );
}

#[tokio::test]
async fn the_run_of_a_kept_prompt_resumes_after_a_power_cut_past_the_stops_max_age_head_canonical()
{
    the_run_of_a_kept_prompt_resumes_after_a_power_cut_past_the_stops_max_age(true).await;
}

#[tokio::test]
async fn the_run_of_a_kept_prompt_resumes_after_a_power_cut_past_the_stops_max_age_whole_blob() {
    the_run_of_a_kept_prompt_resumes_after_a_power_cut_past_the_stops_max_age(false).await;
}

// ---------------------------------------------------------------------------
// A run a graceful stop kept, admitted long before the restart.
// ---------------------------------------------------------------------------

/// A run in flight at a host's graceful stop, whose prompt the runtime
/// admitted 20 minutes before the stop: longer than the 15-minute max age
/// (`UnregisterQueuedInputs::DEFAULT_MAX_AGE`) of the stop a graceful
/// shutdown makes when its host names no disposition, as `rkat-rpc`'s and
/// the REST server's shutdowns do (`MeerkatMachine::graceful_shutdown_queued_inputs`).
/// The stop suspends the run after its tool round's commit, while it waits on
/// the model, and keeps its input bound to it, marked with the stop's time.
/// The next host starts at once and resumes the run from its tail: the run
/// asks the model once more, keeps its tool round once and completes once,
/// and its input is consumed. RED before: the start measured the max age
/// from the prompt's admission, so it ended the input `RequeueExpired` and
/// closed the run as interrupted instead of resuming it, however short the
/// restart.
async fn a_run_admitted_long_before_a_graceful_restart_resumes_and_completes_once(
    head_canonical: bool,
) {
    let root = TempDir::new().expect("temp");
    let stores = DiskStores::new(head_canonical, &root);
    let client = Arc::new(ScriptedModel::new());
    let first = stores.open(&root, &client);
    let id = materialize_session(&first, meerkat_core::Session::new()).await;

    // The run's prompt, admitted 20 minutes before the stop: the run makes
    // one tool call, commits that round and waits on the model.
    client.push(Step::Tool);
    client.push(Step::Held);
    let mut prompt = PromptInput::new("kept", None);
    prompt.header.timestamp = chrono::Utc::now() - chrono::Duration::minutes(20);
    let prompt = Input::Prompt(prompt);
    let kept = prompt.id().clone();
    let (accepted, kept_waiter) = first
        .machine
        .accept_input_with_completion(&id, prompt)
        .await
        .expect("the runtime admits the prompt");
    assert!(
        accepted.is_accepted(),
        "the prompt is admitted: {accepted:?}"
    );
    tokio::time::timeout(DEADLINE, client.entered.acquire())
        .await
        .expect("the run reaches its second model call")
        .expect("model gate")
        .forget();
    let run = tail_run(&first, &id)
        .await
        .expect("the run's round commit stands as the provisional tail");

    // The host's graceful stop, with the disposition its shutdown uses.
    let queued_inputs = first.machine.graceful_shutdown_queued_inputs();
    assert_eq!(
        queued_inputs,
        meerkat_runtime::UnregisterQueuedInputs::Requeue {
            max_age: meerkat_runtime::UnregisterQueuedInputs::DEFAULT_MAX_AGE,
        },
        "a runtime with a store requeues with the default max age"
    );
    let registration = first
        .machine
        .current_session_registration_witnesses()
        .await
        .into_iter()
        .find(|registration| registration.session_id() == &id)
        .expect("the session's registration");
    let unregistered = first
        .machine
        .unregister_session_registration_until_terminal_with_queued_inputs_if_current(
            &registration,
            queued_inputs,
        )
        .await
        .expect("the graceful stop");
    assert!(unregistered, "the graceful stop ends the registration");
    let waited = tokio::time::timeout(DEADLINE, kept_waiter.expect("the prompt's waiter").wait())
        .await
        .expect("the prompt's waiter is answered");
    assert!(
        matches!(&waited, Err(error) if error.is_requeued_across_stop()),
        "the stop keeps the run: {waited:?}"
    );
    first.service.try_shutdown().await.expect("shutdown");
    let stopped = kept_prompt_row(&first, &id, &kept).await;
    assert_eq!(
        (
            stopped.seed.last_run_id.as_ref(),
            stopped.seed.terminal_outcome.as_ref(),
            stopped.state.stop_requeue.map(|mark| mark.max_age_ms),
        ),
        (Some(&run), None, Some(900_000)),
        "the stop keeps the prompt bound to its run, marked with the default max age"
    );

    // The next host starts at once.
    let after_the_stop = TempDir::new().expect("temp");
    let restarted = stores.power_cut_copy(&after_the_stop);
    let resumed_client = Arc::new(ScriptedModel::new());
    resumed_client.push(Step::Text("kept answer"));
    let second = restarted.open(&after_the_stop, &resumed_client);
    let opened =
        try_materialize_session(&second, restarted.stored_session(&second, &id).await).await;
    let deadline = tokio::time::Instant::now() + DEADLINE;
    let settled = loop {
        let row = kept_prompt_row(&second, &id, &kept).await;
        if row.seed.terminal_outcome.is_some() || tokio::time::Instant::now() >= deadline {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        (settled.seed.phase, settled.seed.terminal_outcome),
        (
            InputLifecycleState::Consumed,
            Some(meerkat_runtime::InputTerminalOutcome::Consumed),
        ),
        "the restart resumes the run, and its input is consumed: a prompt admitted 20 minutes \
         before a stop that kept it moments ago is not expired"
    );
    opened.expect("the next host opens the session");
    assert_eq!(
        resumed_client.requests.load(Ordering::SeqCst),
        1,
        "the resumed run asks the model once"
    );
    assert_eq!(
        committed_rows(&second, &id).await,
        vec![
            "user kept".to_string(),
            "assistant tool use".to_string(),
            "tool results".to_string(),
            "assistant kept answer".to_string(),
        ],
        "the run completes once, its tool round kept once, with no interrupted-run notice"
    );
}

#[tokio::test]
async fn a_run_admitted_long_before_a_graceful_restart_resumes_and_completes_once_head_canonical() {
    a_run_admitted_long_before_a_graceful_restart_resumes_and_completes_once(true).await;
}

#[tokio::test]
async fn a_run_admitted_long_before_a_graceful_restart_resumes_and_completes_once_whole_blob() {
    a_run_admitted_long_before_a_graceful_restart_resumes_and_completes_once(false).await;
}
