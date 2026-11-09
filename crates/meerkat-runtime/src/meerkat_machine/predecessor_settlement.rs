//! Settlement of a cold session's predecessor runtime.
//!
//! An earlier incarnation of a session can leave runtime work behind it: a
//! durable tail its boundary never committed, inputs that never ended,
//! terminals whose receipt or publication it still owed, and inputs a
//! recovered boundary consumed without staging a receipt. A host that takes
//! the session over settles that work here, under its own exclusive hosting
//! claim, before the session's next registration, without serving it:
//!
//! 1. [`MeerkatMachine::begin_predecessor_settlement_before`] checks the
//!    claim, holds the session's registration transaction (T), reads every
//!    durable input row (a row it cannot account for refuses before anything
//!    is touched) and reads the placement witness the outboxes of directed
//!    work name.
//! 2. [`MeerkatMachine::settle_predecessor_tail_with_lease`] commits the
//!    tail as a writer while nothing is registered (its commit stages the
//!    terminal batch of the inputs it consumes when its gates allow), and,
//!    after the work is ended, drives it once more as ordinary recovery when
//!    the first drive held it.
//! 3. [`MeerkatMachine::end_predecessor_work_with_lease_before`] publishes an
//!    internal, non-serving registration (consent owner loss completes at
//!    its publication; its converged lifecycle row keeps the placement
//!    witness while directed work still needs it), takes its gates, and in
//!    one commit ends every non-terminal input `Abandoned { Stopped }` with
//!    its runless terminal and answers every input a recovered boundary
//!    consumed without a receipt with its outcome-unknown batch.
//! 4. [`MeerkatMachine::finish_predecessor_settlement_before`] releases every
//!    gate and hands the rest to one process-owned task: the runtime loop's
//!    startup terminal drain with a stored-only publisher, then, under T and
//!    the registration's mutation gate, a closing census of every durable
//!    input row and the exact removal of the internal registration. A
//!    settlement that stops before its drain removes the registration with
//!    [`MeerkatMachine::remove_predecessor_settlement_registration_before`]
//!    instead, publishing nothing.
//!
//! The internal registration carries a marker that refuses every path that
//! could make it live (adoption, attachment, admission, lifecycle mutation,
//! unregister) with `UnregisterInProgress`; only the settlement removes it,
//! exactly and without retiring anything.

use std::collections::HashMap;

use super::*;
use crate::input_state::{
    InputAbandonReason, InputTerminalCompletionReadError, InputTerminalOutcome,
    InteractionTerminalPlacement, StoredInputState,
};
use crate::recovery::{
    DurableTailDriveRole, DurableTailRecoveryDisposition, DurableTailRecoveryError,
    DurableTailRecoveryOutcome,
};
use crate::terminal_status::{InputTerminalReceipt, InputTerminalReceiptRead};
use meerkat_core::interaction::InteractionId;
use meerkat_core::session_hosting::{HostingClaim, HostingOwnerLossProof};

/// The reason a settlement gives the inputs it ends.
const PREDECESSOR_SETTLEMENT_TERMINATION_REASON: &str = "runtime stopped";

/// What a predecessor settlement of one session made durable.
///
/// On success it is the complete census: every input the settlement found
/// non-terminal, terminal but owing its receipt or publication, or left
/// without a receipt by a recovered boundary, is listed once with its
/// finalized receipt, nothing is owed, and every durable input row of the
/// session read closed. As the progress of a refused settlement it carries
/// final evidence only: the inputs that fully converged, and the ids of the
/// inputs still owing a receipt or an append.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ColdPredecessorSettlement {
    pub session_id: SessionId,
    /// The predecessor's tail as this settlement committed it, or as an
    /// earlier exact commit of the same tail left it. `None`: no tail stood.
    pub tail: Option<PredecessorTailCommit>,
    /// On success: every input the settlement found non-terminal, terminal
    /// but owing a receipt or a publication, or left without a receipt by a
    /// recovered boundary, exactly once. As progress: only the inputs that
    /// fully converged (receipt finalized and, when directed, terminal
    /// appended with its outbox receipt).
    pub inputs: Vec<SettledInput>,
    /// The consent owner-loss outcome of the claim's lineage. `None` only in
    /// a refusal's progress, before the internal registration published.
    pub owner_loss: Option<PredecessorOwnerLoss>,
    /// Always empty on success. As progress: every input the settlement made
    /// or found terminal whose receipt is not finalized yet, or, when
    /// directed, whose terminal is not appended with its outbox receipt yet.
    /// Ids only: none of them has a final receipt or sequence. Their rows and
    /// carriers are durable; the process-owned task or a repeat converges
    /// them. A non-terminal row, or a terminal row with no carrier, is never
    /// listed here.
    pub terminals_owed: Vec<InputId>,
    /// On success: the number of durable input rows of the session the
    /// closing census read, every one closed. Zero as progress.
    pub durable_rows_closed: u64,
    /// On success: every row the closing census found terminal with the
    /// migration marker, whose receipt is permanently unavailable. The
    /// settlement never changes or publishes them. Empty as progress.
    pub migrated_without_receipt: Vec<InputId>,
}

impl ColdPredecessorSettlement {
    /// The progress of a settlement that made nothing durable.
    #[must_use]
    pub fn untouched(session_id: SessionId) -> Self {
        Self {
            session_id,
            tail: None,
            inputs: Vec::new(),
            owner_loss: None,
            terminals_owed: Vec::new(),
            durable_rows_closed: 0,
            migrated_without_receipt: Vec::new(),
        }
    }
}

/// The predecessor's durable tail as a settlement committed it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PredecessorTailCommit {
    /// The run the tail belonged to.
    pub run_id: RunId,
    /// `CommitCompleted`, `CommitCompletedRetainInputs` or
    /// `RepairAndCommitInterrupted`.
    pub disposition: DurableTailRecoveryDisposition,
    /// The recovered boundary's sequence.
    pub boundary_sequence: u64,
}

/// One input a settlement settled, with its final evidence.
///
/// Its receipt is finalized, and a directed input's interaction terminal is
/// appended with its outbox receipt: an input short of that is never a
/// `SettledInput`, it is an id in
/// [`ColdPredecessorSettlement::terminals_owed`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SettledInput {
    pub input_id: InputId,
    /// For a directed input: the interaction whose requester the published
    /// terminal answers.
    pub interaction_id: Option<InteractionId>,
    pub settled_by: SettledBy,
    /// The exact finalized receipt, read back from the store.
    pub receipt: InputTerminalReceipt,
    /// For a directed input: the event-store sequence of its published
    /// interaction terminal. `None` for a nondirected input.
    pub published_terminal_seq: Option<u64>,
}

/// How a settled input reached its terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SettledBy {
    /// A contributor the recovered boundary consumed and staged a receipt
    /// for: receipt scope the tail's run, `CompletedWithoutResult` for a
    /// completed tail, `Cancelled` for an interrupted one.
    TailConsumed,
    /// Ended by the settlement: `Abandoned { Stopped }`, receipt scope runtime
    /// termination, outcome runtime terminated ("runtime stopped").
    Ended,
    /// Terminal before the settlement with its receipt or publication still
    /// owed; the settlement finalized and published it.
    PendingTerminalConverged,
    /// Consumed by a run at a recovered boundary that staged no receipt, and
    /// answered by a settlement: terminal `Consumed`, receipt scope runtime
    /// termination, outcome `OutcomeUnknown { run_id }` naming that run; a
    /// directed one's requester got an interaction failure saying the
    /// outcome is unknown. Read from the receipt's outcome, whichever
    /// settlement staged the answer, so it takes precedence over
    /// `PendingTerminalConverged`.
    OutcomeUnknown,
}

/// The consent owner-loss outcome of a settlement's claim lineage.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PredecessorOwnerLoss {
    /// The settlement's internal registration reserved the lineage's owed
    /// proof, the installed issuing consent owner settled every consent the
    /// lost owner issued in the proof's scope, and the reservation completed
    /// at the registration's winning publication.
    Completed(HostingOwnerLossProof),
    /// The same live lineage had already completed that exact settlement at
    /// an earlier winning publication; the proof is the one it settled.
    NotOwed(HostingOwnerLossProof),
    /// No issuing consent owner is installed on the machine: nothing was
    /// reserved, settled or recorded complete, and a lineage that owed its
    /// settlement still owes it. Never a completion.
    NoConsentOwner,
}

/// Why a predecessor settlement refused, or stopped.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PredecessorSettlementRefusal {
    /// A registration of the session exists.
    #[error("the session has a runtime registration")]
    Registered,
    /// The claim is not this runtime owner's claim of the session (another
    /// session, another owner or another hosting domain).
    #[error("the hosting claim is not this runtime owner's claim of the session")]
    ClaimNotOwned,
    /// The claim proves no cross-process exclusion of the session.
    #[error("the hosting claim proves no exclusive cross-process ownership of the session")]
    ExclusivityUnproven,
    /// The runtime store's session persistence profile is not supported.
    #[error("predecessor settlement needs a HeadCanonical runtime store, not {profile}")]
    ProfileUnsupported {
        profile: crate::store::RuntimeSessionPersistenceProfile,
    },
    /// The durable tail held after the settlement ended the work beside it.
    #[error("the predecessor's durable tail is held for recovery")]
    TailHeld,
    /// Recovery refused the durable tail.
    #[error("recovery refused the predecessor's durable tail")]
    TailRefused,
    /// A durable-tail drive failed.
    #[error("durable-tail recovery failed: {0}")]
    Tail(DurableTailRecoveryError),
    /// A typed runtime refusal or failure.
    #[error(transparent)]
    Runtime(RuntimeDriverError),
    /// Input rows the settlement cannot account for, read when it begins:
    /// `undecodable` holds the stored keys of rows whose bytes do not decode
    /// (such a row has no typed id), and `unproven` decodable terminal rows
    /// the exact receipt reader cannot answer that are not inputs a recovered
    /// boundary left without a receipt (a consumed row with a run and no
    /// durable boundary receipt, an abandoned row without its batch, a
    /// finalized directed receipt without its outbox). Nothing was touched.
    #[error(
        "{} input rows do not decode and {} terminal input rows cannot be accounted for",
        undecodable.len(),
        unproven.len()
    )]
    UnsettleableRows {
        undecodable: Vec<String>,
        unproven: Vec<InputId>,
    },
    /// Directed inputs need a new terminal outbox, which names a runtime
    /// placement, and the session's lifecycle row records neither a complete
    /// binding nor a placement witness. Nothing was touched.
    #[error(
        "{} directed inputs need a terminal outbox and the session records no runtime placement",
        directed.len()
    )]
    PlacementWitnessMissing { directed: Vec<InputId> },
    /// The closing census found durable input rows that are not closed.
    /// `open` holds rows still non-terminal, `unreceipted` terminal rows the
    /// exact receipt reader cannot account for (neither a finalized receipt
    /// nor a carrier, or a finalized directed receipt without its outbox),
    /// outside the machine's receipt-less transitions and the migration
    /// marker; neither implies a durable terminal or carrier. A terminal row
    /// whose receipt or append is pending is an owed id of the progress
    /// instead, so both lists may be empty.
    #[error(
        "the closing census found input rows not closed ({} open, {} unreceipted)",
        open.len(),
        unreceipted.len()
    )]
    CensusIncomplete {
        open: Vec<InputId>,
        unreceipted: Vec<InputId>,
    },
}

/// What the internal registration and its sweep did.
#[derive(Debug)]
#[non_exhaustive]
pub struct PredecessorWork {
    /// The claim lineage's consent owner-loss outcome at the internal
    /// registration's publication.
    pub owner_loss: PredecessorOwnerLoss,
    /// The inputs the sweep ended `Abandoned { Stopped }`.
    pub ended: Vec<InputId>,
    /// The inputs a recovered boundary consumed without a receipt that the
    /// sweep answered with their outcome-unknown batch.
    pub answered: Vec<InputId>,
}

/// What the settlement's process-owned task made final.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct PredecessorPublication {
    /// The inputs the settlement settled whose receipt is finalized and, when
    /// directed, whose interaction terminal is appended with its outbox
    /// receipt.
    pub converged: Vec<SettledInput>,
    /// The terminal inputs still owing their receipt or their append.
    pub owed: Vec<InputId>,
    /// The number of durable input rows the closing census read closed; zero
    /// when no census completed.
    pub durable_rows_closed: u64,
    /// The rows the closing census found with the migration marker; empty
    /// when no census completed.
    pub migrated_without_receipt: Vec<InputId>,
}

/// A refused or stopped settlement finish, with the final evidence it made
/// durable before it stopped.
#[derive(Debug)]
#[non_exhaustive]
pub struct PredecessorPublicationRefused {
    pub refusal: PredecessorSettlementRefusal,
    pub progress: PredecessorPublication,
}

/// Settlement custody of one cold session's predecessor runtime. Not `Clone`.
///
/// Holds the session's registration transaction (T) from begin through the
/// sweep and the second drive (released only to conclude a predecessor's
/// cold-recovered unregister drain, and when finish starts its task); after
/// the internal registration publishes also its live lifecycle lease (L) and
/// mutation gate (M). Carries the placement witness read at begin. Dropping
/// it without finish hands the exact removal of the internal registration,
/// without publication, to the same process-owned task kind.
pub struct MachinePredecessorSettlementLease {
    machine: MeerkatMachine,
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    /// A lineage clone of the caller's claim, dropped with the lease: the
    /// caller's own claim keeps the lineage and its OS lock alive.
    claim: HostingClaim,
    census: BeginCensus,
    /// The placement the outboxes of directed work name, read at begin.
    placement: Option<InteractionTerminalPlacement>,
    /// The consent owner-loss outcome, kept from the internal registration's
    /// publication on, so every later refusal reports it.
    owner_loss: Option<PredecessorOwnerLoss>,
    tail_held: bool,
    internal: Option<PublishedSettlementRegistration>,
    runless_terminal: RunlessTerminalAnnouncement,
    mutation_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
    live_lifecycle_lease: Option<crate::member_live::MemberLiveLifecycleLease>,
    registration_guard: Option<crate::tokio::sync::OwnedMutexGuard<()>>,
}

impl std::fmt::Debug for MachinePredecessorSettlementLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MachinePredecessorSettlementLease")
            .field("session_id", &self.session_id)
            .field("tail_held", &self.tail_held)
            .field("registered", &self.internal.is_some())
            .finish_non_exhaustive()
    }
}

impl MachinePredecessorSettlementLease {
    /// The session this lease settles.
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Whether the settlement's first drive held the tail beside work that
    /// ending it can resolve: the second drive is owed after the sweep.
    #[must_use]
    pub fn tail_held(&self) -> bool {
        self.tail_held
    }

    /// Whether the internal registration published.
    #[must_use]
    pub fn registered(&self) -> bool {
        self.internal.is_some()
    }

    /// The consent owner-loss outcome of the claim's lineage, once the
    /// internal registration published: `None` before that, and when the
    /// publication could not establish one.
    #[must_use]
    pub fn owner_loss(&self) -> Option<&PredecessorOwnerLoss> {
        self.owner_loss.as_ref()
    }
}

impl Drop for MachinePredecessorSettlementLease {
    fn drop(&mut self) {
        // A lease dropped without finish (a cancelled caller, an error path)
        // hands its exact removal, without publication, to the process-owned
        // task kind. The task takes T and M once this lease's own guards,
        // dropped right after this, are released.
        let Some(internal) = self.internal.take() else {
            return;
        };
        let census = std::mem::take(&mut self.census);
        if let Err(error) = self.machine.start_predecessor_settlement_task(
            self.session_id.clone(),
            internal,
            None,
            census,
            None,
        ) {
            tracing::error!(
                session_id = %self.session_id,
                %error,
                "a dropped predecessor settlement could not hand its registration's removal to a \
                 process-owned task; the registration refuses registrations until the process exits"
            );
        }
    }
}

/// The inputs a settlement found when it began, under T, and how it found
/// each of them.
#[derive(Debug, Clone, Default)]
struct BeginCensus {
    found: HashMap<InputId, FoundAt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundAt {
    /// Not terminal yet.
    NonTerminal,
    /// Terminal, with its receipt or its publication still owed.
    OwingTerminal,
    /// Consumed by a run at a recovered boundary that staged no receipt.
    OutcomeUnknownRow,
}

/// The internal registration a settlement published, with the identities its
/// exact removal is checked against.
pub(super) struct PublishedSettlementRegistration {
    pub(super) runtime_id: LogicalRuntimeId,
    pub(super) driver: SharedDriver,
    pub(super) completions: SharedCompletionRegistry,
    pub(super) post_commit_hooks: Arc<meerkat_core::PostCommitHookDispatcher>,
    pub(super) mutation_gate: SessionMutationGate,
    pub(super) epoch_id: meerkat_core::RuntimeEpochId,
    pub(super) registration: RuntimeSessionRegistrationWitness,
    pub(super) registration_use: RuntimeSessionRegistrationUse,
    /// Whether an issuing consent owner was installed at the publication.
    pub(super) consent_owner_installed: bool,
    /// The proof this publication's owner-loss reservation completed with.
    pub(super) completed_owner_loss: Option<HostingOwnerLossProof>,
    /// Whether the registration came back as a cold-recovered unregister
    /// drain, published unmarked for the ordinary unregister to conclude.
    pub(super) cold_recovered_draining: bool,
}

/// Why a settlement's registration did not publish.
pub(super) enum SettlementRegistrationRefused {
    Registered,
    Runtime(RuntimeDriverError),
}

/// The per-session slot of a settlement's process-owned task.
pub(super) struct PendingPredecessorSettlement {
    task_id: uuid::Uuid,
    done_rx: crate::tokio::sync::watch::Receiver<bool>,
}

/// A running settlement task's hold on its slot: dropped when the task ends,
/// by return or by panic, it frees the slot (when it is still the task's)
/// and tells every waiter. A task that panics before its exact removal
/// leaves the marked internal registration in place: it refuses every
/// registration and every repeat of the settlement (`UnregisterInProgress`)
/// until the process exits.
struct PredecessorSettlementSlot {
    machine: MeerkatMachine,
    session_id: SessionId,
    task_id: uuid::Uuid,
    done_tx: crate::tokio::sync::watch::Sender<bool>,
}

impl Drop for PredecessorSettlementSlot {
    fn drop(&mut self) {
        {
            let mut pending = self
                .machine
                .pending_predecessor_settlements
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending
                .get(&self.session_id)
                .is_some_and(|pending| pending.task_id == self.task_id)
            {
                pending.remove(&self.session_id);
            }
        }
        self.done_tx.send_replace(true);
    }
}

/// How one decoded durable input row reads to a settlement.
enum RowReading {
    /// Not terminal yet: the tail consumes it or the sweep ends it.
    NonTerminal { directed: bool },
    /// Terminal, with a durable carrier still owing its receipt or its
    /// publication.
    Owing,
    /// Terminal and closed: its finalized receipt (`None` for a receipt-less
    /// terminal the machine's transitions produce) and, for a directed input,
    /// its published interaction terminal.
    Closed {
        receipt: Option<InputTerminalReceipt>,
        interaction_id: Option<InteractionId>,
        published_terminal_seq: Option<u64>,
    },
    /// Terminal with the migration marker: its receipt is permanently
    /// unavailable.
    Migrated,
    /// Consumed by `run_id` at a recovered boundary that staged no receipt.
    OutcomeUnknown { run_id: RunId, directed: bool },
    /// Terminal, and none of the above: no finalized receipt and no carrier
    /// outside the machine's receipt-less transitions, the migration marker
    /// and the outcome-unknown gate, or a finalized directed receipt without
    /// its outbox.
    Unproven,
}

fn remaining_until(deadline: meerkat_core::time_compat::Instant) -> Option<std::time::Duration> {
    deadline
        .checked_duration_since(meerkat_core::time_compat::Instant::now())
        .filter(|remaining| !remaining.is_zero())
}

fn store_unavailable(context: &str, error: crate::store::RuntimeStoreError) -> RuntimeDriverError {
    RuntimeDriverError::RecoveryBackoff {
        reason: format!("{context}: {error}"),
    }
}

/// The interaction a row's terminal publication answers: its outbox's, or
/// the one its retained payload names. `Err` when the payload does not
/// validate.
fn directed_interaction(stored: &StoredInputState) -> Result<Option<InteractionId>, String> {
    if let Some(outbox) = stored.state.interaction_terminal_outbox.as_ref() {
        return Ok(Some(outbox.interaction_id));
    }
    match stored.state.persisted_input.as_ref() {
        Some(input) => crate::input::validated_directed_interaction_id(input),
        None => Ok(None),
    }
}

/// Every durable input row of the session: the decoded rows, and the stored
/// keys of the rows that do not decode.
async fn load_session_rows(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<(Vec<StoredInputState>, Vec<String>), RuntimeDriverError> {
    let rows = store
        .load_input_states(runtime_id)
        .await
        .map_err(|error| store_unavailable("the session's input rows could not be read", error))?;
    let mut decoded = Vec::with_capacity(rows.len());
    let mut undecodable = Vec::new();
    for row in rows {
        match row {
            crate::store::InputStateRow::Decoded(stored) => decoded.push(*stored),
            crate::store::InputStateRow::Corrupt { input_id, .. } => undecodable.push(input_id),
        }
    }
    Ok((decoded, undecodable))
}

/// Read one decoded row against every row of the session (the exact receipt
/// reader validates the row's whole batch). The one place that decides what
/// a settlement makes of a row.
async fn read_row(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    rows: &[StoredInputState],
    stored: &StoredInputState,
) -> Result<RowReading, RuntimeDriverError> {
    if crate::store::input_state_is_recovery_nonterminal(stored) {
        return Ok(RowReading::NonTerminal {
            directed: !matches!(directed_interaction(stored), Ok(None)),
        });
    }
    let receipt_owed = stored
        .state
        .terminal_completion
        .as_ref()
        .is_some_and(|completion| {
            matches!(
                completion.phase,
                crate::input_state::InputTerminalCompletionPhase::Pending
            )
        });
    let publication_owed = stored
        .state
        .interaction_terminal_outbox
        .as_ref()
        .is_some_and(|outbox| {
            !matches!(
                outbox.phase,
                crate::input_state::InteractionTerminalOutboxPhase::Published { .. }
            )
        });
    if receipt_owed || publication_owed {
        return Ok(RowReading::Owing);
    }
    let Ok(interaction_id) = directed_interaction(stored) else {
        return Ok(RowReading::Unproven);
    };
    match crate::terminal_status::input_terminal_receipt_read(rows, &stored.state.input_id) {
        Ok(Some(InputTerminalReceiptRead::Finalized(receipt))) => {
            let Some(interaction_id) = interaction_id else {
                return Ok(RowReading::Closed {
                    receipt: Some(*receipt),
                    interaction_id: None,
                    published_terminal_seq: None,
                });
            };
            match stored
                .state
                .interaction_terminal_outbox
                .as_ref()
                .map(|outbox| &outbox.phase)
            {
                Some(crate::input_state::InteractionTerminalOutboxPhase::Published {
                    publication,
                    ..
                }) => Ok(RowReading::Closed {
                    receipt: Some(*receipt),
                    interaction_id: Some(interaction_id),
                    published_terminal_seq: Some(publication.terminal_seq),
                }),
                // A directed receipt without its published outbox: its
                // requester can never be answered.
                _ => Ok(RowReading::Unproven),
            }
        }
        Ok(Some(InputTerminalReceiptRead::TerminalWithoutReceipt { .. })) => {
            Ok(RowReading::Closed {
                receipt: None,
                interaction_id,
                published_terminal_seq: None,
            })
        }
        Ok(Some(InputTerminalReceiptRead::Pending { .. })) => Ok(RowReading::Owing),
        Ok(None) => Ok(RowReading::Unproven),
        Err(InputTerminalCompletionReadError::MigratedReceiptUnavailable) => {
            Ok(RowReading::Migrated)
        }
        Err(_) => outcome_unknown_gate(store, runtime_id, stored).await,
    }
}

/// The outcome-unknown gate: a decoded terminal row consumed by run `r` at
/// boundary sequence `n`, with no terminal completion, no outbox and no
/// migration marker, whose run's boundary receipt at `n` is durable. Only a
/// recovery commit that staged no batch leaves such a row (it installs the
/// receipt with the row); every other terminalizing path stages its batch in
/// the same transaction. A directed one keeps the payload that names its
/// interaction.
async fn outcome_unknown_gate(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    stored: &StoredInputState,
) -> Result<RowReading, RuntimeDriverError> {
    let seed = &stored.seed;
    let (Some(InputTerminalOutcome::Consumed), Some(run_id), Some(sequence)) = (
        seed.terminal_outcome.as_ref(),
        seed.last_run_id.as_ref(),
        seed.last_boundary_sequence,
    ) else {
        return Ok(RowReading::Unproven);
    };
    if stored.state.terminal_completion.is_some()
        || stored.state.interaction_terminal_outbox.is_some()
        || stored.state.terminal_completion_unavailable
    {
        return Ok(RowReading::Unproven);
    }
    let boundary = store
        .load_boundary_receipt(runtime_id, run_id, sequence)
        .await
        .map_err(|error| {
            store_unavailable("a recovered boundary's receipt could not be read", error)
        })?;
    if boundary.is_none() {
        return Ok(RowReading::Unproven);
    }
    let Ok(interaction_id) = directed_interaction(stored) else {
        return Ok(RowReading::Unproven);
    };
    Ok(RowReading::OutcomeUnknown {
        run_id: run_id.clone(),
        directed: interaction_id.is_some(),
    })
}

/// Read every durable input row of the session when a settlement begins,
/// under T and before anything is touched: the inputs it must settle and the
/// directed ones that need a new outbox. A row it cannot account for refuses
/// it, typed, with the rows unchanged.
async fn read_begin_census(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<(BeginCensus, Vec<InputId>), PredecessorSettlementRefusal> {
    let (decoded, undecodable) = load_session_rows(store, runtime_id)
        .await
        .map_err(PredecessorSettlementRefusal::Runtime)?;
    let mut census = BeginCensus::default();
    let mut directed_needing_outbox = Vec::new();
    let mut unproven = Vec::new();
    for stored in &decoded {
        let input_id = stored.state.input_id.clone();
        match read_row(store, runtime_id, &decoded, stored)
            .await
            .map_err(PredecessorSettlementRefusal::Runtime)?
        {
            RowReading::NonTerminal { directed } => {
                if directed {
                    directed_needing_outbox.push(input_id.clone());
                }
                census.found.insert(input_id, FoundAt::NonTerminal);
            }
            RowReading::Owing => {
                census.found.insert(input_id, FoundAt::OwingTerminal);
            }
            RowReading::OutcomeUnknown { directed, .. } => {
                if directed {
                    directed_needing_outbox.push(input_id.clone());
                }
                census.found.insert(input_id, FoundAt::OutcomeUnknownRow);
            }
            RowReading::Closed { .. } | RowReading::Migrated => {}
            RowReading::Unproven => unproven.push(input_id),
        }
    }
    if !undecodable.is_empty() || !unproven.is_empty() {
        return Err(PredecessorSettlementRefusal::UnsettleableRows {
            undecodable,
            unproven,
        });
    }
    Ok((census, directed_needing_outbox))
}

/// What a settlement's sweep reads of the rows after its tail drive: the
/// inputs a recovered boundary consumed without a receipt, with their runs,
/// and the directed rows that still need a new outbox.
struct SweepSelection {
    outcome_unknown: Vec<(StoredInputState, RunId)>,
    directed_needing_outbox: Vec<InputId>,
}

async fn read_sweep_selection(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<SweepSelection, PredecessorSettlementRefusal> {
    let (decoded, undecodable) = load_session_rows(store, runtime_id)
        .await
        .map_err(PredecessorSettlementRefusal::Runtime)?;
    if !undecodable.is_empty() {
        return Err(PredecessorSettlementRefusal::UnsettleableRows {
            undecodable,
            unproven: Vec::new(),
        });
    }
    let mut selection = SweepSelection {
        outcome_unknown: Vec::new(),
        directed_needing_outbox: Vec::new(),
    };
    for stored in &decoded {
        match read_row(store, runtime_id, &decoded, stored)
            .await
            .map_err(PredecessorSettlementRefusal::Runtime)?
        {
            RowReading::NonTerminal { directed: true } => selection
                .directed_needing_outbox
                .push(stored.state.input_id.clone()),
            RowReading::OutcomeUnknown { run_id, directed } => {
                if directed {
                    selection
                        .directed_needing_outbox
                        .push(stored.state.input_id.clone());
                }
                selection.outcome_unknown.push((stored.clone(), run_id));
            }
            _ => {}
        }
    }
    Ok(selection)
}

/// The placement the outboxes of a settlement's directed work name: the
/// session's lifecycle row's complete binding, else the placement witness the
/// row carries, else none. This is the one place that decides where it comes
/// from.
async fn read_placement_witness(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<InteractionTerminalPlacement>, PredecessorSettlementRefusal> {
    match store.observe_machine_lifecycle(runtime_id).await {
        Ok(crate::store::MachineLifecycleObservation::Decoded { record, .. }) => {
            Ok(InteractionTerminalPlacement::from_lifecycle_row(
                record.binding(),
                record.placement_witness(),
            ))
        }
        Ok(_) | Err(crate::store::RuntimeStoreError::Unsupported(_)) => Ok(None),
        Err(error) => Err(PredecessorSettlementRefusal::Runtime(store_unavailable(
            "the session's lifecycle row could not be read",
            error,
        ))),
    }
}

/// How an input the settlement found reached the receipt it now holds;
/// `None` when no step of the settlement explains it.
fn settled_by_of(found: FoundAt, receipt: &InputTerminalReceipt) -> Option<SettledBy> {
    if receipt.outcome().outcome_unknown_run().is_some() {
        return Some(SettledBy::OutcomeUnknown);
    }
    match found {
        FoundAt::OwingTerminal => Some(SettledBy::PendingTerminalConverged),
        FoundAt::OutcomeUnknownRow => None,
        FoundAt::NonTerminal => match receipt.terminal() {
            InputTerminalOutcome::Consumed if receipt.run_id().is_some() => {
                Some(SettledBy::TailConsumed)
            }
            InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped,
            } if receipt.run_id().is_none() => Some(SettledBy::Ended),
            _ => None,
        },
    }
}

/// The settlement's evidence over the durable rows.
#[derive(Default)]
struct SettlementEvidence {
    /// The settled inputs that fully converged.
    converged: Vec<SettledInput>,
    /// The settled inputs still owing their receipt or their append.
    owed: Vec<InputId>,
    /// Rows still non-terminal.
    open: Vec<InputId>,
    /// Terminal rows with neither a finalized receipt nor a carrier, outside
    /// the receipt-less transitions and the migration marker.
    unreceipted: Vec<InputId>,
    /// Settled inputs whose terminal no step of the settlement explains.
    unexplained: Vec<InputId>,
    /// Stored keys of rows that do not decode.
    undecodable: Vec<String>,
    /// Rows with the migration marker.
    migrated: Vec<InputId>,
    /// Whether any terminal row still owes its receipt or its append.
    any_owed: bool,
    /// Rows read closed.
    closed_rows: u64,
}

/// Read every durable row of the session for the settlement's evidence and
/// closing census.
async fn read_settlement_evidence(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    census: &BeginCensus,
) -> Result<SettlementEvidence, RuntimeDriverError> {
    let (decoded, undecodable) = load_session_rows(store, runtime_id).await?;
    let mut evidence = SettlementEvidence {
        undecodable,
        ..SettlementEvidence::default()
    };
    for stored in &decoded {
        let input_id = &stored.state.input_id;
        let found = census.found.get(input_id).copied();
        match read_row(store, runtime_id, &decoded, stored).await? {
            RowReading::NonTerminal { .. } => evidence.open.push(input_id.clone()),
            RowReading::Owing => {
                evidence.any_owed = true;
                if found.is_some() {
                    evidence.owed.push(input_id.clone());
                }
            }
            RowReading::Closed {
                receipt,
                interaction_id,
                published_terminal_seq,
            } => {
                evidence.closed_rows += 1;
                let Some(found) = found else {
                    continue;
                };
                let settled = receipt.and_then(|receipt| {
                    settled_by_of(found, &receipt).map(|settled_by| (receipt, settled_by))
                });
                match settled {
                    Some((receipt, settled_by)) => evidence.converged.push(SettledInput {
                        input_id: input_id.clone(),
                        interaction_id,
                        settled_by,
                        receipt,
                        published_terminal_seq,
                    }),
                    None => evidence.unexplained.push(input_id.clone()),
                }
            }
            RowReading::Migrated => {
                evidence.closed_rows += 1;
                evidence.migrated.push(input_id.clone());
            }
            RowReading::OutcomeUnknown { .. } | RowReading::Unproven => {
                evidence.unreceipted.push(input_id.clone());
            }
        }
    }
    Ok(evidence)
}

impl SettlementEvidence {
    /// The closing census: `Ok` with the complete publication when every
    /// durable row reads closed and every settled input is explained;
    /// otherwise the refusal, with the final evidence as its progress.
    #[allow(
        clippy::result_large_err,
        reason = "the refusal carries its final evidence inline, as the public result does"
    )]
    fn into_closing_census(self) -> Result<PredecessorPublication, PredecessorPublicationRefused> {
        let progress = PredecessorPublication {
            converged: self.converged,
            owed: self.owed,
            durable_rows_closed: 0,
            migrated_without_receipt: Vec::new(),
        };
        let refusal = if !self.undecodable.is_empty() {
            Some(PredecessorSettlementRefusal::UnsettleableRows {
                undecodable: self.undecodable,
                unproven: Vec::new(),
            })
        } else if !self.unexplained.is_empty() {
            Some(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::Internal(format!(
                    "the closing census found settled inputs whose terminal no step of the \
                     settlement explains: {}",
                    self.unexplained
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            ))
        } else if !self.open.is_empty() || !self.unreceipted.is_empty() || self.any_owed {
            Some(PredecessorSettlementRefusal::CensusIncomplete {
                open: self.open,
                unreceipted: self.unreceipted,
            })
        } else {
            None
        };
        match refusal {
            Some(refusal) => Err(PredecessorPublicationRefused { refusal, progress }),
            None => Ok(PredecessorPublication {
                durable_rows_closed: self.closed_rows,
                migrated_without_receipt: self.migrated,
                ..progress
            }),
        }
    }
}

impl MeerkatMachine {
    /// Begin settling the runtime work an earlier incarnation of
    /// `claim.session_id()` left, under `claim`, before the session's next
    /// registration.
    ///
    /// Waits first, holding no gate and bounded by `deadline`, for a
    /// settlement task an earlier call left running. Then checks the claim
    /// (this machine's own owner and hosting domain, a cross-process OS
    /// lock), requires a HeadCanonical store, takes the session's
    /// registration transaction (T), refuses an existing registration, reads
    /// every durable input row (a row it cannot account for refuses typed),
    /// and reads the placement witness the outboxes of directed work name
    /// (refused typed when directed work needs one and the session records
    /// none). Nothing is touched before every check passed.
    pub async fn begin_predecessor_settlement_before(
        &self,
        claim: &HostingClaim,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<MachinePredecessorSettlementLease, PredecessorSettlementRefusal> {
        let session_id = claim.session_id().clone();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        self.await_predecessor_settlement_task_before(&session_id, &runtime_id, deadline)
            .await?;
        let store = self.store.as_ref().map(Arc::clone).ok_or_else(|| {
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "session {session_id} has no durable runtime store, so no predecessor runtime \
                     to settle"
                ),
            })
        })?;
        self.check_predecessor_settlement_claim(claim)?;
        let profile = store.session_persistence_profile();
        if profile != crate::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1 {
            return Err(PredecessorSettlementRefusal::ProfileUnsupported { profile });
        }
        let registration_guard = self
            .lock_registration_transaction_before(&session_id, deadline)
            .await?;
        self.refuse_registered_under_transaction(&session_id)
            .await?;
        let (census, directed_needing_outbox) =
            read_begin_census(store.as_ref(), &runtime_id).await?;
        let placement = read_placement_witness(store.as_ref(), &runtime_id).await?;
        if placement.is_none() && !directed_needing_outbox.is_empty() {
            return Err(PredecessorSettlementRefusal::PlacementWitnessMissing {
                directed: directed_needing_outbox,
            });
        }
        Ok(MachinePredecessorSettlementLease {
            machine: self.clone(),
            session_id,
            runtime_id,
            claim: claim.clone(),
            census,
            placement,
            owner_loss: None,
            tail_held: false,
            internal: None,
            runless_terminal: self.runless_terminal_announcement(),
            mutation_guard: None,
            live_lifecycle_lease: None,
            registration_guard: Some(registration_guard),
        })
    }

    /// Drive the session's durable tail for a settlement. The caller holds
    /// the session's recovery gate (R).
    ///
    /// Before the internal registration: the writer drive under the lease's
    /// claim, while nothing is registered. A committed or already aligned
    /// tail goes on; a held one is remembered
    /// ([`MachinePredecessorSettlementLease::tail_held`]) for the second
    /// drive; a refused one refuses; a run the drive would continue means the
    /// exclusive-writer proof lapsed. After the internal registration:
    /// ordinary recovery, under the internal registration's driver lock; a
    /// tail it still holds or refuses ends the call typed.
    pub async fn settle_predecessor_tail_with_lease(
        &self,
        lease: &mut MachinePredecessorSettlementLease,
    ) -> Result<Option<PredecessorTailCommit>, PredecessorSettlementRefusal> {
        let store = self.require_settlement_store(lease)?;
        let session_id = lease.session_id.clone();
        // The run of the tail a drive may commit, read before the drive.
        let tail_run = self
            .provisional_tail_run(&session_id)
            .await
            .map_err(PredecessorSettlementRefusal::Runtime)?;
        let outcome = match lease.internal.as_ref() {
            None => {
                let claim = lease.claim.clone();
                meerkat_core::session_hosting::with_write_hosting(
                    claim.clone(),
                    crate::recovery::recover_durable_tail_under_hosting_claim(
                        store.as_ref(),
                        &session_id,
                        self,
                        &claim,
                        DurableTailDriveRole::Writer,
                    ),
                )
                .await
                .map(|(outcome, _)| outcome)
            }
            Some(internal) => {
                let _driver = internal.driver.lock().await;
                crate::recovery::recover_durable_tail(store.as_ref(), &session_id).await
            }
        }
        .map_err(PredecessorSettlementRefusal::Tail)?;
        let second_drive = lease.internal.is_some();
        match outcome {
            DurableTailRecoveryOutcome::Committed {
                disposition,
                boundary_sequence,
                ..
            } => {
                let run_id = tail_run.ok_or_else(|| {
                    PredecessorSettlementRefusal::Runtime(RuntimeDriverError::Internal(format!(
                        "session {session_id} committed a durable tail no provisional run named"
                    )))
                })?;
                lease.tail_held = false;
                Ok(Some(PredecessorTailCommit {
                    run_id,
                    disposition,
                    boundary_sequence,
                }))
            }
            DurableTailRecoveryOutcome::AlreadyAligned { .. } => {
                lease.tail_held = false;
                Ok(None)
            }
            DurableTailRecoveryOutcome::Held if !second_drive => {
                lease.tail_held = true;
                Ok(None)
            }
            DurableTailRecoveryOutcome::Held => Err(PredecessorSettlementRefusal::TailHeld),
            DurableTailRecoveryOutcome::Refused => Err(PredecessorSettlementRefusal::TailRefused),
            // A proven writer never continues a run: the proof lapsed.
            DurableTailRecoveryOutcome::Continue { .. } if !second_drive => {
                Err(PredecessorSettlementRefusal::ExclusivityUnproven)
            }
            // The run's inputs are terminal after the sweep, so ordinary
            // recovery classifies the tail as interrupted; a continuation
            // here leaves the tail as it is.
            DurableTailRecoveryOutcome::Continue { .. } => {
                Err(PredecessorSettlementRefusal::TailHeld)
            }
        }
    }

    /// Publish the settlement's internal, non-serving registration, take its
    /// live lifecycle lease (L) and mutation gate (M), and in one commit end
    /// every non-terminal input `Abandoned { Stopped }` with its runless
    /// terminal and answer every input a recovered boundary consumed without
    /// a receipt with its outcome-unknown batch.
    ///
    /// While directed work still needs a new outbox, the registration's
    /// converged lifecycle row keeps the placement witness until the sweep
    /// commit, which writes the row without it. A cold-recovered unregister
    /// drain the predecessor left is concluded first, unmarked, as every
    /// registration concludes it: T is released for it and taken again, and
    /// a registration that took T meanwhile refuses the call `Registered`.
    /// Consent owner loss completes at the publication. A failed sweep
    /// commit rolls the prepared terminals back and marks an ended resumable
    /// run resumable again.
    pub async fn end_predecessor_work_with_lease_before(
        &self,
        lease: &mut MachinePredecessorSettlementLease,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<PredecessorWork, PredecessorSettlementRefusal> {
        let store = self.require_settlement_store(lease)?;
        if lease.internal.is_some() {
            return Err(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::Internal(format!(
                    "the predecessor settlement of session {} already published its registration",
                    lease.session_id
                )),
            ));
        }
        let session_id = lease.session_id.clone();
        let runtime_id = lease.runtime_id.clone();
        let mut concluded_drain = false;
        // The proof this call's own first publication completed, when that
        // publication came back as a cold-recovered drain.
        let mut completed_by_this_call = None;
        let published = loop {
            // Directed work that still needs a new outbox keeps the placement
            // witness in the converged lifecycle row until the sweep commit.
            let selection = read_sweep_selection(store.as_ref(), &runtime_id).await?;
            let witness = if selection.directed_needing_outbox.is_empty() {
                None
            } else {
                Some(lease.placement.as_ref().ok_or_else(|| {
                    PredecessorSettlementRefusal::PlacementWitnessMissing {
                        directed: selection.directed_needing_outbox.clone(),
                    }
                })?)
            };
            let prepared = self
                .prepare_predecessor_settlement_registration(session_id.clone(), witness)
                .await
                .map_err(PredecessorSettlementRefusal::Runtime)?;
            let published = self
                .commit_prepared_predecessor_settlement_registration_under_transaction(prepared)
                .await
                .map_err(|refused| match refused {
                    SettlementRegistrationRefused::Registered => {
                        PredecessorSettlementRefusal::Registered
                    }
                    SettlementRegistrationRefused::Runtime(error) => {
                        PredecessorSettlementRefusal::Runtime(error)
                    }
                })?;
            if !published.cold_recovered_draining {
                break published;
            }
            completed_by_this_call = published.completed_owner_loss.clone();
            if concluded_drain {
                return Err(PredecessorSettlementRefusal::Runtime(
                    RuntimeDriverError::Internal(format!(
                        "session {session_id} re-entered a cold-recovered unregister drain right \
                         after its teardown was concluded"
                    )),
                ));
            }
            concluded_drain = true;
            // The predecessor's own unregister concludes first, unmarked.
            drop(lease.registration_guard.take());
            self.conclude_cold_recovered_unregister_drain_before(
                &session_id,
                &published.epoch_id,
                deadline,
            )
            .await
            .map_err(PredecessorSettlementRefusal::Runtime)?;
            lease.registration_guard = Some(
                self.lock_registration_transaction_before(&session_id, deadline)
                    .await?,
            );
            self.refuse_registered_under_transaction(&session_id)
                .await?;
        };
        let owner_loss =
            predecessor_owner_loss(&published, &lease.claim, completed_by_this_call.as_ref());
        let mutation_gate = published.mutation_gate.clone();
        // From here a dropped lease hands the registration's removal off,
        // and every refusal reports the owner-loss outcome.
        lease.internal = Some(published);
        lease.owner_loss = owner_loss.as_ref().ok().cloned();
        let owner_loss = owner_loss?;

        #[cfg(feature = "live")]
        {
            lease.live_lifecycle_lease = Some(
                self.acquire_member_live_disposal_lease(&session_id)
                    .await
                    .map_err(|error| {
                        PredecessorSettlementRefusal::Runtime(RuntimeDriverError::Internal(
                            format!(
                                "the settlement registration of session {session_id} could not \
                                 take its live lifecycle lease: {error}"
                            ),
                        ))
                    })?,
            );
        }
        // The internal registration's marker refuses the ordinary gate
        // helper: the settlement takes its own gate directly.
        lease.mutation_guard = Some(
            mutation_gate
                .lock_settled(SettledGateWait::UnattachedSettlement)
                .await,
        );
        self.require_current_settlement_registration(lease).await?;
        // A fresh read under M selects the inputs a recovered boundary left
        // without a receipt: the begin scan's, and the contributors the tail
        // drive committed without a batch.
        let selection = read_sweep_selection(store.as_ref(), &runtime_id).await?;
        if lease.placement.is_none() && !selection.directed_needing_outbox.is_empty() {
            return Err(PredecessorSettlementRefusal::PlacementWitnessMissing {
                directed: selection.directed_needing_outbox,
            });
        }
        let (ended, answered) = self
            .sweep_predecessor_work(lease, store.as_ref(), selection.outcome_unknown)
            .await?;
        Ok(PredecessorWork {
            owner_loss,
            ended,
            answered,
        })
    }

    /// Release the settlement's gates and hand the rest to one process-owned
    /// task: the runtime loop's startup terminal drain with `publication`,
    /// holding no runtime gate, then T and M for the closing census of every
    /// durable input row and the exact removal of the internal registration.
    /// Awaits the task's result until `deadline`; past it, the task drains
    /// and removes on its own, and the call answers
    /// `RuntimeTerminalPublicationInProgress` with the evidence final at that
    /// moment.
    pub async fn finish_predecessor_settlement_before(
        &self,
        lease: MachinePredecessorSettlementLease,
        publication: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<PredecessorPublication, PredecessorPublicationRefused> {
        self.hand_off_predecessor_settlement_before(lease, Some(publication), deadline)
            .await
    }

    /// Remove the settlement's internal registration exactly, for a
    /// settlement that stops before its drain: the gates are released and the
    /// same process-owned task takes T and M and removes it, publishing
    /// nothing and running no census. `Ok` once it is removed, or when the
    /// lease holds no registration; past `deadline`,
    /// `RuntimeTerminalPublicationInProgress` while the task removes it on its
    /// own.
    pub async fn remove_predecessor_settlement_registration_before(
        &self,
        lease: MachinePredecessorSettlementLease,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), PredecessorSettlementRefusal> {
        if !lease.registered() {
            return Ok(());
        }
        self.hand_off_predecessor_settlement_before(lease, None, deadline)
            .await
            .map(|_| ())
            .map_err(|refused| refused.refusal)
    }

    /// [`Self::finish_predecessor_settlement_before`], and with no
    /// `publication` [`Self::remove_predecessor_settlement_registration_before`].
    async fn hand_off_predecessor_settlement_before(
        &self,
        mut lease: MachinePredecessorSettlementLease,
        publication: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<PredecessorPublication, PredecessorPublicationRefused> {
        let refused = |refusal| PredecessorPublicationRefused {
            refusal,
            progress: PredecessorPublication::default(),
        };
        let store = self.require_settlement_store(&lease).map_err(refused)?;
        let Some(internal) = lease.internal.take() else {
            return Err(refused(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::Internal(format!(
                    "the predecessor settlement of session {} has no registration to finish",
                    lease.session_id
                )),
            )));
        };
        let session_id = lease.session_id.clone();
        let runtime_id = lease.runtime_id.clone();
        let census = std::mem::take(&mut lease.census);
        // M, L, then T: the task takes T, then M, after the drain.
        drop(lease.mutation_guard.take());
        #[allow(
            clippy::drop_non_drop,
            reason = "with the live feature the lease holds L's guard and this drop releases it in \
                      order; without it the lease is uninhabited and the drop does nothing"
        )]
        drop(lease.live_lifecycle_lease.take());
        drop(lease.registration_guard.take());
        lease.runless_terminal.release();
        // A removal publishes nothing and reports no progress.
        let census_for_progress = publication.is_some().then(|| census.clone());
        let (result_tx, result_rx) = crate::tokio::sync::oneshot::channel();
        if let Err(error) = self.start_predecessor_settlement_task(
            session_id.clone(),
            internal,
            publication,
            census,
            Some(result_tx),
        ) {
            return Err(PredecessorPublicationRefused {
                refusal: PredecessorSettlementRefusal::Runtime(error),
                progress: finish_progress(
                    store.as_ref(),
                    &runtime_id,
                    census_for_progress.as_ref(),
                )
                .await,
            });
        }
        drop(lease);
        let in_progress = || {
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress {
                    runtime_id: runtime_id.clone(),
                },
            )
        };
        let Some(remaining) = remaining_until(deadline) else {
            return Err(PredecessorPublicationRefused {
                refusal: in_progress(),
                progress: finish_progress(
                    store.as_ref(),
                    &runtime_id,
                    census_for_progress.as_ref(),
                )
                .await,
            });
        };
        match crate::tokio::time::timeout(remaining, result_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(PredecessorPublicationRefused {
                refusal: PredecessorSettlementRefusal::Runtime(RuntimeDriverError::Internal(
                    format!(
                        "the predecessor settlement task of session {session_id} ended without a \
                         result"
                    ),
                )),
                progress: finish_progress(
                    store.as_ref(),
                    &runtime_id,
                    census_for_progress.as_ref(),
                )
                .await,
            }),
            Err(_) => Err(PredecessorPublicationRefused {
                refusal: in_progress(),
                progress: finish_progress(
                    store.as_ref(),
                    &runtime_id,
                    census_for_progress.as_ref(),
                )
                .await,
            }),
        }
    }

    /// Whether the registration `session_id` has here holds a claim of the
    /// same live lineage as `claim`, the lineage a settlement settled under.
    /// `None` when the session has no registration here. An observation for
    /// a host's continuity check; it grants nothing.
    pub async fn registration_claim_same_lineage(
        &self,
        session_id: &SessionId,
        claim: &HostingClaim,
    ) -> Option<bool> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|entry| entry._hosting_claim.same_lineage(claim))
    }

    #[allow(
        clippy::result_large_err,
        reason = "the public refusal carries a runtime or recovery error inline"
    )]
    fn require_settlement_store(
        &self,
        lease: &MachinePredecessorSettlementLease,
    ) -> Result<Arc<dyn crate::store::RuntimeStore>, PredecessorSettlementRefusal> {
        if !Arc::ptr_eq(&lease.machine.shared, &self.shared) {
            return Err(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "the predecessor settlement lease of session {} belongs to another machine",
                        lease.session_id
                    ),
                },
            ));
        }
        self.store.as_ref().map(Arc::clone).ok_or_else(|| {
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::ValidationFailed {
                reason: format!("session {} has no durable runtime store", lease.session_id),
            })
        })
    }

    /// The claim checks of a settlement: this machine's own hosting owner,
    /// a cross-process OS lock, and this machine's hosting domain.
    #[allow(
        clippy::result_large_err,
        reason = "the public refusal carries a runtime or recovery error inline"
    )]
    fn check_predecessor_settlement_claim(
        &self,
        claim: &HostingClaim,
    ) -> Result<(), PredecessorSettlementRefusal> {
        let authority = self.hosting_authority();
        if claim.owner() != authority.owner() {
            return Err(PredecessorSettlementRefusal::ClaimNotOwned);
        }
        if !claim.is_cross_process() {
            return Err(PredecessorSettlementRefusal::ExclusivityUnproven);
        }
        if claim.domain_ref() != authority.capability().domain_ref() {
            return Err(PredecessorSettlementRefusal::ClaimNotOwned);
        }
        Ok(())
    }

    async fn lock_registration_transaction_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<crate::tokio::sync::OwnedMutexGuard<()>, PredecessorSettlementRefusal> {
        let held_past_deadline = || {
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::RecoveryBackoff {
                reason: format!(
                    "the registration transaction of session {session_id} stayed held past the \
                     settlement's deadline"
                ),
            })
        };
        let remaining = remaining_until(deadline).ok_or_else(held_past_deadline)?;
        crate::tokio::time::timeout(
            remaining,
            self.lock_session_registration_transaction(session_id),
        )
        .await
        .map_err(|_| held_past_deadline())
    }

    /// Refuse a registration the session has here: `Registered`, or the
    /// typed refusal of a registration leaving through its unregister (a
    /// settlement's own registration a task still removes included).
    async fn refuse_registered_under_transaction(
        &self,
        session_id: &SessionId,
    ) -> Result<(), PredecessorSettlementRefusal> {
        let sessions = self.sessions.read().await;
        let Some(entry) = sessions.get(session_id) else {
            return Ok(());
        };
        Err(match entry.registration_blocked_by_unregister(session_id) {
            Some(RuntimeDriverError::NotReady { .. }) => {
                PredecessorSettlementRefusal::Runtime(RuntimeDriverError::UnregisterInProgress {
                    runtime_id: entry.runtime_id.clone(),
                })
            }
            Some(error) => PredecessorSettlementRefusal::Runtime(error),
            None => PredecessorSettlementRefusal::Registered,
        })
    }

    /// Prove the session's current registration is still the settlement's
    /// own, unadopted and marked, under the gate the lease holds.
    async fn require_current_settlement_registration(
        &self,
        lease: &MachinePredecessorSettlementLease,
    ) -> Result<(), PredecessorSettlementRefusal> {
        let Some(internal) = lease.internal.as_ref() else {
            return Err(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::Internal(format!(
                    "the predecessor settlement of session {} has no registration",
                    lease.session_id
                )),
            ));
        };
        let sessions = self.sessions.read().await;
        let current = sessions.get(&lease.session_id).is_some_and(|entry| {
            Arc::ptr_eq(&entry.driver, &internal.driver)
                && entry.epoch_id == internal.epoch_id
                && internal.registration.matches_entry(entry)
                && entry.registration_use() == internal.registration_use
                && entry.is_predecessor_settlement()
        });
        if current {
            Ok(())
        } else {
            Err(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "the settlement registration of session {} is no longer current",
                        lease.session_id
                    ),
                },
            ))
        }
    }

    /// The sweep, in one commit: every non-terminal input of the internal
    /// registration ends `Abandoned { Stopped }` with its runless terminal
    /// and, for a directed one, its outbox (the retired-pending sweep with
    /// the reason `Stopped`), and every input of `outcome_unknown` gets its
    /// run's outcome-unknown batch (runless, at most one batch size each) and,
    /// for a directed one, its outbox. Every outbox takes the lease's
    /// placement while the registration is unplaced; the lifecycle row is
    /// written from the registration's own authority, without a placement
    /// witness. Terminals a store-owned recovery wrote outside the
    /// registration are adopted first, the `outcome_unknown` rows enter the
    /// registration's authority as a registration loads a pending-terminal
    /// row, and a resumable run's mark is ended first without touching its
    /// inputs. Returns the ended and the answered inputs.
    async fn sweep_predecessor_work(
        &self,
        lease: &mut MachinePredecessorSettlementLease,
        store: &dyn crate::store::RuntimeStore,
        outcome_unknown: Vec<(StoredInputState, RunId)>,
    ) -> Result<(Vec<InputId>, Vec<InputId>), PredecessorSettlementRefusal> {
        let Some(internal) = lease.internal.as_ref() else {
            return Err(PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::Internal(format!(
                    "the predecessor settlement of session {} has no registration to sweep",
                    lease.session_id
                )),
            ));
        };
        let mut driver = internal.driver.lock().await;
        driver
            .adopt_durable_consumed_inputs(Some(&internal.completions))
            .await
            .map_err(PredecessorSettlementRefusal::Runtime)?;
        // Each run's answered inputs, in the order the runs first appear.
        let mut answered: Vec<(RunId, Vec<InputId>)> = Vec::new();
        let mut rows = Vec::with_capacity(outcome_unknown.len());
        for (stored, run_id) in outcome_unknown {
            let input_id = stored.state.input_id.clone();
            match answered.iter_mut().find(|(run, _)| run == &run_id) {
                Some((_, input_ids)) => input_ids.push(input_id),
                None => answered.push((run_id, vec![input_id])),
            }
            rows.push(stored);
        }
        super::driver::machine_recover_settlement_terminal_rows(
            store,
            &internal.runtime_id,
            &mut driver,
            rows,
        )
        .await
        .map_err(PredecessorSettlementRefusal::Runtime)?;
        let answered_ids = answered
            .iter()
            .flat_map(|(_, input_ids)| input_ids.iter().cloned())
            .collect::<Vec<_>>();
        let ended_run = match driver.resumable_run_id() {
            Some(run_id) => {
                driver
                    .end_resumable_run(&run_id, &InputAbandonReason::Stopped)
                    .map_err(PredecessorSettlementRefusal::Runtime)?;
                Some(run_id)
            }
            None => None,
        };
        let ended = driver.as_driver().active_input_ids();
        let prepared = match driver.prepare_predecessor_settlement_terminals(
            &ended,
            PREDECESSOR_SETTLEMENT_TERMINATION_REASON.to_string(),
            &answered,
            lease.placement.as_ref(),
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                if let Some(run_id) = &ended_run {
                    driver.restore_ended_resumable_run(run_id);
                }
                return Err(PredecessorSettlementRefusal::Runtime(error));
            }
        };
        match driver
            .abandon_pending_inputs_with_terminal_rows(InputAbandonReason::Stopped, &answered_ids)
            .await
        {
            Ok(abandoned) => {
                if abandoned > 0 || !answered_ids.is_empty() {
                    lease.runless_terminal.owe();
                }
            }
            Err(error) => {
                driver.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                if let Some(run_id) = &ended_run {
                    driver.restore_ended_resumable_run(run_id);
                }
                // The commit may have landed before a later step failed: a
                // conservative re-observation is owed.
                lease.runless_terminal.owe();
                return Err(PredecessorSettlementRefusal::Runtime(
                    RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: format!(
                            "the predecessor settlement's sweep of session {} did not commit: \
                             {error}",
                            lease.session_id
                        ),
                    },
                ));
            }
        }
        let _batches =
            super::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(
                prepared,
            );
        Ok((ended, answered_ids))
    }

    /// Wait, holding no gate and bounded by `deadline`, for a settlement task
    /// of `session_id` an earlier call left running.
    async fn await_predecessor_settlement_task_before(
        &self,
        session_id: &SessionId,
        runtime_id: &LogicalRuntimeId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), PredecessorSettlementRefusal> {
        let in_progress = || {
            PredecessorSettlementRefusal::Runtime(
                RuntimeDriverError::RuntimeTerminalPublicationInProgress {
                    runtime_id: runtime_id.clone(),
                },
            )
        };
        loop {
            let pending = self
                .pending_predecessor_settlements
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session_id)
                .map(|pending| (pending.task_id, pending.done_rx.clone()));
            let Some((task_id, mut done_rx)) = pending else {
                return Ok(());
            };
            let remaining = remaining_until(deadline).ok_or_else(in_progress)?;
            let finished = crate::tokio::time::timeout(remaining, async {
                loop {
                    if *done_rx.borrow() {
                        break;
                    }
                    if done_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await;
            if finished.is_err() {
                return Err(in_progress());
            }
            // A task that ended without freeing its slot is done as well:
            // its slot goes, so no later begin waits on it again.
            let mut pending = self
                .pending_predecessor_settlements
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending
                .get(session_id)
                .is_some_and(|pending| pending.task_id == task_id)
            {
                pending.remove(session_id);
            }
        }
    }

    /// Start the settlement's process-owned task in the session's
    /// single-flight slot: the drain with `publication`, when given, then T
    /// and M for the closing census and the exact removal. `result_tx`, when
    /// given, receives the task's result; the task runs to its end either
    /// way.
    fn start_predecessor_settlement_task(
        &self,
        session_id: SessionId,
        internal: PublishedSettlementRegistration,
        publication: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        census: BeginCensus,
        result_tx: Option<
            crate::tokio::sync::oneshot::Sender<
                Result<PredecessorPublication, PredecessorPublicationRefused>,
            >,
        >,
    ) -> Result<(), RuntimeDriverError> {
        let spawner = MachineCleanupTaskSpawner::acquire()?;
        let task_id = uuid::Uuid::new_v4();
        let (done_tx, done_rx) = crate::tokio::sync::watch::channel(false);
        self.pending_predecessor_settlements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                session_id.clone(),
                PendingPredecessorSettlement { task_id, done_rx },
            );
        // Frees the slot and tells its waiters when the task ends: by return,
        // by panic, or with its future dropped unpolled.
        let slot = PredecessorSettlementSlot {
            machine: self.clone(),
            session_id: session_id.clone(),
            task_id,
            done_tx,
        };
        let machine = self.clone();
        spawner.spawn(async move {
            let result = machine
                .run_predecessor_settlement_task(&session_id, internal, publication, &census)
                .await;
            drop(slot);
            if let Some(result_tx) = result_tx {
                let _ = result_tx.send(result);
            }
        });
        Ok(())
    }

    async fn run_predecessor_settlement_task(
        &self,
        session_id: &SessionId,
        internal: PublishedSettlementRegistration,
        publication: Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        census: &BeginCensus,
    ) -> Result<PredecessorPublication, PredecessorPublicationRefused> {
        // The drain holds no runtime gate: the stored-only publisher takes
        // the session's recovery gate itself, briefly, and nothing that holds
        // that gate waits on this task.
        let drain = match publication.as_ref() {
            Some(publication) => {
                crate::runtime_loop::drain_recovered_terminals_with_stored_only_publisher(
                    &internal.driver,
                    &internal.post_commit_hooks,
                    Some(&internal.completions),
                    publication.as_ref(),
                )
                .await
            }
            None => Ok(()),
        };
        // T, then M with the settled wait: never M while waiting for T.
        let registration_guard = self.lock_session_registration_transaction(session_id).await;
        let mutation_guard = internal
            .mutation_gate
            .lock_settled(SettledGateWait::UnattachedSettlement)
            .await;
        let evidence = match (publication.as_ref(), self.store.as_ref()) {
            (Some(_), Some(store)) => {
                Some(read_settlement_evidence(store.as_ref(), &internal.runtime_id, census).await)
            }
            _ => None,
        };
        let removal = self
            .remove_predecessor_settlement_registration_exact(session_id, &internal)
            .await;
        drop(mutation_guard);
        drop(registration_guard);

        let census_result = match evidence {
            Some(Ok(evidence)) => evidence.into_closing_census(),
            Some(Err(error)) => Err(PredecessorPublicationRefused {
                refusal: PredecessorSettlementRefusal::Runtime(error),
                progress: PredecessorPublication::default(),
            }),
            // Without publication only the removal ran.
            None => Ok(PredecessorPublication::default()),
        };
        if let Err(error) = drain {
            return Err(refused_with_final_evidence(
                PredecessorSettlementRefusal::Runtime(error),
                census_result,
            ));
        }
        if let Err(error) = removal {
            return Err(refused_with_final_evidence(
                PredecessorSettlementRefusal::Runtime(error),
                census_result,
            ));
        }
        census_result
    }

    /// Remove a settlement's internal registration exactly. The caller holds
    /// T and the registration's mutation gate. It removes only the same
    /// runtime and driver, under the same registration witness and use (no
    /// one adopted it), still marked, never attached (no attachment, loop
    /// teardown, wake sender or publication handle, no unregister or stop
    /// coordinator, no pending finalization, its materialization claim
    /// vacant), with no other publication task on its driver. It writes
    /// nothing, so it equals a process exit at this point; an absent
    /// registration is converged.
    async fn remove_predecessor_settlement_registration_exact(
        &self,
        session_id: &SessionId,
        internal: &PublishedSettlementRegistration,
    ) -> Result<(), RuntimeDriverError> {
        let not_exact = |what: &str| {
            RuntimeDriverError::Internal(format!(
                "the settlement registration of session {session_id} cannot be removed: {what}"
            ))
        };
        let removed = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(());
            };
            if entry.runtime_id != internal.runtime_id
                || !Arc::ptr_eq(&entry.driver, &internal.driver)
            {
                return Err(not_exact("it was replaced"));
            }
            if entry.epoch_id != internal.epoch_id || !internal.registration.matches_entry(entry) {
                return Err(not_exact("its registration witness changed"));
            }
            if entry.registration_use() != internal.registration_use {
                return Err(not_exact("it was adopted"));
            }
            if !entry.is_predecessor_settlement() {
                return Err(not_exact("it lost its settlement marker"));
            }
            let claim_vacant = {
                let claim = entry
                    .materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                claim.current.is_none()
                    && matches!(
                        claim.phase,
                        crate::RuntimeActorMaterializationClaimPhase::Vacant
                    )
            };
            if !matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty)
                || entry.runtime_loop_teardown.is_some()
                || entry.wake_sender().is_some()
                || entry.publication_handle().is_some()
                || entry.unregister_coordinator.is_some()
                || entry.runtime_stop_cleanup_coordinator.is_some()
                || entry.reload_required_discard_coordinator.is_some()
                || entry.pending_unregister_finalization.is_some()
                || !claim_vacant
            {
                return Err(not_exact("it was attached"));
            }
            if self
                .existing_runless_terminal_publication_dispatch(&entry.driver)
                .is_some()
            {
                return Err(not_exact("a publication task still runs for its driver"));
            }
            sessions.remove(session_id)
        };
        if let Some(entry) = removed.as_ref() {
            entry.post_commit_hooks.shutdown();
            entry.notify_registration_removed();
            // The session is no longer served here.
            self.tool_grant_deadlines.disarm(session_id);
        }
        drop(removed);
        Ok(())
    }
}

/// The settlement's final evidence at this moment, read without any gate:
/// the inputs it settled that converged and the terminal ids still owed.
/// Inputs it did not reach are not reported.
async fn settlement_progress(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    census: &BeginCensus,
) -> PredecessorPublication {
    match read_settlement_evidence(store, runtime_id, census).await {
        Ok(evidence) => PredecessorPublication {
            converged: evidence.converged,
            owed: evidence.owed,
            durable_rows_closed: 0,
            migrated_without_receipt: Vec::new(),
        },
        Err(_) => PredecessorPublication::default(),
    }
}

/// The final evidence a refused finish reports: that of `census`, or none
/// for a removal, which publishes nothing.
async fn finish_progress(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    census: Option<&BeginCensus>,
) -> PredecessorPublication {
    match census {
        Some(census) => settlement_progress(store, runtime_id, census).await,
        None => PredecessorPublication::default(),
    }
}

/// `refusal`, with the final evidence of `census`: its converged inputs and
/// owed ids, never a census count.
fn refused_with_final_evidence(
    refusal: PredecessorSettlementRefusal,
    census: Result<PredecessorPublication, PredecessorPublicationRefused>,
) -> PredecessorPublicationRefused {
    let progress = match census {
        Ok(publication) => PredecessorPublication {
            durable_rows_closed: 0,
            migrated_without_receipt: Vec::new(),
            ..publication
        },
        Err(refused) => refused.progress,
    };
    PredecessorPublicationRefused { refusal, progress }
}

/// The consent owner-loss outcome of the internal registration's
/// publication. `completed_by_this_call` is the proof an earlier publication
/// of this same call completed (the unmarked publication of a cold-recovered
/// drain it concluded first): this call completed it, so it is reported
/// `Completed`, not `NotOwed`.
#[allow(
    clippy::result_large_err,
    reason = "the public refusal carries a runtime or recovery error inline"
)]
fn predecessor_owner_loss(
    published: &PublishedSettlementRegistration,
    claim: &HostingClaim,
    completed_by_this_call: Option<&HostingOwnerLossProof>,
) -> Result<PredecessorOwnerLoss, PredecessorSettlementRefusal> {
    if let Some(proof) = published
        .completed_owner_loss
        .clone()
        .or_else(|| completed_by_this_call.cloned())
    {
        return Ok(PredecessorOwnerLoss::Completed(proof));
    }
    if !published.consent_owner_installed {
        return Ok(PredecessorOwnerLoss::NoConsentOwner);
    }
    claim
        .completed_owner_loss()
        .map(PredecessorOwnerLoss::NotOwed)
        .ok_or_else(|| {
            PredecessorSettlementRefusal::Runtime(RuntimeDriverError::ConsentOwnerLossUnsettled {
                session_id: claim.session_id().clone(),
            })
        })
}

#[cfg(all(test, feature = "sqlite-store"))]
#[path = "predecessor_settlement_tests.rs"]
mod tests;
