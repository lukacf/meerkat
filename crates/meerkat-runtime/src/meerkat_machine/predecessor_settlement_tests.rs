//! Machine rows for the cold-predecessor settlement: the claim, profile and
//! registration refusals, the row scan, the placement witness, the marker on
//! the internal registration, its exact removal and the process-owned task;
//! then whole settlements over a HeadCanonical durable tail, driven step by
//! step as the session service drives them, with a recording publisher in
//! place of the service's stored-only one (the service's own rows are the
//! session crate's).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::input::Input;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord};
use crate::session_hosting::HostingPaths;
use crate::store::RuntimeStore;
use meerkat_core::lifecycle::core_executor::{
    CoreExecutorError, CoreExecutorPublicationHandle, CoreInteractionTerminalPublicationReceipt,
};

const BOUND: std::time::Duration = std::time::Duration::from_secs(10);

fn deadline() -> meerkat_core::time_compat::Instant {
    meerkat_core::time_compat::Instant::now() + BOUND
}

fn memory_blob_store() -> Arc<dyn meerkat_core::BlobStore> {
    Arc::new(meerkat_store::MemoryBlobStore::new())
}

/// A HeadCanonical store whose sessions are hosted under cross-process OS
/// locks, and its machine.
struct Host {
    _dir: tempfile::TempDir,
    store: Arc<crate::store::SqliteRuntimeStore>,
    machine: MeerkatMachine,
}

impl Host {
    fn head_canonical() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            crate::store::SqliteRuntimeStore::new_head_canonical(
                dir.path().join("runtime.sqlite3"),
            )
            .unwrap()
            .with_hosting_paths(HostingPaths {
                hosting_lock_dir: dir.path().join("hosting"),
                cold_delivery_lock: dir.path().join("delivery").join("cold-delivery.lock"),
                database: None,
            }),
        );
        let machine = MeerkatMachine::persistent(
            Arc::clone(&store) as Arc<dyn RuntimeStore>,
            memory_blob_store(),
        )
        .unwrap();
        Self {
            _dir: dir,
            store,
            machine,
        }
    }

    /// The same stores as a new process opens them: the runtime store
    /// reopened from its file with the same hosting paths, and a new machine
    /// that holds nothing this one held in memory.
    fn reopened(self) -> Self {
        let Self {
            _dir,
            store,
            machine,
        } = self;
        drop(machine);
        let path = store.path().to_path_buf();
        drop(store);
        let store = Arc::new(
            crate::store::SqliteRuntimeStore::new_head_canonical(path)
                .unwrap()
                .with_hosting_paths(HostingPaths {
                    hosting_lock_dir: _dir.path().join("hosting"),
                    cold_delivery_lock: _dir.path().join("delivery").join("cold-delivery.lock"),
                    database: None,
                }),
        );
        let machine = MeerkatMachine::persistent(
            Arc::clone(&store) as Arc<dyn RuntimeStore>,
            memory_blob_store(),
        )
        .unwrap();
        Self {
            _dir,
            store,
            machine,
        }
    }

    fn claim(&self, session_id: &SessionId) -> HostingClaim {
        let claim = self
            .machine
            .grant_session_hosting(session_id)
            .expect("the host grants its own claim");
        assert!(claim.is_cross_process());
        claim
    }

    async fn persist(&self, session_id: &SessionId, stored: StoredInputState) {
        self.store
            .persist_input_state(
                &LogicalRuntimeId::for_session(session_id),
                &InputStatePersistenceRecord::stored_row_for_test(stored).expect("a stored row"),
            )
            .await
            .expect("the row is written");
    }

    fn write_corrupt_row(&self, session_id: &SessionId, key: &str) {
        let conn = rusqlite::Connection::open(self.store.path()).unwrap();
        conn.execute(
            "INSERT INTO runtime_input_states (runtime_id, input_id, state_json)
             VALUES (?1, ?2, ?3)",
            rusqlite::params![
                LogicalRuntimeId::for_session(session_id).0,
                key,
                b"{\"not\":\"an input state\"}".as_slice()
            ],
        )
        .unwrap();
    }
}

fn prompt(text: &str) -> Input {
    Input::Prompt(crate::input::PromptInput::new(text, None))
}

fn directed_step(label: &str) -> Input {
    crate::mob_adapter::create_tracked_flow_step_input(
        label,
        meerkat_core::types::ContentInput::Text(format!("directed {label}")),
        "predecessor-settlement-flow",
        None,
        &uuid::Uuid::new_v4().to_string(),
    )
    .expect("a directed flow step")
}

/// A row of `input` as a predecessor left it: `phase`, with the run and
/// boundary it names.
fn row(
    input: &Input,
    phase: InputLifecycleState,
    run_id: Option<RunId>,
    boundary_sequence: Option<u64>,
) -> StoredInputState {
    let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(crate::input::InputDurability::Durable);
    let policy = crate::DefaultPolicyTable::resolve(input, true);
    state.runtime_semantics = Some(
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
            .expect("generated admission semantics"),
    );
    state.policy = Some(crate::input_state::PolicySnapshot {
        version: policy.policy_version,
        decision: policy,
    });
    let terminal = phase == InputLifecycleState::Consumed;
    StoredInputState {
        state,
        seed: crate::input_state::InputStateSeed {
            phase,
            last_run_id: run_id,
            last_boundary_sequence: boundary_sequence,
            terminal_outcome: terminal.then_some(InputTerminalOutcome::Consumed),
            attempt_count: u32::from(terminal),
            admission_sequence: None,
            recovery_lane: (!terminal).then_some(meerkat_core::types::HandlingMode::Queue),
        },
    }
}

/// A stored-only publisher that records what it publishes.
#[derive(Default)]
struct RecordingPublisher {
    published: std::sync::Mutex<Vec<meerkat_core::event::AgentEvent>>,
}

impl RecordingPublisher {
    fn events(&self) -> Vec<meerkat_core::event::AgentEvent> {
        self.published.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for RecordingPublisher {
    async fn publish_interaction_terminals(
        &self,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        let mut published = self.published.lock().unwrap();
        events
            .iter()
            .map(|event| {
                published.push(event.clone());
                CoreInteractionTerminalPublicationReceipt::try_new(event, published.len() as u64)
            })
            .collect()
    }
}

/// Only this machine's own cross-process claim of the session settles
/// it. Another machine's claim is refused `ClaimNotOwned`, and a claim with
/// no cross-process OS lock `ExclusivityUnproven`; nothing is touched.
#[tokio::test]
async fn a_settlement_refuses_another_owners_claim_and_a_claim_without_an_os_lock() {
    let host = Host::head_canonical();
    let other = Host::head_canonical();
    let session_id = SessionId::new();
    let foreign = other.claim(&session_id);
    assert!(matches!(
        host.machine
            .begin_predecessor_settlement_before(&foreign, deadline())
            .await,
        Err(PredecessorSettlementRefusal::ClaimNotOwned)
    ));

    let dir = tempfile::tempdir().unwrap();
    let unlocked: Arc<dyn RuntimeStore> = Arc::new(
        crate::store::SqliteRuntimeStore::new_head_canonical(dir.path().join("runtime.sqlite3"))
            .unwrap(),
    );
    let machine = MeerkatMachine::persistent(unlocked, memory_blob_store()).unwrap();
    let unlocked_claim = machine
        .grant_session_hosting(&session_id)
        .expect("a store without OS locks still grants its own claim");
    assert!(!unlocked_claim.is_cross_process());
    assert!(matches!(
        machine
            .begin_predecessor_settlement_before(&unlocked_claim, deadline())
            .await,
        Err(PredecessorSettlementRefusal::ExclusivityUnproven)
    ));
    assert!(!host.machine.contains_session(&session_id).await);
    assert!(!machine.contains_session(&session_id).await);
}

/// A WholeBlob store refuses the settlement before anything is
/// touched.
#[tokio::test]
async fn a_whole_blob_store_refuses_the_settlement_before_anything_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(
        crate::store::SqliteRuntimeStore::new(dir.path().join("runtime.sqlite3"))
            .unwrap()
            .with_hosting_paths(HostingPaths {
                hosting_lock_dir: dir.path().join("hosting"),
                cold_delivery_lock: dir.path().join("delivery").join("cold-delivery.lock"),
                database: None,
            }),
    );
    let machine = MeerkatMachine::persistent(store, memory_blob_store()).unwrap();
    let session_id = SessionId::new();
    let claim = machine.grant_session_hosting(&session_id).unwrap();
    assert!(matches!(
        machine
            .begin_predecessor_settlement_before(&claim, deadline())
            .await,
        Err(PredecessorSettlementRefusal::ProfileUnsupported { .. })
    ));
    assert!(!machine.contains_session(&session_id).await);
}

/// A live registration of the session refuses the settlement
/// `Registered`, and the registration is untouched.
#[tokio::test]
async fn a_registered_session_refuses_the_settlement() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    host.machine
        .register_session(session_id.clone())
        .await
        .expect("the session registers under the host's lineage");
    assert!(matches!(
        host.machine
            .begin_predecessor_settlement_before(&claim, deadline())
            .await,
        Err(PredecessorSettlementRefusal::Registered)
    ));
    assert!(host.machine.contains_session(&session_id).await);
}

/// A row that does not decode, and a consumed row with a run whose
/// boundary receipt is not durable, are rows the settlement cannot
/// account for. It refuses `UnsettleableRows` naming them, before anything
/// is touched or registered; the rows are unchanged.
#[tokio::test]
async fn rows_the_settlement_cannot_account_for_refuse_it_untouched() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    host.write_corrupt_row(&session_id, "zz-undecodable-row");
    let consumed = prompt("consumed without a boundary");
    host.persist(
        &session_id,
        row(
            &consumed,
            InputLifecycleState::Consumed,
            Some(RunId::new()),
            Some(1),
        ),
    )
    .await;

    match host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
    {
        Err(PredecessorSettlementRefusal::UnsettleableRows {
            undecodable,
            unproven,
        }) => {
            assert_eq!(undecodable, vec!["zz-undecodable-row".to_string()]);
            assert_eq!(unproven, vec![consumed.id().clone()]);
        }
        other => panic!("the settlement refuses the rows it cannot account for: {other:?}"),
    }
    assert!(!host.machine.contains_session(&session_id).await);
    let rows = host
        .store
        .load_input_states(&LogicalRuntimeId::for_session(&session_id))
        .await
        .unwrap();
    assert_eq!(rows.len(), 2, "both rows stay as they were");
}

/// A queued directed input would need a new outbox, and the session's
/// lifecycle row records neither a complete binding nor a placement
/// witness: the settlement refuses `PlacementWitnessMissing` naming it,
/// before anything is touched.
#[tokio::test]
async fn directed_work_without_a_placement_witness_refuses_the_settlement() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    let directed = directed_step("queued without a placement");
    host.persist(
        &session_id,
        row(&directed, InputLifecycleState::Queued, None, None),
    )
    .await;

    match host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
    {
        Err(PredecessorSettlementRefusal::PlacementWitnessMissing { directed: named }) => {
            assert_eq!(named, vec![directed.id().clone()]);
        }
        other => panic!("directed work without a placement witness refuses: {other:?}"),
    }
    assert!(!host.machine.contains_session(&session_id).await);
}

/// The internal registration's marker, its exact removal and registrable
/// afterwards. While the settlement's registration stands, the session's
/// entry is the settlement's, and every taker that reads it (a
/// registration, an adoption, an admission, a lifecycle mutation, an
/// unregister) is refused `UnregisterInProgress`; once finish's task ran,
/// the session holds no registration, the census found nothing owed, and a
/// fresh registration inserts in the caller's lineage.
#[tokio::test]
async fn the_settlement_registration_refuses_every_taker_and_leaves_exactly() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the internal registration publishes and sweeps");
    assert_eq!(work.owner_loss, PredecessorOwnerLoss::NoConsentOwner);
    assert!(work.ended.is_empty());
    assert!(work.answered.is_empty());
    assert!(lease.registered());
    assert!(host.machine.contains_session(&session_id).await);

    // The lease holds the registration transaction and the mutation gate
    // here, so the takers are proven where every one of them reads the
    // entry.
    {
        let sessions = host.machine.sessions.read().await;
        let entry = sessions
            .get(&session_id)
            .expect("the settlement's registration stands");
        assert!(entry.is_predecessor_settlement());
        assert!(matches!(
            entry.registration_blocked_by_unregister(&session_id),
            Some(RuntimeDriverError::UnregisterInProgress { .. })
        ));
        assert!(matches!(
            entry.dsl_mutation_blocked_by_unregister(&session_id),
            Some(RuntimeDriverError::UnregisterInProgress { .. })
        ));
    }

    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::new(RecordingPublisher::default()),
            deadline(),
        )
        .await
        .expect("the task drains, finds every row closed and removes");
    assert!(publication.converged.is_empty());
    assert!(publication.owed.is_empty());
    assert_eq!(publication.durable_rows_closed, 0);
    assert!(!host.machine.contains_session(&session_id).await);

    host.machine
        .register_session(session_id.clone())
        .await
        .expect("the session is registrable afterwards");
    assert_eq!(
        host.machine
            .registration_claim_same_lineage(&session_id, &claim)
            .await,
        Some(true),
        "the fresh registration joined the caller's lineage"
    );
}

/// A lease dropped after its internal registration published hands
/// the exact removal to the process-owned task, and a later settlement
/// waits for that task, holding no gate, before it begins.
#[tokio::test]
async fn a_dropped_lease_hands_its_removal_to_the_task_and_a_repeat_waits_for_it() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the internal registration publishes");
    drop(lease);

    let repeat = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the repeat begins once the dropped lease's task removed the registration");
    assert!(!repeat.registered());
    assert!(!host.machine.contains_session(&session_id).await);
}

/// A queued nondirected input the predecessor left ends `Abandoned {
/// Stopped }` in the sweep, with its runless receipt, which the task
/// finalizes: the census lists it `Ended`, with no publication, and finds
/// its row closed.
#[tokio::test]
async fn a_queued_nondirected_input_is_ended_stopped_with_its_runless_receipt() {
    let host = Host::head_canonical();
    let session_id = SessionId::new();
    let claim = host.claim(&session_id);
    let queued = prompt("queued by the predecessor");
    let mut queued_row = row(&queued, InputLifecycleState::Queued, None, None);
    queued_row.seed.admission_sequence = Some(1);
    host.persist(&session_id, queued_row).await;

    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep ends the queued input");
    assert_eq!(work.ended, vec![queued.id().clone()]);
    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::new(RecordingPublisher::default()),
            deadline(),
        )
        .await
        .expect("the task finalizes the receipt and finds every row closed");
    let [settled] = publication.converged.as_slice() else {
        panic!("one input settled: {:?}", publication.converged);
    };
    assert_eq!(&settled.input_id, queued.id());
    assert_eq!(settled.settled_by, SettledBy::Ended);
    assert_eq!(settled.published_terminal_seq, None);
    assert_eq!(
        settled.receipt.terminal(),
        &InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped,
        }
    );
    assert_eq!(settled.receipt.run_id(), None);
    assert_eq!(publication.durable_rows_closed, 1);
    assert!(publication.owed.is_empty());
    assert!(!host.machine.contains_session(&session_id).await);
}

// ---------------------------------------------------------------------------
// Whole settlements over a HeadCanonical durable tail
// ---------------------------------------------------------------------------

use crate::completion::CompletionOutcome;
use crate::store::{
    MachineLifecycleBindingFacts, MachineLifecycleCommit, MachineLifecycleObservation,
    MachineLifecycleRunFacts, SupervisorAuthoritySnapshot,
};
use crate::terminal_status::InputTerminalReceiptScope;
use meerkat_core::event::{AgentEvent, InteractionFailureReason};
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult,
    TranscriptMessageIdentity, UserMessage, message_timestamp_now,
};

const PLACED_RUNTIME: &str = "predecessor-runtime";
const PLACED_FENCE: u64 = 11;
const PLACED_GENERATION: u64 = 3;
const PLACED_EPOCH: &str = "predecessor-epoch";

/// The placement of the runtime that hosted the session before.
fn predecessor_placement() -> MachineLifecycleBindingFacts {
    MachineLifecycleBindingFacts::new(
        Some(PLACED_RUNTIME.to_string()),
        Some(PLACED_FENCE),
        Some(PLACED_GENERATION),
        Some(PLACED_EPOCH.to_string()),
    )
}

impl Host {
    /// The session's lifecycle row as a quiescent runtime left it: Idle,
    /// with `binding`, and `witness` as its placement witness.
    async fn lifecycle_row(
        &self,
        session_id: &SessionId,
        binding: MachineLifecycleBindingFacts,
        witness: Option<MachineLifecycleBindingFacts>,
    ) {
        self.store
            .commit_machine_lifecycle(
                &LogicalRuntimeId::for_session(session_id),
                MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
                    crate::runtime_state::RuntimeState::Idle,
                    binding,
                    MachineLifecycleRunFacts::default(),
                    SupervisorAuthoritySnapshot::UnboundNoReceipt,
                    None,
                    crate::live_execution::LiveBridgeRecoveryImage::default(),
                )
                .with_placement_witness(witness),
                &[],
            )
            .await
            .expect("the lifecycle row is written");
    }

    async fn rows(&self, session_id: &SessionId) -> Vec<StoredInputState> {
        self.store
            .load_input_states_strict(&LogicalRuntimeId::for_session(session_id))
            .await
            .expect("the session's input rows load")
    }

    /// The placement witness the session's lifecycle row carries.
    async fn placement_witness(
        &self,
        session_id: &SessionId,
    ) -> Option<MachineLifecycleBindingFacts> {
        match self
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(session_id))
            .await
            .expect("the lifecycle row reads")
        {
            MachineLifecycleObservation::Decoded { record, .. } => {
                record.placement_witness().cloned()
            }
            other => panic!("the lifecycle row decodes: {other:?}"),
        }
    }

    async fn provisional_tail_stands(&self, session_id: &SessionId) -> bool {
        self.store
            .load_head_canonical_provisional_tail(&LogicalRuntimeId::for_session(session_id))
            .await
            .expect("the provisional tail reads")
            .is_some()
    }
}

/// The tail a run leaves when it answered and its end-of-run save landed
/// but its boundary did not.
fn completed_tail(mut session: meerkat_core::Session, run_id: &RunId) -> meerkat_core::Session {
    session.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "the answer".to_string(),
            meta: None,
        }],
        stop_reason: Some(StopReason::EndTurn),
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: message_timestamp_now(),
        assistant_message_id: None,
    }));
    session
}

/// The tail a run leaves when it was cut after a tool round: its call is
/// answered, but no final answer and no boundary followed.
fn interrupted_tail(mut session: meerkat_core::Session, run_id: &RunId) -> meerkat_core::Session {
    session.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "cut-call".to_string(),
            name: "cut_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        stop_reason: Some(StopReason::ToolUse),
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: message_timestamp_now(),
        assistant_message_id: None,
    }));
    session.push(Message::tool_results(vec![ToolResult::new(
        "cut-call".to_string(),
        "the tool's result".to_string(),
        false,
    )]));
    session
}

/// How a row builds a run's tail from the committed session.
type TailFn = Box<dyn FnOnce(meerkat_core::Session, &RunId) -> meerkat_core::Session>;

/// A session with one committed boundary (its first turn's prompt) on the
/// host's store and the co-located session store. With `tail`, run `run_id`
/// then left `tail` as the store's provisional tail and the physical head,
/// and the process died before the run's boundary commit.
async fn durable_session(
    host: &Host,
    tail: Option<fn(meerkat_core::Session, &RunId) -> meerkat_core::Session>,
) -> (SessionId, RunId) {
    durable_session_built(host, None, tail.map(|tail| Box::new(tail) as TailFn)).await
}

/// [`durable_session`], with a boundary of `left.0` that consumed `left.1`
/// without a receipt committed before the tail (see [`commit_left_boundary`]),
/// and any tail a row builds.
async fn durable_session_built(
    host: &Host,
    left: Option<(&RunId, &[&Input])>,
    tail: Option<TailFn>,
) -> (SessionId, RunId) {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::{IncrementalSessionStore as _, SessionStore as _};

    let session_store = meerkat_store::SqliteSessionStore::open(host.store.path())
        .expect("the co-located session store");
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("the run's prompt")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let root = PreparedHeadCanonicalMutation::prepare(&committed, None)
        .expect("the committed head prepares");
    let document = BoundSessionCommit::sealed(Arc::new(committed))
        .expect("the committed document seals")
        .with_head_canonical_mutation(root)
        .expect("the committed head binds");
    let mut committed_authority = host
        .store
        .commit_prepared_session_boundary(
            &runtime_id,
            crate::store::PreparedRuntimeSessionCommit::snapshot_only(document),
        )
        .await
        .expect("the first boundary commits")
        .authority()
        .and_then(|authority| authority.head_canonical())
        .cloned()
        .expect("the boundary returns HeadCanonical authority");
    if let Some((left_run, inputs)) = left {
        commit_left_boundary(host, &session_id, left_run, inputs).await;
        committed_authority = host
            .store
            .load_session_boundary_authority(&runtime_id)
            .await
            .expect("the session's authority reads")
            .expect("the session has a committed authority")
            .head_canonical()
            .cloned()
            .expect("HeadCanonical authority");
    }
    let run_id = RunId::new();
    if let Some(tail) = tail {
        let base = session_store
            .load(&session_id)
            .await
            .expect("the committed session loads")
            .expect("the committed session exists");
        let candidate = tail(base, &run_id);
        let observed = session_store
            .load_head(&session_id)
            .await
            .expect("the committed head loads")
            .expect("the committed head exists");
        let mutation = PreparedHeadCanonicalMutation::prepare(&candidate, Some(observed))
            .expect("the run's tail prepares");
        host.store
            .write_prepared_head_canonical_provisional_tail(
                &runtime_id,
                crate::store::PreparedHeadCanonicalProvisionalTail::prepare(
                    committed_authority,
                    run_id.clone(),
                    mutation.successor_head(),
                    mutation.successor_head_token(),
                    &candidate,
                )
                .expect("the provisional tail prepares"),
            )
            .await
            .expect("the provisional tail is written");
        session_store
            .apply_prepared_head_canonical_mutation(&mutation)
            .await
            .expect("the physical head moves past the boundary");
    }
    (session_id, run_id)
}

/// A boundary of `left_run` that consumed `inputs` and staged no receipt for
/// them, as a recovery commit before this release left it: its receipt
/// (sequence 1) and each input's row, consumed by that run at that
/// sequence with no completion and no outbox, a directed one keeping its
/// payload.
async fn commit_left_boundary(
    host: &Host,
    session_id: &SessionId,
    left_run: &RunId,
    inputs: &[&Input],
) {
    host.store
        .commit_prepared_session_boundary(
            &LogicalRuntimeId::for_session(session_id),
            crate::store::PreparedRuntimeSessionCommit::success(
                None,
                meerkat_core::lifecycle::RunBoundaryReceipt {
                    run_id: left_run.clone(),
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    contributing_input_ids: inputs.iter().map(|input| input.id().clone()).collect(),
                    conversation_digest: None,
                    message_count: 1,
                    sequence: 1,
                    owner_contributions: Vec::new(),
                },
                inputs
                    .iter()
                    .map(|input| {
                        record(row(
                            input,
                            InputLifecycleState::Consumed,
                            Some(left_run.clone()),
                            Some(1),
                        ))
                    })
                    .collect(),
                Some(session_id.clone()),
            ),
        )
        .await
        .expect("the run's boundary commits its receipt and its consumed rows");
}

/// The tail of an open run cut after a tool round whose result it
/// committed, with the run cursor its round commit wrote: the run binds
/// `bound` and resumes with its next model call.
fn continuable_tail(bound: Vec<InputId>) -> TailFn {
    Box::new(move |session: meerkat_core::Session, run_id: &RunId| {
        let mut session = interrupted_tail(session, run_id);
        session
            .set_run_cursor(&meerkat_core::SessionRunCursor::new(
                run_id.clone(),
                bound,
                0,
                meerkat_core::RunResumePoint::AwaitingModel,
            ))
            .expect("the run cursor serializes");
        session
    })
}

/// `input` applied by `run_id` and waiting for the run's boundary.
fn applied_to(input: &Input, run_id: &RunId) -> StoredInputState {
    let mut stored = row(
        input,
        InputLifecycleState::AppliedPendingConsumption,
        Some(run_id.clone()),
        Some(1),
    );
    stored.seed.attempt_count = 1;
    stored
}

/// `input` queued, never bound to a run, admitted `admission`-th.
fn queued(input: &Input, admission: u64) -> StoredInputState {
    let mut stored = row(input, InputLifecycleState::Queued, None, None);
    stored.seed.admission_sequence = Some(admission);
    stored
}

fn record(stored: StoredInputState) -> InputStatePersistenceRecord {
    InputStatePersistenceRecord::stored_row_for_test(stored).expect("a stored row")
}

fn stored_row<'a>(rows: &'a [StoredInputState], input: &Input) -> &'a StoredInputState {
    rows.iter()
        .find(|stored| &stored.state.input_id == input.id())
        .unwrap_or_else(|| panic!("the row of {}", input.id()))
}

fn settled_input<'a>(publication: &'a PredecessorPublication, input: &Input) -> &'a SettledInput {
    publication
        .converged
        .iter()
        .find(|settled| &settled.input_id == input.id())
        .unwrap_or_else(|| panic!("{} is settled: {:?}", input.id(), publication.converged))
}

/// The interaction terminal published for `input`'s requester.
fn published_for<'a>(events: &'a [AgentEvent], input: &Input) -> &'a AgentEvent {
    events
        .iter()
        .find(|event| {
            crate::input_state::interaction_terminal_event_id(event)
                .is_some_and(|interaction_id| interaction_id.0 == input.id().0)
        })
        .unwrap_or_else(|| panic!("a terminal is published for {}: {events:?}", input.id()))
}

fn sorted_ids(inputs: &[&Input]) -> Vec<InputId> {
    let mut ids = inputs
        .iter()
        .map(|input| input.id().clone())
        .collect::<Vec<_>>();
    ids.sort_by_key(|input_id| input_id.0);
    ids
}

fn sorted(mut ids: Vec<InputId>) -> Vec<InputId> {
    ids.sort_by_key(|input_id| input_id.0);
    ids
}

/// What a whole settlement made durable, step by step.
struct WholeSettlement {
    tail: Option<PredecessorTailCommit>,
    work: PredecessorWork,
    publication: Result<PredecessorPublication, PredecessorPublicationRefused>,
}

/// One whole settlement as the session service drives it: begin, the tail
/// drive, the internal registration and its sweep, the second drive when
/// the first held, and finish with `publisher`.
async fn settle_whole<P: CoreExecutorPublicationHandle + 'static>(
    host: &Host,
    claim: &HostingClaim,
    publisher: &Arc<P>,
) -> WholeSettlement {
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(claim, deadline())
        .await
        .expect("the settlement begins");
    let mut tail = host
        .machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the first drive answers");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the internal registration publishes and sweeps");
    if lease.tail_held()
        && let Some(second) = host
            .machine
            .settle_predecessor_tail_with_lease(&mut lease)
            .await
            .expect("the second drive commits the tail")
    {
        tail = Some(second);
    }
    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline(),
        )
        .await;
    WholeSettlement {
        tail,
        work,
        publication,
    }
}

/// A run whose answer reached its tail before its boundary: the writer
/// drive commits the tail (retaining the unbound queued inputs), and its
/// commit stages the run's batch for the contributors, the directed one's
/// outbox placed by the row's binding. The sweep ends the queued inputs
/// `Stopped`, the directed one's outbox placed by the witness while the
/// internal registration is unplaced, and the task publishes both directed
/// terminals and finds every row closed. The session is left with nothing
/// registered, no provisional tail and its lifecycle row without the
/// witness; a repeat finds nothing to settle and publishes nothing.
#[tokio::test]
async fn a_settlement_commits_the_tail_ends_the_rest_and_publishes_every_terminal() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("the run's directed contributor");
    let p1 = prompt("the run's prompt contributor");
    let q1 = prompt("queued beside the run");
    let d2 = directed_step("queued directed beside the run");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    host.persist(&session_id, queued(&d2, 2)).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the tail is committed");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::CommitCompletedRetainInputs
    );
    assert_eq!(sorted(settled.work.ended), sorted_ids(&[&q1, &d2]));
    assert!(settled.work.answered.is_empty());
    assert_eq!(
        settled.work.owner_loss,
        PredecessorOwnerLoss::NoConsentOwner
    );
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert!(publication.owed.is_empty());
    assert!(publication.migrated_without_receipt.is_empty());
    assert_eq!(publication.durable_rows_closed, 4);
    assert_eq!(publication.converged.len(), 4);
    for input in [&d1, &p1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::TailConsumed);
        assert_eq!(settled.receipt.terminal(), &InputTerminalOutcome::Consumed);
        assert_eq!(settled.receipt.run_id(), Some(&run_id));
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::CompletedWithoutResult
        ));
    }
    for input in [&q1, &d2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::Ended);
        assert_eq!(
            settled.receipt.terminal(),
            &InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped,
            }
        );
        assert_eq!(
            settled.receipt.scope(),
            &InputTerminalReceiptScope::RuntimeTermination
        );
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::RuntimeTerminated { reason, .. } if reason == "runtime stopped"
        ));
    }
    assert_eq!(
        settled_input(&publication, &p1).published_terminal_seq,
        None
    );
    assert_eq!(
        settled_input(&publication, &q1).published_terminal_seq,
        None
    );
    assert!(
        settled_input(&publication, &d1)
            .published_terminal_seq
            .is_some()
    );
    assert!(
        settled_input(&publication, &d2)
            .published_terminal_seq
            .is_some()
    );

    let events = publisher.events();
    assert_eq!(
        events.len(),
        2,
        "only the directed inputs' requesters are answered: {events:?}"
    );
    assert!(matches!(
        published_for(&events, &d1),
        AgentEvent::InteractionComplete { result, .. } if result.is_empty()
    ));
    assert!(matches!(
        published_for(&events, &d2),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "runtime stopped"
    ));
    let rows = host.rows(&session_id).await;
    for input in [&d1, &d2] {
        let outbox = stored_row(&rows, input)
            .state
            .interaction_terminal_outbox
            .as_ref()
            .expect("the directed input's outbox");
        assert_eq!(
            outbox.owner_agent_runtime_id.as_deref(),
            Some(PLACED_RUNTIME)
        );
        assert_eq!(outbox.owner_fence_token, Some(PLACED_FENCE));
        assert_eq!(outbox.owner_runtime_generation, Some(PLACED_GENERATION));
    }
    assert!(!host.machine.contains_session(&session_id).await);
    assert!(!host.provisional_tail_stands(&session_id).await);
    assert_eq!(
        host.placement_witness(&session_id).await,
        None,
        "the sweep commit wrote the row back without the witness"
    );

    let repeat_publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &repeat_publisher).await;
    assert!(repeat.tail.is_none(), "the tail stands no more");
    assert!(repeat.work.ended.is_empty());
    assert!(repeat.work.answered.is_empty());
    let repeated = repeat
        .publication
        .expect("the repeat finds every row closed");
    assert!(repeated.converged.is_empty());
    assert!(repeated.owed.is_empty());
    assert_eq!(repeated.durable_rows_closed, 4);
    assert!(repeat_publisher.events().is_empty());
    assert!(!host.machine.contains_session(&session_id).await);
}

/// A run cut after a tool round, with a queued input beside it that no run
/// bound: the writer drive holds the tail. The sweep ends the run's inputs
/// and the queued one `Stopped` in one commit, and the second drive then
/// commits the tail as interrupted with no contributor left to consume.
#[tokio::test]
async fn a_tail_held_beside_unbound_work_commits_once_the_sweep_ended_it() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(interrupted_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let b1 = directed_step("bound to the cut run");
    let b2 = prompt("also bound to the cut run");
    let q1 = prompt("queued beside the cut run");
    host.persist(&session_id, applied_to(&b1, &run_id)).await;
    host.persist(&session_id, applied_to(&b2, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    assert_eq!(
        host.machine
            .settle_predecessor_tail_with_lease(&mut lease)
            .await
            .expect("the first drive answers"),
        None,
        "the first drive holds the tail beside the unbound input"
    );
    assert!(lease.tail_held());
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep ends every input");
    assert_eq!(sorted(work.ended), sorted_ids(&[&b1, &b2, &q1]));
    let tail = host
        .machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the second drive answers")
        .expect("the second drive commits the tail");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted
    );
    assert!(!lease.tail_held());
    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline(),
        )
        .await
        .expect("the census finds every row closed");

    assert_eq!(publication.durable_rows_closed, 3);
    for input in [&b1, &b2, &q1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::Ended);
        assert_eq!(
            settled.receipt.terminal(),
            &InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped,
            }
        );
        assert_eq!(settled.receipt.run_id(), None);
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &b1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { .. },
            ..
        }
    ));
    assert!(!host.provisional_tail_stands(&session_id).await);
    assert!(!host.machine.contains_session(&session_id).await);
}

/// A run cut after a tool round with nothing else left: the writer drive
/// commits it interrupted, and its contributors' batch answers them
/// `Cancelled` under the run's scope; the directed one's requester gets the
/// cancellation.
#[tokio::test]
async fn an_interrupted_tail_alone_answers_its_inputs_cancelled() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(interrupted_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let i1 = directed_step("cut directed");
    let i2 = prompt("cut prompt");
    host.persist(&session_id, applied_to(&i1, &run_id)).await;
    host.persist(&session_id, applied_to(&i2, &run_id)).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the tail is committed");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted
    );
    assert!(settled.work.ended.is_empty());
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    for input in [&i1, &i2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::TailConsumed);
        assert_eq!(settled.receipt.terminal(), &InputTerminalOutcome::Consumed);
        assert_eq!(settled.receipt.run_id(), Some(&run_id));
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::Cancelled
        ));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &i1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Cancelled,
            ..
        }
    ));
}

/// A boundary of run `left_run` committed before this release left two
/// inputs consumed with no receipt, its receipt durable at their sequence.
/// The settlement answers them in one runless batch: terminal `Consumed`,
/// scope runtime termination, outcome unknown naming `left_run`; the
/// directed one's requester gets the outcome-unknown failure, its outbox
/// placed by the row's binding. The rows keep their run and sequence. A
/// repeat answers nothing again and publishes nothing.
#[tokio::test]
async fn inputs_a_recovered_boundary_left_without_a_receipt_are_answered_outcome_unknown() {
    let host = Host::head_canonical();
    let (session_id, _) = durable_session(&host, None).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let left_run = RunId::new();
    let l1 = directed_step("consumed without a receipt");
    let l2 = prompt("consumed without a receipt too");
    commit_left_boundary(&host, &session_id, &left_run, &[&l1, &l2]).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    assert!(settled.tail.is_none());
    assert!(settled.work.ended.is_empty());
    assert_eq!(sorted(settled.work.answered), sorted_ids(&[&l1, &l2]));
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert_eq!(publication.durable_rows_closed, 2);
    for input in [&l1, &l2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::OutcomeUnknown);
        assert_eq!(settled.receipt.terminal(), &InputTerminalOutcome::Consumed);
        assert_eq!(
            settled.receipt.scope(),
            &InputTerminalReceiptScope::RuntimeTermination
        );
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::OutcomeUnknown { run_id } if run_id == &left_run
        ));
    }
    assert!(
        settled_input(&publication, &l1)
            .published_terminal_seq
            .is_some()
    );
    assert_eq!(
        settled_input(&publication, &l2).published_terminal_seq,
        None
    );
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &l1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::OutcomeUnknown,
            ..
        }
    ));
    let rows = host.rows(&session_id).await;
    for input in [&l1, &l2] {
        let stored = stored_row(&rows, input);
        assert_eq!(stored.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(stored.seed.last_run_id.as_ref(), Some(&left_run));
        assert_eq!(stored.seed.last_boundary_sequence, Some(1));
    }
    let outbox = stored_row(&rows, &l1)
        .state
        .interaction_terminal_outbox
        .as_ref()
        .expect("the directed input's outbox");
    assert_eq!(
        outbox.owner_agent_runtime_id.as_deref(),
        Some(PLACED_RUNTIME)
    );

    let repeat_publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &repeat_publisher).await;
    assert!(repeat.work.answered.is_empty());
    let repeated = repeat
        .publication
        .expect("the repeat finds every row closed");
    assert!(repeated.converged.is_empty());
    assert_eq!(repeated.durable_rows_closed, 2);
    assert!(
        repeat_publisher.events().is_empty(),
        "an answered input is never published again"
    );
}

/// Beside another run's batch that has not finished, the writer drive
/// stages nothing for the tail's contributors, so they are left consumed
/// without a receipt: the sweep answers them outcome unknown, naming the
/// tail's run, and the task finishes the other run's batch too.
#[tokio::test]
async fn a_tail_beside_another_runs_open_batch_answers_its_contributors_outcome_unknown() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("the tail's directed contributor");
    let p1 = prompt("the tail's prompt contributor");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    let earlier_run = RunId::new();
    let c0 = prompt("an earlier run's contributor");
    let mut earlier = row(
        &c0,
        InputLifecycleState::Consumed,
        Some(earlier_run.clone()),
        Some(1),
    );
    earlier.state.terminal_completion = Some(
        crate::meerkat_machine::driver::build_input_terminal_completion_rows(
            crate::meerkat_machine::driver::InteractionTerminalBatchScope::Run(&earlier_run),
            std::slice::from_ref(c0.id()),
            crate::input_state::InteractionTerminalCandidate::CompletedWithoutResult,
            false,
        )
        .expect("the earlier run's batch")
        .remove(0),
    );
    host.persist(&session_id, earlier).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the tail is committed");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(sorted(settled.work.answered), sorted_ids(&[&d1, &p1]));
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert_eq!(publication.durable_rows_closed, 3);
    let converged = settled_input(&publication, &c0);
    assert_eq!(converged.settled_by, SettledBy::PendingTerminalConverged);
    assert_eq!(converged.receipt.run_id(), Some(&earlier_run));
    for input in [&d1, &p1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::OutcomeUnknown);
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::OutcomeUnknown { run_id: answered } if answered == &run_id
        ));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &d1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::OutcomeUnknown,
            ..
        }
    ));
}

/// An earlier start's recovery committed the tail with its contributors'
/// batch and the process died before its drain: the settlement finds the
/// tail aligned, rehydrates the batch, and the task finalizes and publishes
/// it.
#[tokio::test]
async fn a_tail_an_earlier_start_committed_has_its_batch_converged() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("committed by an earlier start");
    let p1 = prompt("committed by an earlier start too");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    match crate::recovery::recover_durable_tail(host.store.as_ref(), &session_id)
        .await
        .expect("the earlier start's recovery answers")
    {
        crate::recovery::DurableTailRecoveryOutcome::Committed { disposition, .. } => {
            assert_eq!(disposition, DurableTailRecoveryDisposition::CommitCompleted);
        }
        other => panic!("the earlier start commits the tail: {other:?}"),
    }
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    assert!(settled.tail.is_none(), "the tail is aligned already");
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert_eq!(publication.durable_rows_closed, 2);
    for input in [&d1, &p1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::PendingTerminalConverged);
        assert_eq!(settled.receipt.run_id(), Some(&run_id));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &d1),
        AgentEvent::InteractionComplete { .. }
    ));
}

/// A settlement that stopped after its internal registration wrote the
/// lifecycle row with the placement witness (clean, unbound) and before its
/// sweep: the repeat's begin takes the placement from the witness, and the
/// sweep places the directed input's outbox by it, the whole witness
/// (runtime id, fence, generation and epoch), and writes the row back
/// without it. The drain then adopts the outbox on the internal
/// registration's unplaced shell before it publishes: the generated
/// adoption keeps the outbox's runtime id, fence and generation there and
/// re-stamps its epoch with the shell's own, as for every outbox an
/// unplaced shell adopts.
#[tokio::test]
async fn a_repeat_takes_the_placement_from_the_witness_a_stopped_settlement_left() {
    let host = Host::head_canonical();
    let (session_id, _) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(
        &session_id,
        MachineLifecycleBindingFacts::default(),
        Some(predecessor_placement()),
    )
    .await;
    let d2 = directed_step("queued directed");
    host.persist(&session_id, queued(&d2, 1)).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins from the witness");
    assert!(
        host.machine
            .settle_predecessor_tail_with_lease(&mut lease)
            .await
            .expect("the drive answers")
            .is_some()
    );
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep commits");
    assert_eq!(work.ended, vec![d2.id().clone()]);
    let rows = host.rows(&session_id).await;
    let staged = stored_row(&rows, &d2)
        .state
        .interaction_terminal_outbox
        .clone()
        .expect("the sweep staged the directed input's outbox");
    assert_eq!(
        (
            staged.owner_agent_runtime_id.as_deref(),
            staged.owner_fence_token,
            staged.owner_runtime_generation,
            staged.owner_runtime_epoch_id.as_deref(),
        ),
        (
            Some(PLACED_RUNTIME),
            Some(PLACED_FENCE),
            Some(PLACED_GENERATION),
            Some(PLACED_EPOCH),
        ),
        "the sweep places the outbox by the whole witness"
    );
    assert_eq!(host.placement_witness(&session_id).await, None);

    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline(),
        )
        .await
        .expect("the census finds every row closed");
    assert_eq!(
        settled_input(&publication, &d2).settled_by,
        SettledBy::Ended
    );
    assert_eq!(publisher.events().len(), 1);
    let rows = host.rows(&session_id).await;
    let published = stored_row(&rows, &d2)
        .state
        .interaction_terminal_outbox
        .as_ref()
        .expect("the published outbox stays with its row");
    assert_eq!(
        (
            published.owner_agent_runtime_id.as_deref(),
            published.owner_fence_token,
            published.owner_runtime_generation,
        ),
        (
            Some(PLACED_RUNTIME),
            Some(PLACED_FENCE),
            Some(PLACED_GENERATION),
        ),
        "the adoption keeps the witness's runtime id, fence and generation"
    );
}

/// A terminal row a migration left without its receipt (the marker set) is
/// reported apart and never changed or published.
#[tokio::test]
async fn a_migrated_row_is_reported_apart_and_left_as_it_is() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, None).await;
    let m1 = prompt("terminal before receipts existed");
    let mut migrated = row(&m1, InputLifecycleState::Consumed, Some(run_id), Some(1));
    migrated.state.terminal_completion_unavailable = true;
    host.persist(&session_id, migrated).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert!(publication.converged.is_empty());
    assert_eq!(publication.migrated_without_receipt, vec![m1.id().clone()]);
    assert_eq!(publication.durable_rows_closed, 1);
    assert!(publisher.events().is_empty());
    let rows = host.rows(&session_id).await;
    let stored = stored_row(&rows, &m1);
    assert!(stored.state.terminal_completion_unavailable);
    assert!(stored.state.terminal_completion.is_none());
}

// ---------------------------------------------------------------------------
// The predecessor's pending terminal and its lost consent
// ---------------------------------------------------------------------------

/// The row of `input`, a directed input that a cancel point of the
/// predecessor (placed at the predecessor's placement) ended while it was
/// still queued: terminal `Abandoned { Cancelled }`, its runless batch
/// pending with `reason`, and its outbox not yet published. Built through the
/// predecessor runtime's own staging path, on a scratch driver whose
/// authority holds that placement.
async fn cancelled_while_queued(
    session_id: &SessionId,
    input: &Input,
    reason: &str,
) -> StoredInputState {
    use crate::meerkat_machine::dsl;
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let mut driver = crate::meerkat_machine::driver::DriverEntry::Ephemeral(
        crate::EphemeralRuntimeDriver::new(runtime_id),
    );
    let epoch_id = dsl::RuntimeEpochId::from(PLACED_EPOCH.to_string());
    let mut authority = crate::meerkat_machine::dsl_authority::new_registered_authority_id(
        dsl::SessionId::from_domain(session_id),
        epoch_id.clone(),
    )
    .expect("the predecessor's registered authority");
    dsl::MeerkatMachineMutator::apply(
        &mut authority,
        dsl::MeerkatMachineInput::PrepareBindings {
            agent_runtime_id: dsl::AgentRuntimeId::from(PLACED_RUNTIME.to_string()),
            fence_token: dsl::FenceToken::from(PLACED_FENCE),
            generation: Some(dsl::Generation::from(PLACED_GENERATION)),
            runtime_epoch_id: Some(epoch_id),
            session_id: dsl::SessionId::from_domain(session_id),
            residency_owner: dsl::SessionResidencyOwner::Session,
        },
    )
    .expect("the predecessor's placed authority");
    *driver
        .shared_dsl_authority()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
    driver.set_control_projection(crate::RuntimeState::Attached, None, None);
    assert!(
        driver
            .as_driver_mut()
            .accept_input(input.clone())
            .await
            .expect("the predecessor accepts the input")
            .is_accepted()
    );
    let prepared = driver
        .prepare_sole_runless_runtime_termination(input.id(), reason.to_string())
        .expect("the cancel point prepares its runless terminal");
    assert!(
        driver
            .abandon_queued_input(input.id(), InputAbandonReason::Cancelled)
            .await
            .expect("the cancel point ends the queued input")
    );
    assert_eq!(
        crate::meerkat_machine::driver::DriverEntry::commit_prepared_sole_runless_termination(
            prepared
        ),
        Some(input.id().clone())
    );
    driver
        .as_driver()
        .stored_input_state(input.id())
        .expect("the cancelled input's row")
}

/// A consent a lost owner of the session issued for routed work in the
/// session's scope of the host's hosting domain, approved by an
/// authenticated human and parked at its run's close: unspent and
/// unwithdrawn.
async fn lost_owners_parked_consent(
    host: &Host,
    session_id: &SessionId,
    approvals: &meerkat_core::approval::ApprovalService,
) -> meerkat_core::approval::consent_test_support::ParkedRoutedConsent {
    let domain = host
        .store
        .hosting_capability()
        .domain_ref()
        .expect("the store hosts its sessions under OS locks");
    meerkat_core::approval::consent_test_support::ParkedRoutedConsent::park(
        approvals,
        meerkat_core::approval::ApprovalConsentScope {
            domain,
            session_id: session_id.clone(),
        },
        meerkat_core::retained_work::RetainedWorkIdentity::new(
            LogicalRuntimeId::for_session(session_id).0,
            RunId::new(),
            Vec::new(),
            std::collections::BTreeMap::new(),
            None,
        ),
    )
    .await
    .expect("the lost owner's consent is parked")
}

/// Wait, bounded, until no claim of `session_id`'s lineage is alive in this
/// process: the settlement task drops its registration's last clone of the
/// claim right after it answers.
async fn lineage_ended(host: &Host, session_id: &SessionId) {
    for _ in 0..200 {
        if matches!(
            host.machine.session_serving(session_id),
            crate::session_hosting::SessionServing::NotHeldInThisProcess
        ) {
            return;
        }
        crate::tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the claim lineage of session {session_id} never ended");
}

/// An issuing consent owner that records the proof of every reservation it
/// settles, or fails every settlement while `failing` is set.
#[derive(Default)]
struct RecordingConsentOwner {
    settled: std::sync::Mutex<Vec<HostingOwnerLossProof>>,
    failing: std::sync::atomic::AtomicBool,
}

impl meerkat_core::approval::ConsentOwnerLossSettlement for RecordingConsentOwner {
    fn settle(
        &self,
        reservation: &crate::session_hosting::OwnerLossReservation,
    ) -> Result<(), meerkat_core::ApprovalError> {
        if self.failing.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(meerkat_core::ApprovalError::StoreUnavailable(
                "the consent owner's store is unavailable".to_string(),
            ));
        }
        self.settled
            .lock()
            .unwrap()
            .push(reservation.proof().clone());
        Ok(())
    }
}

/// The whole predecessor: a run whose answer reached its tail before its
/// boundary (its directed and nondirected contributors), a queued prompt
/// and a queued directed input beside it, a directed input a cancel point
/// ended whose runless terminal is still unpublished, and an approved,
/// unspent consent the lost owner parked in the session's scope. The lost
/// owner is the predecessor's own approval owner; this process installs a
/// new issuing consent owner over the same approval store, which knows the
/// consent only from its stored row, as after a restart.
///
/// The settlement commits the tail with its contributors' batch, ends the
/// queued inputs `Stopped`, converges the cancelled input's pending terminal
/// (its requester gets the failure the cancel point owed), and the issuing
/// consent owner settles the lost owner's consent under the claim's own
/// reservation, which completes at the internal registration's publication:
/// the consent keeps its decision, can no longer be spent, and owes its
/// Withdrawn notice. A repeat on the same live lineage settles nothing
/// again: it reports the proof the lineage settled, and the consent owes
/// one notice still.
#[tokio::test]
async fn a_settlement_answers_everything_the_predecessor_left_and_settles_its_lost_consent() {
    use meerkat_core::approval::{
        ApprovalConsentDeliveryState, ApprovalConsentTransition, ApprovalConsentWithdrawalReason,
        ApprovalService, ApprovalStatus,
    };

    let host = Host::head_canonical();
    // The approval store outlives the predecessor process: its approval
    // owner parked the consent, and this process's owner is a new instance
    // over the same store.
    let approval_store: Arc<dyn meerkat_core::approval::ApprovalStore> =
        Arc::new(meerkat_core::approval::InMemoryApprovalStore::new());
    let approvals =
        ApprovalService::with_store(Arc::clone(&approval_store)).expect("this process's owner");
    host.machine
        .install_consent_owner_loss_settlement(Arc::new(approvals.clone()))
        .expect("the issuing consent owner is installed before any registration");
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("the run's directed contributor");
    let p1 = prompt("the run's prompt contributor");
    let q1 = prompt("queued beside the run");
    let d2 = directed_step("queued directed beside the run");
    let x0 = directed_step("cancelled while queued");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    host.persist(&session_id, queued(&d2, 2)).await;
    host.persist(
        &session_id,
        cancelled_while_queued(&session_id, &x0, "cancelled by its caller").await,
    )
    .await;
    let lost_owner =
        ApprovalService::with_store(approval_store).expect("the predecessor's approval owner");
    let consent = lost_owners_parked_consent(&host, &session_id, &lost_owner).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the tail is committed");
    assert_eq!(tail.run_id, run_id);
    let PredecessorOwnerLoss::Completed(proof) = settled.work.owner_loss.clone() else {
        panic!(
            "the consent owner settled the lineage's owner loss: {:?}",
            settled.work.owner_loss
        );
    };
    assert_eq!(proof.session_id(), &session_id);
    assert_eq!(
        proof.acquisition(),
        crate::session_hosting::HostingAcquisition::AcquiredFresh
    );
    assert_eq!(
        Some(proof.domain()),
        host.store.hosting_capability().domain_ref().as_ref()
    );
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert!(publication.owed.is_empty());
    assert_eq!(publication.durable_rows_closed, 5);
    for input in [&d1, &p1] {
        assert_eq!(
            settled_input(&publication, input).settled_by,
            SettledBy::TailConsumed
        );
    }
    for input in [&q1, &d2] {
        assert_eq!(
            settled_input(&publication, input).settled_by,
            SettledBy::Ended
        );
    }
    let cancelled = settled_input(&publication, &x0);
    assert_eq!(cancelled.settled_by, SettledBy::PendingTerminalConverged);
    assert_eq!(
        cancelled.receipt.terminal(),
        &InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        }
    );
    assert_eq!(
        cancelled.receipt.scope(),
        &InputTerminalReceiptScope::RuntimeTermination
    );
    assert!(matches!(
        cancelled.receipt.outcome(),
        CompletionOutcome::RuntimeTerminated { reason, .. } if reason == "cancelled by its caller"
    ));
    assert!(cancelled.published_terminal_seq.is_some());
    let events = publisher.events();
    assert_eq!(events.len(), 3, "{events:?}");
    assert!(matches!(
        published_for(&events, &x0),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "cancelled by its caller"
    ));

    let withdrawn = |approvals: &ApprovalService| {
        let record = approvals
            .get(consent.approval_id())
            .expect("the consent row reads");
        assert_eq!(
            record.status,
            ApprovalStatus::Approved,
            "the decision stays"
        );
        let row = record.consent.expect("a consent row");
        assert!(row.consumed_at.is_none(), "the consent was never spent");
        assert_eq!(
            row.withdrawal.map(|withdrawal| withdrawal.reason),
            Some(ApprovalConsentWithdrawalReason::Abandoned),
            "the lost owner's consent can no longer be spent"
        );
        row.deliveries
            .iter()
            .filter(|delivery| {
                delivery.transition == ApprovalConsentTransition::Withdrawn
                    && delivery.state == ApprovalConsentDeliveryState::Owed
            })
            .count()
    };
    assert_eq!(
        withdrawn(&approvals),
        1,
        "its Withdrawn notice is owed once"
    );

    let repeat_publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &repeat_publisher).await;
    assert_eq!(
        repeat.work.owner_loss,
        PredecessorOwnerLoss::NotOwed(proof),
        "the same live lineage settles nothing again"
    );
    repeat
        .publication
        .expect("the repeat finds every row closed");
    assert!(repeat_publisher.events().is_empty());
    assert_eq!(withdrawn(&approvals), 1, "no notice is owed twice");
}

/// The issuing consent owner fails the lineage's owner-loss settlement: the
/// internal registration is refused before it publishes, typed, after the
/// tail was committed; nothing is swept and nothing stays registered.
#[tokio::test]
async fn a_failed_consent_settlement_refuses_the_internal_registration_before_any_sweep() {
    let host = Host::head_canonical();
    let owner = Arc::new(RecordingConsentOwner::default());
    owner
        .failing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    host.machine
        .install_consent_owner_loss_settlement(owner.clone())
        .expect("the issuing consent owner is installed before any registration");
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let p1 = prompt("the run's prompt contributor");
    let q1 = prompt("queued beside the run");
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    let claim = host.claim(&session_id);

    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    let tail = host
        .machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the first drive answers")
        .expect("the tail is committed before the registration");
    assert_eq!(tail.run_id, run_id);
    let refused = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect_err("an unsettled owner loss refuses the internal registration");
    assert!(
        matches!(
            &refused,
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::ConsentOwnerLossUnsettled {
                session_id: refused_session,
            }) if refused_session == &session_id
        ),
        "{refused:?}"
    );
    assert!(!lease.registered());
    drop(lease);
    assert!(!host.machine.contains_session(&session_id).await);
    let rows = host.rows(&session_id).await;
    assert_eq!(
        stored_row(&rows, &q1).seed.phase,
        InputLifecycleState::Queued,
        "nothing was swept"
    );
    assert!(owner.settled.lock().unwrap().is_empty());
}

/// The host drops its claim between the settlement and its registration:
/// the lineage the settlement completed ends with it. The next grant is not
/// a join; its registration owes, and the consent owner settles, the owner
/// loss of the new lineage, a second settlement. A proof names the hosting
/// domain, the session and how the claim was acquired, not the lineage, so
/// the second settlement's proof can equal the first.
#[tokio::test]
async fn a_dropped_claim_ends_the_lineage_the_settlement_completed() {
    let host = Host::head_canonical();
    let owner = Arc::new(RecordingConsentOwner::default());
    host.machine
        .install_consent_owner_loss_settlement(owner.clone())
        .expect("the issuing consent owner is installed before any registration");
    let (session_id, _) = durable_session(&host, Some(completed_tail)).await;
    let claim = host.claim(&session_id);
    let settled = settle_whole(&host, &claim, &Arc::new(RecordingPublisher::default())).await;
    let PredecessorOwnerLoss::Completed(first) = settled.work.owner_loss.clone() else {
        panic!(
            "the settlement completed the owner loss: {:?}",
            settled.work.owner_loss
        );
    };
    settled
        .publication
        .expect("the census finds every row closed");
    drop(claim);
    lineage_ended(&host, &session_id).await;

    let next = host.claim(&session_id);
    assert_ne!(
        next.acquisition(),
        crate::session_hosting::HostingAcquisition::Joined,
        "the settlement's lineage ended with its last claim"
    );
    host.machine
        .register_session(session_id.clone())
        .await
        .expect("the session registers under the new lineage");
    let settled_proofs = owner.settled.lock().unwrap().clone();
    assert_eq!(
        settled_proofs.len(),
        2,
        "the new lineage's registration settled its own owner loss: {settled_proofs:?}"
    );
    assert_eq!(settled_proofs[0], first);
}

// ---------------------------------------------------------------------------
// Crash and cancellation between the steps
// ---------------------------------------------------------------------------

/// The lifecycle row's record version and placement witness.
async fn lifecycle_record(
    host: &Host,
    session_id: &SessionId,
) -> (u16, Option<MachineLifecycleBindingFacts>) {
    match host
        .store
        .observe_machine_lifecycle(&LogicalRuntimeId::for_session(session_id))
        .await
        .expect("the lifecycle row reads")
    {
        MachineLifecycleObservation::Decoded { record, .. } => {
            (record.record_version(), record.placement_witness().cloned())
        }
        other => panic!("the lifecycle row decodes: {other:?}"),
    }
}

/// The held-tail shape: an open run at a committed round (its round
/// commit's cursor binds a directed and a nondirected input), with a queued
/// input beside it that an earlier run left (requeued, still naming that
/// run). The cursor does not list it, but nothing proves it is not in the
/// tail, so the writer drive holds the tail (a store-only writer never
/// continues a run, and the unbound input keeps it from closing the run as
/// interrupted). A queued input that names no run would not hold it: the
/// cursor proves such an input outside the tail, and the drive commits the
/// run interrupted at once. The internal registration
/// marks the run resumable; the sweep ends the mark and every input
/// `Stopped` in one commit; the second drive then finds the cursor's inputs
/// settled and commits the tail as interrupted, with no contributor left to
/// consume.
#[tokio::test]
async fn an_open_run_held_beside_unbound_work_is_ended_and_then_committed_interrupted() {
    let host = Host::head_canonical();
    let b1 = directed_step("bound to the open run");
    let b2 = prompt("also bound to the open run");
    let q1 = prompt("queued beside the open run");
    let (session_id, run_id) = durable_session_built(
        &host,
        None,
        Some(continuable_tail(vec![b1.id().clone(), b2.id().clone()])),
    )
    .await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    host.persist(&session_id, applied_to(&b1, &run_id)).await;
    host.persist(&session_id, applied_to(&b2, &run_id)).await;
    host.persist(&session_id, queued_after_an_earlier_run(&q1, 1))
        .await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    assert_eq!(
        host.machine
            .settle_predecessor_tail_with_lease(&mut lease)
            .await
            .expect("the first drive answers"),
        None,
        "the writer drive holds the open run"
    );
    assert!(lease.tail_held());
    assert!(host.provisional_tail_stands(&session_id).await);
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep ends every input");
    assert_eq!(sorted(work.ended), sorted_ids(&[&b1, &b2, &q1]));
    let tail = host
        .machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the second drive answers")
        .expect("the second drive commits the tail");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted
    );
    let publication = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline(),
        )
        .await
        .expect("the census finds every row closed");

    for input in [&b1, &b2, &q1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::Ended);
        assert_eq!(
            settled.receipt.terminal(),
            &InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped,
            }
        );
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &b1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "runtime stopped"
    ));
    assert!(!host.provisional_tail_stands(&session_id).await);
    assert!(!host.machine.contains_session(&session_id).await);
}

/// `input` queued again after an earlier run that never committed it: its
/// row still names that run, admitted `admission`-th.
fn queued_after_an_earlier_run(input: &Input, admission: u64) -> StoredInputState {
    let mut stored = queued(input, admission);
    stored.seed.last_run_id = Some(RunId::new());
    stored.seed.attempt_count = 1;
    stored
}

/// The held-tail shape again, with a crash after the internal registration
/// wrote the clean lifecycle row with the placement witness and before the
/// sweep commit (the first attempt's drive held; its registration's row is
/// written here as that registration wrote it, version 7, unbound, the
/// predecessor's placement as witness, and nothing terminalized). The
/// retry's begin takes the placement from the witness, its drive holds
/// again, the sweep places the directed input's outbox by the witness and
/// writes the row back without it, and the second drive commits the tail
/// interrupted: the retry ends as the uninterrupted settlement does.
#[tokio::test]
async fn a_retry_after_a_crash_between_registration_and_sweep_finishes_by_the_witness() {
    let host = Host::head_canonical();
    let b1 = directed_step("bound to the open run");
    let b2 = prompt("also bound to the open run");
    let q1 = prompt("queued beside the open run");
    let (session_id, run_id) = durable_session_built(
        &host,
        None,
        Some(continuable_tail(vec![b1.id().clone(), b2.id().clone()])),
    )
    .await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    host.persist(&session_id, applied_to(&b1, &run_id)).await;
    host.persist(&session_id, applied_to(&b2, &run_id)).await;
    host.persist(&session_id, queued_after_an_earlier_run(&q1, 1))
        .await;
    let claim = host.claim(&session_id);

    let mut first = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the first attempt begins");
    assert_eq!(
        host.machine
            .settle_predecessor_tail_with_lease(&mut first)
            .await
            .expect("the first drive answers"),
        None
    );
    drop(first);
    host.lifecycle_row(
        &session_id,
        MachineLifecycleBindingFacts::default(),
        Some(predecessor_placement()),
    )
    .await;
    assert_eq!(
        lifecycle_record(&host, &session_id).await,
        (7, Some(predecessor_placement())),
        "instrument: the crash left the version 7 row with its witness"
    );

    let publisher = Arc::new(RecordingPublisher::default());
    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the second drive commits the tail");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::RepairAndCommitInterrupted
    );
    assert_eq!(sorted(settled.work.ended), sorted_ids(&[&b1, &b2, &q1]));
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    for input in [&b1, &b2, &q1] {
        assert_eq!(
            settled_input(&publication, input).settled_by,
            SettledBy::Ended
        );
    }
    let rows = host.rows(&session_id).await;
    let outbox = stored_row(&rows, &b1)
        .state
        .interaction_terminal_outbox
        .as_ref()
        .expect("the directed input's outbox");
    assert_eq!(
        outbox.owner_agent_runtime_id.as_deref(),
        Some(PLACED_RUNTIME)
    );
    assert_eq!(outbox.owner_fence_token, Some(PLACED_FENCE));
    assert_eq!(outbox.owner_runtime_generation, Some(PLACED_GENERATION));
    assert_eq!(publisher.events().len(), 1);
    let (version, witness) = lifecycle_record(&host, &session_id).await;
    assert_ne!(version, 7, "the sweep commit wrote the row back");
    assert_eq!(witness, None);
}

/// A settlement stopped after its sweep commit, before the drain (its
/// caller went away, so its registration was removed without publishing):
/// every carrier is durable and every receipt pending. The repeat ends
/// nothing, rehydrates the pending batches, drains them, and lists every
/// input as a pending terminal it converged.
#[tokio::test]
async fn a_settlement_stopped_before_its_drain_is_converged_by_the_repeat() {
    let host = Host::head_canonical();
    let (session_id, run_id) = durable_session(&host, Some(completed_tail)).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("the run's directed contributor");
    let p1 = prompt("the run's prompt contributor");
    let q1 = prompt("queued beside the run");
    let d2 = directed_step("queued directed beside the run");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    host.persist(&session_id, queued(&d2, 2)).await;
    let claim = host.claim(&session_id);

    let mut first = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the first attempt begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut first)
        .await
        .expect("the first drive answers")
        .expect("the tail is committed");
    host.machine
        .end_predecessor_work_with_lease_before(&mut first, deadline())
        .await
        .expect("the sweep commits");
    drop(first);
    let rows = host.rows(&session_id).await;
    for input in [&d1, &p1, &q1, &d2] {
        assert!(
            matches!(
                stored_row(&rows, input)
                    .state
                    .terminal_completion
                    .as_ref()
                    .map(|completion| &completion.phase),
                Some(crate::input_state::InputTerminalCompletionPhase::Pending)
            ),
            "instrument: {} owes its receipt",
            input.id()
        );
    }

    let publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &publisher).await;

    assert!(repeat.tail.is_none(), "the tail was committed already");
    assert!(repeat.work.ended.is_empty(), "the repeat ends nothing");
    assert!(repeat.work.answered.is_empty());
    let publication = repeat
        .publication
        .expect("the census finds every row closed");
    assert_eq!(publication.durable_rows_closed, 4);
    for input in [&d1, &p1, &q1, &d2] {
        assert_eq!(
            settled_input(&publication, input).settled_by,
            SettledBy::PendingTerminalConverged
        );
    }
    assert_eq!(
        settled_input(&publication, &q1).receipt.terminal(),
        &InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Stopped,
        }
    );
    assert_eq!(
        publisher.events().len(),
        2,
        "d1 and d2 are answered once each"
    );
}

/// A settlement stopped after its sweep commit staged the outcome-unknown
/// batch, before the drain: the repeat finds those rows owing their receipt
/// (no longer rows a recovered boundary left without one), rehydrates the
/// batch, drains it, and lists both inputs answered outcome unknown, naming
/// their run.
#[tokio::test]
async fn an_outcome_unknown_batch_a_stopped_settlement_left_is_converged_by_the_repeat() {
    let host = Host::head_canonical();
    let (session_id, _) = durable_session(&host, None).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let left_run = RunId::new();
    let l1 = directed_step("consumed without a receipt");
    let l2 = prompt("consumed without a receipt too");
    commit_left_boundary(&host, &session_id, &left_run, &[&l1, &l2]).await;
    let claim = host.claim(&session_id);

    let mut first = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the first attempt begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut first)
        .await
        .expect("the first drive answers");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut first, deadline())
        .await
        .expect("the sweep commits the outcome-unknown batch");
    assert_eq!(sorted(work.answered), sorted_ids(&[&l1, &l2]));
    drop(first);

    let publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &publisher).await;

    assert!(
        repeat.work.answered.is_empty(),
        "the rows owe their receipt now; nothing is answered again"
    );
    let publication = repeat
        .publication
        .expect("the census finds every row closed");
    for input in [&l1, &l2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::OutcomeUnknown);
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::OutcomeUnknown { run_id } if run_id == &left_run
        ));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(matches!(
        published_for(&events, &l1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::OutcomeUnknown,
            ..
        }
    ));
}

/// A process restart after the sweep commit, before the drain: the queued
/// directed input the sweep ended carries an outbox placed by the witness,
/// which the lifecycle row no longer records, and the inputs it answered
/// carry the pending outcome-unknown batch. A new process over the same
/// stores settles again: its internal registration rehydrates both
/// batches, the repeat ends and answers nothing more, every input converges
/// (the ended one as a pending terminal, the answered ones outcome unknown,
/// naming their run), each requester is answered once, and a further repeat
/// publishes nothing.
#[tokio::test]
async fn a_restart_after_the_sweep_rehydrates_both_batches_and_answers_each_requester_once() {
    let host = Host::head_canonical();
    let left_run = RunId::new();
    let l1 = directed_step("consumed without a receipt");
    let l2 = prompt("consumed without a receipt too");
    let left_inputs: [&Input; 2] = [&l1, &l2];
    let (session_id, _) =
        durable_session_built(&host, Some((&left_run, &left_inputs[..])), None).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d2 = directed_step("queued directed by the predecessor");
    host.persist(&session_id, queued(&d2, 1)).await;
    let claim = host.claim(&session_id);

    let mut first = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the first attempt begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut first)
        .await
        .expect("the first drive answers");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut first, deadline())
        .await
        .expect("the sweep commits");
    assert_eq!(work.ended, vec![d2.id().clone()]);
    assert_eq!(sorted(work.answered), sorted_ids(&[&l1, &l2]));
    // The process ends here: the registration goes without a drain.
    drop(first);
    let rows = host.rows(&session_id).await;
    let outbox = stored_row(&rows, &d2)
        .state
        .interaction_terminal_outbox
        .as_ref()
        .expect("the ended directed input's outbox is durable");
    assert_eq!(
        outbox.owner_agent_runtime_id.as_deref(),
        Some(PLACED_RUNTIME),
        "instrument: the outbox names the witness's placement"
    );
    let (version, witness) = lifecycle_record(&host, &session_id).await;
    assert_ne!(
        version, 7,
        "instrument: the sweep commit wrote the row back"
    );
    assert_eq!(witness, None, "instrument: the row no longer records it");
    drop(claim);
    lineage_ended(&host, &session_id).await;

    let host = host.reopened();
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());
    let repeat = settle_whole(&host, &claim, &publisher).await;

    assert!(repeat.tail.is_none());
    assert!(repeat.work.ended.is_empty(), "the repeat ends nothing");
    assert!(
        repeat.work.answered.is_empty(),
        "the repeat answers nothing again"
    );
    let publication = repeat
        .publication
        .expect("the census finds every row closed");
    assert!(publication.owed.is_empty());
    assert_eq!(publication.durable_rows_closed, 3);
    assert_eq!(
        settled_input(&publication, &d2).settled_by,
        SettledBy::PendingTerminalConverged
    );
    for input in [&l1, &l2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::OutcomeUnknown);
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::OutcomeUnknown { run_id } if run_id == &left_run
        ));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 2, "{events:?}");
    assert!(matches!(
        published_for(&events, &d2),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "runtime stopped"
    ));
    assert!(matches!(
        published_for(&events, &l1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::OutcomeUnknown,
            ..
        }
    ));

    let again = settle_whole(&host, &claim, &publisher).await;
    assert!(
        again
            .publication
            .expect("a further repeat finds every row closed")
            .converged
            .is_empty()
    );
    assert_eq!(publisher.events().len(), 2, "nothing is published twice");
}

/// An outcome-unknown answer a stopped settlement left owed is a blocked
/// repair to the runless convergence an archive or a stop runs, not
/// corruption: the refusal names the repeat of the settlement, which alone
/// publishes the answer.
#[tokio::test]
async fn an_owed_outcome_unknown_answer_blocks_the_runless_convergence_for_the_repeat() {
    let host = Host::head_canonical();
    let (session_id, _) = durable_session(&host, None).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let left_run = RunId::new();
    let l1 = directed_step("consumed without a receipt");
    commit_left_boundary(&host, &session_id, &left_run, &[&l1]).await;
    let claim = host.claim(&session_id);

    let mut first = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut first)
        .await
        .expect("the drive answers");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut first, deadline())
        .await
        .expect("the sweep commits the outcome-unknown batch");
    assert_eq!(work.answered, vec![l1.id().clone()]);
    host.machine
        .remove_predecessor_settlement_registration_before(first, deadline())
        .await
        .expect("the registration goes without a drain");

    host.machine
        .register_session(session_id.clone())
        .await
        .expect("the session registers with its owed answer");
    let driver = {
        let sessions = host.machine.sessions.read().await;
        sessions
            .get(&session_id)
            .expect("the session's registration")
            .driver
            .clone()
    };
    let refused =
        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
            &driver,
            None,
            None,
            Some(deadline()),
        )
        .await
        .expect_err("the convergence does not publish the owed answer");
    assert!(
        matches!(
            &refused,
            RuntimeDriverError::RecoveryRepairBlocked { reason, .. }
                if reason.contains("repeat the settlement")
        ),
        "{refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Deadlines and publication inside the process-owned task
// ---------------------------------------------------------------------------

fn deadline_in(millis: u64) -> meerkat_core::time_compat::Instant {
    meerkat_core::time_compat::Instant::now() + std::time::Duration::from_millis(millis)
}

/// What a [`ScriptedPublisher`] does at its next publication; every later
/// one appends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Script {
    Append,
    /// Waits at the publication until released, then appends.
    HoldThenAppend,
    /// Fails before it appends anything.
    FailBeforeAppend,
    /// Appends, then fails before the caller learns of it (a crash between
    /// the event append and its receipt).
    FailAfterAppend,
    /// Panics before it appends anything.
    Panic,
}

/// A stored-only publisher scripted for its next publication. An event it
/// already appended (the same interaction, the same bytes) is answered with
/// the sequence it got, appending nothing, as an event store answers a
/// replay.
struct ScriptedPublisher {
    appended: std::sync::Mutex<Vec<(AgentEvent, u64)>>,
    next: std::sync::Mutex<Script>,
    entered: crate::tokio::sync::Notify,
    release: crate::tokio::sync::Notify,
}

impl ScriptedPublisher {
    fn new(script: Script) -> Self {
        Self {
            appended: std::sync::Mutex::new(Vec::new()),
            next: std::sync::Mutex::new(script),
            entered: crate::tokio::sync::Notify::new(),
            release: crate::tokio::sync::Notify::new(),
        }
    }

    fn events(&self) -> Vec<AgentEvent> {
        self.appended
            .lock()
            .unwrap()
            .iter()
            .map(|(event, _)| event.clone())
            .collect()
    }

    fn append(
        &self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        let mut appended = self.appended.lock().unwrap();
        events
            .iter()
            .map(|event| {
                let digest = crate::input_state::interaction_terminal_payload_digest(event)
                    .map_err(CoreExecutorError::Internal)?;
                let replayed = appended.iter().find_map(|(existing, seq)| {
                    crate::input_state::interaction_terminal_payload_digest(existing)
                        .is_ok_and(|existing_digest| existing_digest == digest)
                        .then_some(*seq)
                });
                let seq = match replayed {
                    Some(seq) => seq,
                    None => {
                        let seq = appended.len() as u64 + 1;
                        appended.push((event.clone(), seq));
                        seq
                    }
                };
                CoreInteractionTerminalPublicationReceipt::try_new(event, seq)
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl CoreExecutorPublicationHandle for ScriptedPublisher {
    async fn publish_interaction_terminals(
        &self,
        events: &[AgentEvent],
    ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
        let script = std::mem::replace(&mut *self.next.lock().unwrap(), Script::Append);
        match script {
            Script::Append => self.append(events),
            Script::HoldThenAppend => {
                self.entered.notify_one();
                self.release.notified().await;
                self.append(events)
            }
            Script::FailBeforeAppend => Err(CoreExecutorError::Internal(
                "the event store refused the append".to_string(),
            )),
            Script::FailAfterAppend => {
                self.append(events)?;
                Err(CoreExecutorError::Internal(
                    "the process stopped before the append's receipt".to_string(),
                ))
            }
            Script::Panic => panic!("the publisher panics before it appends anything"),
        }
    }
}

/// A queued prompt and a queued directed input of a placed predecessor,
/// with a committed session and no tail.
async fn queued_pair(host: &Host) -> (SessionId, Input, Input) {
    let (session_id, _) = durable_session(host, None).await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let q1 = prompt("queued by the predecessor");
    let d2 = directed_step("queued directed by the predecessor");
    host.persist(&session_id, queued(&q1, 1)).await;
    host.persist(&session_id, queued(&d2, 2)).await;
    (session_id, q1, d2)
}

/// A refusal of a registration of the session that never adopts it: what
/// every caller meets while the settlement's task owns the registration.
async fn registration_refused_unregister_in_progress(host: &Host, session_id: &SessionId) {
    let refused = host
        .machine
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::RegisterSession {
                session_id: session_id.clone(),
            },
        )
        .await
        .expect_err("the settlement's registration refuses a registration");
    assert!(
        matches!(
            refused,
            MeerkatMachineCommandError::Driver(RuntimeDriverError::UnregisterInProgress { .. })
        ),
        "{refused:?}"
    );
}

/// The caller's deadline passes before the task's drain finalized anything
/// (the drain waits on the internal registration's driver lock, which the
/// test holds). The call answers RuntimeTerminalPublicationInProgress with
/// final evidence only: nothing converged, both inputs it ended owed, by id
/// only, although their rows are durably ended with their carriers. Until
/// the task finishes, a registration of the session is refused, never
/// adopted, and a concurrent settlement waits for the task, holding no
/// gate, and refuses at its own deadline. The task then drains and removes
/// on its own, and a repeat finds nothing left to settle.
#[tokio::test]
async fn a_deadline_before_the_drain_reports_final_evidence_only_and_the_task_finishes_alone() {
    let host = Host::head_canonical();
    let (session_id, q1, d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the drive answers");
    let work = host
        .machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep commits");
    assert_eq!(sorted(work.ended), sorted_ids(&[&q1, &d2]));
    let driver = Arc::clone(
        &lease
            .internal
            .as_ref()
            .expect("the internal registration published")
            .driver,
    );
    let held = driver.lock().await;

    let refused = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline_in(200),
        )
        .await
        .expect_err("the deadline passes inside the drain");
    assert!(
        matches!(
            refused.refusal,
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. }
            )
        ),
        "{:?}",
        refused.refusal
    );
    assert!(
        refused.progress.converged.is_empty(),
        "no receipt is final yet"
    );
    assert_eq!(sorted(refused.progress.owed), sorted_ids(&[&q1, &d2]));
    assert_eq!(refused.progress.durable_rows_closed, 0);
    assert!(refused.progress.migrated_without_receipt.is_empty());
    let rows = host.rows(&session_id).await;
    for input in [&q1, &d2] {
        assert_eq!(
            stored_row(&rows, input).seed.phase,
            InputLifecycleState::Abandoned,
            "{} is durably ended",
            input.id()
        );
    }

    registration_refused_unregister_in_progress(&host, &session_id).await;
    let waiting = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline_in(100))
        .await
        .expect_err("a concurrent settlement waits for the task until its deadline");
    assert!(
        matches!(
            waiting,
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. }
            )
        ),
        "{waiting:?}"
    );
    drop(held);

    let repeat = settle_whole(&host, &claim, &Arc::new(RecordingPublisher::default())).await;
    let repeated = repeat
        .publication
        .expect("the repeat finds every row closed");
    assert!(
        repeated.converged.is_empty(),
        "the task converged everything on its own"
    );
    assert_eq!(repeated.durable_rows_closed, 2);
    let events = publisher.events();
    assert_eq!(events.len(), 1, "{events:?}");
    published_for(&events, &d2);
    let rows = host.rows(&session_id).await;
    for input in [&q1, &d2] {
        assert!(
            matches!(
                crate::terminal_status::input_terminal_receipt_read(&rows, input.id()),
                Ok(Some(InputTerminalReceiptRead::Finalized(_)))
            ),
            "{} has its final receipt",
            input.id()
        );
    }
}

/// The task's publication waits in the publisher (an event store held by
/// someone else) past the caller's deadline. The receipts were finalized
/// before the publication, so the call's progress lists the nondirected
/// input converged and the directed one owed, by id only, with no sequence.
/// While the publication waits, the task holds no gate: the internal
/// registration's driver lock and mutation gate are free, a registration
/// takes the registration transaction and is refused at the entry, and a
/// concurrent settlement waits for the task holding nothing. Released, the
/// publication appends once and the task finishes; a repeat finds nothing
/// left.
#[tokio::test]
async fn a_held_publication_holds_no_gate_and_finishes_once_released() {
    let host = Host::head_canonical();
    let (session_id, q1, d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(ScriptedPublisher::new(Script::HoldThenAppend));
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the drive answers");
    host.machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep commits");
    let internal = lease
        .internal
        .as_ref()
        .expect("the internal registration published");
    let driver = Arc::clone(&internal.driver);
    let mutation_gate = internal.mutation_gate.clone();

    let refused = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>,
            deadline_in(1_000),
        )
        .await
        .expect_err("the deadline passes while the publication waits");
    crate::tokio::time::timeout(BOUND, publisher.entered.notified())
        .await
        .expect("the task reached its publication");
    assert!(
        matches!(
            refused.refusal,
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. }
            )
        ),
        "{:?}",
        refused.refusal
    );
    let [converged] = refused.progress.converged.as_slice() else {
        panic!(
            "only the nondirected input converged: {:?}",
            refused.progress.converged
        );
    };
    assert_eq!(&converged.input_id, q1.id());
    assert_eq!(converged.settled_by, SettledBy::Ended);
    assert_eq!(refused.progress.owed, vec![d2.id().clone()]);

    drop(
        driver
            .try_lock()
            .expect("no driver lock is held across the publication"),
    );
    drop(
        crate::tokio::time::timeout(
            BOUND,
            mutation_gate.lock_settled(SettledGateWait::UnattachedSettlement),
        )
        .await
        .expect("no mutation gate is held across the publication"),
    );
    registration_refused_unregister_in_progress(&host, &session_id).await;
    let waiting = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline_in(100))
        .await
        .expect_err("a concurrent settlement waits for the task until its deadline");
    assert!(
        matches!(
            waiting,
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. }
            )
        ),
        "{waiting:?}"
    );
    publisher.release.notify_one();

    let repeat = settle_whole(&host, &claim, &Arc::new(RecordingPublisher::default())).await;
    let repeated = repeat
        .publication
        .expect("the repeat finds every row closed");
    assert!(repeated.converged.is_empty());
    assert_eq!(repeated.durable_rows_closed, 2);
    assert_eq!(publisher.events().len(), 1, "the terminal is appended once");
    assert!(!host.machine.contains_session(&session_id).await);
}

/// The publication fails before it appends: the receipts are final, the
/// directed input's terminal is not appended, so it stays owed (an id with
/// no sequence) and the nondirected one converged. The task still removes
/// its registration. The repeat appends the terminal and only then lists
/// the directed input, with that sequence.
#[tokio::test]
async fn a_terminal_whose_append_failed_stays_owed_until_the_repeat_appends_it() {
    let host = Host::head_canonical();
    let (session_id, q1, d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(ScriptedPublisher::new(Script::FailBeforeAppend));

    let settled = settle_whole(&host, &claim, &publisher).await;

    let refused = settled
        .publication
        .expect_err("the drain stops at the failed append");
    assert!(
        matches!(refused.refusal, PredecessorSettlementRefusal::Runtime(_)),
        "{:?}",
        refused.refusal
    );
    let [converged] = refused.progress.converged.as_slice() else {
        panic!(
            "only the nondirected input converged: {:?}",
            refused.progress.converged
        );
    };
    assert_eq!(&converged.input_id, q1.id());
    assert_eq!(refused.progress.owed, vec![d2.id().clone()]);
    assert!(publisher.events().is_empty());
    assert!(!host.machine.contains_session(&session_id).await);

    let repeat = settle_whole(&host, &claim, &publisher).await;
    let repeated = repeat
        .publication
        .expect("the repeat appends the terminal and finds every row closed");
    let appended = settled_input(&repeated, &d2);
    assert_eq!(appended.settled_by, SettledBy::PendingTerminalConverged);
    assert_eq!(appended.published_terminal_seq, Some(1));
    assert_eq!(publisher.events().len(), 1);
}

/// The publication appends the terminal and the process stops before the
/// append's receipt: the input stays owed. The repeat's append answers the
/// same terminal as a replay, appending nothing, and the outbox receipt
/// lands once, with the sequence of the first append.
#[tokio::test]
async fn a_terminal_appended_before_its_receipt_is_replayed_once_by_the_repeat() {
    let host = Host::head_canonical();
    let (session_id, _q1, d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(ScriptedPublisher::new(Script::FailAfterAppend));

    let settled = settle_whole(&host, &claim, &publisher).await;

    let refused = settled
        .publication
        .expect_err("the drain stops before the append's receipt");
    assert_eq!(refused.progress.owed, vec![d2.id().clone()]);
    assert_eq!(publisher.events().len(), 1, "the terminal was appended");

    let repeat = settle_whole(&host, &claim, &publisher).await;
    let repeated = repeat
        .publication
        .expect("the repeat converges the replayed terminal");
    let replayed = settled_input(&repeated, &d2);
    assert_eq!(replayed.settled_by, SettledBy::PendingTerminalConverged);
    assert_eq!(replayed.published_terminal_seq, Some(1));
    assert_eq!(publisher.events().len(), 1, "the replay appended nothing");
}

// ---------------------------------------------------------------------------
// The mixed predecessor
// ---------------------------------------------------------------------------

/// Everything at once (a tail with its contributors, queued and cancelled
/// work beside it, and the inputs an earlier boundary of another run
/// consumed without a receipt): the writer drive commits the
/// tail with its contributors' batch (the earlier run left no batch, so the
/// tail's is the only run-scoped one); one sweep commit ends the queued
/// inputs `Stopped`, answers the earlier run's inputs outcome unknown and
/// drops the witness; the task publishes the four directed terminals.
#[tokio::test]
async fn a_mixed_predecessor_is_settled_by_one_sweep_commit() {
    let host = Host::head_canonical();
    let left_run = RunId::new();
    let l1 = directed_step("consumed without a receipt");
    let l2 = prompt("consumed without a receipt too");
    let left_inputs: [&Input; 2] = [&l1, &l2];
    let (session_id, run_id) = durable_session_built(
        &host,
        Some((&left_run, &left_inputs[..])),
        Some(Box::new(completed_tail) as TailFn),
    )
    .await;
    host.lifecycle_row(&session_id, predecessor_placement(), None)
        .await;
    let d1 = directed_step("the run's directed contributor");
    let p1 = prompt("the run's prompt contributor");
    let q1 = prompt("queued beside the run");
    let d2 = directed_step("queued directed beside the run");
    let x0 = directed_step("cancelled while queued");
    host.persist(&session_id, applied_to(&d1, &run_id)).await;
    host.persist(&session_id, applied_to(&p1, &run_id)).await;
    host.persist(&session_id, queued(&q1, 1)).await;
    host.persist(&session_id, queued(&d2, 2)).await;
    host.persist(
        &session_id,
        cancelled_while_queued(&session_id, &x0, "cancelled by its caller").await,
    )
    .await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(RecordingPublisher::default());

    let settled = settle_whole(&host, &claim, &publisher).await;

    let tail = settled.tail.expect("the tail is committed");
    assert_eq!(tail.run_id, run_id);
    assert_eq!(
        tail.disposition,
        DurableTailRecoveryDisposition::CommitCompletedRetainInputs
    );
    assert_eq!(sorted(settled.work.ended), sorted_ids(&[&q1, &d2]));
    assert_eq!(sorted(settled.work.answered), sorted_ids(&[&l1, &l2]));
    let publication = settled
        .publication
        .expect("the census finds every row closed");
    assert!(publication.owed.is_empty());
    assert_eq!(publication.durable_rows_closed, 7);
    for input in [&d1, &p1] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::TailConsumed);
        assert_eq!(settled.receipt.run_id(), Some(&run_id));
    }
    for input in [&q1, &d2] {
        assert_eq!(
            settled_input(&publication, input).settled_by,
            SettledBy::Ended
        );
    }
    assert_eq!(
        settled_input(&publication, &x0).settled_by,
        SettledBy::PendingTerminalConverged
    );
    for input in [&l1, &l2] {
        let settled = settled_input(&publication, input);
        assert_eq!(settled.settled_by, SettledBy::OutcomeUnknown);
        assert!(matches!(
            settled.receipt.outcome(),
            CompletionOutcome::OutcomeUnknown { run_id } if run_id == &left_run
        ));
    }
    let events = publisher.events();
    assert_eq!(events.len(), 4, "{events:?}");
    assert!(matches!(
        published_for(&events, &d1),
        AgentEvent::InteractionComplete { .. }
    ));
    assert!(matches!(
        published_for(&events, &d2),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "runtime stopped"
    ));
    assert!(matches!(
        published_for(&events, &x0),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::Abandoned { detail },
            ..
        } if detail == "cancelled by its caller"
    ));
    assert!(matches!(
        published_for(&events, &l1),
        AgentEvent::InteractionFailed {
            reason: InteractionFailureReason::OutcomeUnknown,
            ..
        }
    ));
    assert_eq!(host.placement_witness(&session_id).await, None);
    assert!(!host.machine.contains_session(&session_id).await);
}

/// An executor no settlement registration ever runs.
struct IdleExecutor;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for IdleExecutor {
    async fn apply(
        &mut self,
        _run_id: RunId,
        _primitive: meerkat_core::lifecycle::RunPrimitive,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, CoreExecutorError> {
        Err(CoreExecutorError::Internal(
            "a settlement registration never runs".to_string(),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// While the settlement's process-owned task holds the internal
/// registration (its publication waits in a publisher the test holds),
/// every caller that could make it live or end it is refused at its first
/// decision, and none changes it:
/// - a registration, an explicit unregister, a quiescent close of its exact
///   witness, an explicit stop and an executor claim: typed
///   `UnregisterInProgress`;
/// - an exact-registration materialization of its witness:
///   `RegistrationOwned`;
/// - a retire, a reset, a destroy and an admission: the same refusal, and
///   the admission writes no row.
/// The entry keeps no unregister or stop coordinator, its registration use
/// and its vacant materialization claim. Released, finish answers `Ok`, the
/// registration is gone, and a fresh registration inserts.
#[tokio::test]
async fn every_live_caller_is_refused_while_the_settlement_task_holds_the_registration() {
    let host = Host::head_canonical();
    let (session_id, _q1, _d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let publisher = Arc::new(ScriptedPublisher::new(Script::HoldThenAppend));
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the drive answers");
    host.machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep commits");
    let use_before = {
        let sessions = host.machine.sessions.read().await;
        sessions
            .get(&session_id)
            .expect("the settlement's registration stands")
            .registration_use()
    };
    let finishing = {
        let machine = host.machine.clone();
        let publisher = Arc::clone(&publisher) as Arc<dyn CoreExecutorPublicationHandle>;
        crate::tokio::spawn(async move {
            machine
                .finish_predecessor_settlement_before(lease, publisher, deadline())
                .await
        })
    };
    crate::tokio::time::timeout(BOUND, publisher.entered.notified())
        .await
        .expect("the task reached its publication");
    let in_progress = |error: &dyn std::fmt::Display| {
        error
            .to_string()
            .contains("Unregister teardown is still in progress")
    };

    registration_refused_unregister_in_progress(&host, &session_id).await;
    let unregistering = host
        .machine
        .unregister_session(&session_id)
        .await
        .expect_err("an explicit unregister is refused");
    assert!(
        matches!(
            unregistering,
            RuntimeDriverError::UnregisterInProgress { .. }
        ),
        "{unregistering:?}"
    );
    let witness = host
        .machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("the settlement's registration has a witness");
    let closing = host
        .machine
        .unregister_session_registration_until_terminal_if_quiescent(&witness)
        .await
        .expect_err("a quiescent close is refused");
    assert!(
        matches!(closing, RuntimeDriverError::UnregisterInProgress { .. }),
        "{closing:?}"
    );
    let stopping = host
        .machine
        .stop_runtime_executor(&session_id, "an explicit stop")
        .await
        .expect_err("an explicit stop is refused");
    assert!(
        matches!(stopping, RuntimeDriverError::UnregisterInProgress { .. }),
        "{stopping:?}"
    );
    let claiming = Arc::new(host.machine.clone())
        .ensure_session_with_executor(session_id.clone(), Box::new(IdleExecutor))
        .await
        .expect_err("an executor claim is refused");
    assert!(
        matches!(claiming, RuntimeDriverError::UnregisterInProgress { .. }),
        "{claiming:?}"
    );
    let materializing = Arc::new(host.machine.clone())
        .prepare_local_session_materialization_for_registration(witness.clone())
        .await
        .expect_err("an exact-registration materialization is refused");
    assert!(
        matches!(materializing, RuntimeBindingsError::RegistrationOwned(_)),
        "{materializing:?}"
    );
    let destroying = host
        .machine
        .execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::Destroy {
                runtime_id: LogicalRuntimeId::for_session(&session_id),
            },
        )
        .await
        .expect_err("a destroy is refused");
    assert!(in_progress(&destroying), "{destroying:?}");
    let retiring = SessionServiceRuntimeExt::retire_runtime(&host.machine, &session_id)
        .await
        .expect_err("a retire is refused");
    assert!(in_progress(&retiring), "{retiring:?}");
    let resetting = SessionServiceRuntimeExt::reset_runtime(&host.machine, &session_id)
        .await
        .expect_err("a reset is refused");
    assert!(in_progress(&resetting), "{resetting:?}");
    let admitting = SessionServiceRuntimeExt::accept_input(
        &host.machine,
        &session_id,
        prompt("admitted while the settlement owns the registration"),
    )
    .await
    .expect_err("an admission is refused");
    assert!(in_progress(&admitting), "{admitting:?}");
    assert_eq!(
        host.rows(&session_id).await.len(),
        2,
        "the refused admission wrote no row"
    );
    {
        let sessions = host.machine.sessions.read().await;
        let entry = sessions
            .get(&session_id)
            .expect("the settlement's registration stands");
        assert!(entry.is_predecessor_settlement());
        assert!(
            entry.unregister_coordinator.is_none(),
            "no unregister started"
        );
        assert!(
            entry.runtime_stop_cleanup_coordinator.is_none(),
            "no stop cleanup started"
        );
        assert!(entry.reload_required_discard_coordinator.is_none());
        assert_eq!(entry.registration_use(), use_before, "nobody adopted it");
        let claim_state = entry
            .materialization_claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(claim_state.current.is_none());
        assert!(matches!(
            claim_state.phase,
            crate::RuntimeActorMaterializationClaimPhase::Vacant
        ));
    }
    publisher.release.notify_one();

    let publication = finishing
        .await
        .expect("the finish task joins")
        .expect("the task removes its registration exactly and the census is complete");
    assert_eq!(publication.durable_rows_closed, 2);
    assert!(!host.machine.contains_session(&session_id).await);
    host.machine
        .register_session(session_id.clone())
        .await
        .expect("a fresh registration inserts");
}

/// A settlement task that panics frees its slot and tells its waiters: the
/// finish answers at once that the task ended without a result, and a later
/// begin answers at once instead of waiting out its deadline on a slot no
/// task would free. It answers `UnregisterInProgress`: the panic left the
/// marked internal registration in place, which refuses every registration
/// and repeat until the process exits.
#[tokio::test]
async fn a_panicking_settlement_task_frees_its_slot() {
    let host = Host::head_canonical();
    let (session_id, _q1, _d2) = queued_pair(&host).await;
    let claim = host.claim(&session_id);
    let mut lease = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect("the settlement begins");
    host.machine
        .settle_predecessor_tail_with_lease(&mut lease)
        .await
        .expect("the drive answers");
    host.machine
        .end_predecessor_work_with_lease_before(&mut lease, deadline())
        .await
        .expect("the sweep commits");

    let refused = host
        .machine
        .finish_predecessor_settlement_before(
            lease,
            Arc::new(ScriptedPublisher::new(Script::Panic)),
            deadline(),
        )
        .await
        .expect_err("a task that panics gives no result");
    assert!(
        matches!(
            refused.refusal,
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::Internal(_))
        ),
        "{:?}",
        refused.refusal
    );

    let started = std::time::Instant::now();
    let later = host
        .machine
        .begin_predecessor_settlement_before(&claim, deadline())
        .await
        .expect_err("the panic left the internal registration in place");
    assert!(
        matches!(
            later,
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::UnregisterInProgress { .. })
        ),
        "{later:?}"
    );
    assert!(
        started.elapsed() < BOUND / 2,
        "the later begin waited {:?} on the slot of a task that ended",
        started.elapsed()
    );
    assert!(
        !host
            .machine
            .pending_predecessor_settlements
            .lock()
            .unwrap()
            .contains_key(&session_id),
        "the slot is free"
    );
}
