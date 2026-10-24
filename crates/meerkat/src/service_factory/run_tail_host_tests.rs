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
use meerkat_core::service::{DeferredPromptPolicy, InitialTurnPolicy, SessionService};
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

    fn runtime_store(&self) -> Arc<dyn RuntimeStore> {
        match self {
            Self::HeadCanonical { path } => Arc::new(
                meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(path.clone())
                    .expect("open the head-canonical runtime store"),
            ),
            Self::WholeBlob { dir } => {
                std::fs::create_dir_all(dir).expect("create the realm directory");
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                        dir.join("runtime.sqlite3"),
                    )
                    .expect("open the whole-blob runtime store"),
                )
            }
        }
    }

    /// One process start over the stores.
    fn open(&self, root: &TempDir, client: &Arc<ScriptedModel>) -> Start {
        let persistence = crate::PersistenceBundle::new(
            self.session_store(),
            self.runtime_store(),
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
    .await
    .expect("materialize the session and its executor");
    created.session_id
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
