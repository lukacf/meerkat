//! RuntimeStore — atomic persistence for runtime state.
//!
//! Machine-owned runtime commands durably persist [`RunBoundaryReceipt`] values
//! atomically with their session and input-state effects.

pub mod memory;
#[cfg(feature = "sqlite-store")]
pub mod sqlite;

use std::collections::{HashMap, HashSet};

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::runtime_state::RuntimeState;

const LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 1;
const SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 2;
const MACHINE_LIFECYCLE_STORE_RECORD_VERSION: u16 = 3;

/// Maximum number of exact input-state rows admitted by one compare-and-swap
/// boundary. Directed-terminal outbox batches share the same 256-row bound as
/// their publication seam.
pub const MAX_INPUT_STATE_BATCH_CAS: usize = 256;

/// Result of an exact input-state batch compare-and-swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStateBatchCasOutcome {
    /// Every expected durable row matched and every replacement committed, or
    /// every durable row was already byte-identical to its replacement from an
    /// earlier invocation whose acknowledgement was lost.
    Swapped,
    /// At least one expected row was missing or no longer byte-identical; no
    /// replacement was written.
    Stale,
}

#[derive(Debug)]
struct PreparedInputStateBatchCasRow {
    input_id: InputId,
    expected_json: Vec<u8>,
    replacement: StoredInputState,
    // SQLite writes the already-validated bytes; the in-memory implementation
    // stores the typed replacement directly.
    #[cfg_attr(not(feature = "sqlite-store"), allow(dead_code))]
    replacement_json: Vec<u8>,
}

fn prepare_input_state_batch_cas(
    expected: &[StoredInputState],
    replacements: &[InputStatePersistenceRecord],
) -> Result<Vec<PreparedInputStateBatchCasRow>, RuntimeStoreError> {
    if expected.len() != replacements.len() {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "expected row count {} does not match replacement row count {}",
                expected.len(),
                replacements.len()
            ),
        });
    }
    if expected.len() > MAX_INPUT_STATE_BATCH_CAS {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!(
                "batch contains {} rows, exceeding the maximum of {MAX_INPUT_STATE_BATCH_CAS}",
                expected.len()
            ),
        });
    }

    let mut expected_ids = HashSet::with_capacity(expected.len());
    for row in expected {
        if !expected_ids.insert(row.state.input_id.clone()) {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("expected batch repeats input {}", row.state.input_id),
            });
        }
    }

    let mut replacement_by_id = HashMap::with_capacity(replacements.len());
    for record in replacements {
        let replacement = record.clone_stored();
        let input_id = replacement.state.input_id.clone();
        let replacement_json = serde_json::to_vec(&replacement)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        if replacement_by_id
            .insert(input_id.clone(), (replacement, replacement_json))
            .is_some()
        {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("replacement batch repeats input {input_id}"),
            });
        }
    }

    let mut prepared = Vec::with_capacity(expected.len());
    for expected_row in expected {
        let input_id = expected_row.state.input_id.clone();
        let Some((replacement, replacement_json)) = replacement_by_id.remove(&input_id) else {
            return Err(RuntimeStoreError::InvalidInputStateBatchCas {
                reason: format!("replacement batch does not contain expected input {input_id}"),
            });
        };
        let expected_json = serde_json::to_vec(expected_row)
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        prepared.push(PreparedInputStateBatchCasRow {
            input_id,
            expected_json,
            replacement,
            replacement_json,
        });
    }
    if let Some(extra) = replacement_by_id.keys().next() {
        return Err(RuntimeStoreError::InvalidInputStateBatchCas {
            reason: format!("replacement batch contains unexpected input {extra}"),
        });
    }
    Ok(prepared)
}

/// Errors from RuntimeStore operations.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeStoreError {
    /// Write failed.
    #[error("Store write failed: {0}")]
    WriteFailed(String),
    /// Read failed.
    #[error("Store read failed: {0}")]
    ReadFailed(String),
    /// The explicit session-store key does not match the serialized session.
    #[error("Session store key mismatch: expected {expected}, actual {actual}")]
    SessionKeyMismatch {
        expected: meerkat_core::types::SessionId,
        actual: meerkat_core::types::SessionId,
    },
    /// Not found.
    #[error("Not found: {0}")]
    NotFound(String),
    /// Operation is not supported by this store implementation.
    #[error("Unsupported store operation: {0}")]
    Unsupported(String),
    /// A detached producer attempted to persist an ops snapshot after the
    /// matching epoch was atomically retired by unregister.
    #[error("Ops lifecycle epoch {epoch_id} for runtime {runtime_id} is retired")]
    OpsLifecycleEpochRetired {
        runtime_id: String,
        epoch_id: meerkat_core::RuntimeEpochId,
    },
    /// An unregister-finalization commit may have become durable, but the
    /// backend could not authoritatively classify its outcome.
    ///
    /// Callers must retry the idempotent atomic finalization and must not
    /// publish a compensating lifecycle rollback for this error.
    #[error("Unregister finalization outcome is unknown: {0}")]
    UnregisterFinalizationOutcomeUnknown(String),
    /// Runtime snapshot CAS rejected a stale transcript rewrite.
    #[error("Transcript revision conflict: expected {expected}, actual {actual}")]
    TranscriptRevisionConflict { expected: String, actual: String },
    /// An atomic boundary commit carried a session snapshot that was already
    /// superseded by the durable append-only head. Callers must observe this
    /// as a failed commit rather than mistaking a no-op for publication.
    #[error("Session snapshot for runtime '{runtime_id}' was superseded by the durable head")]
    SessionSnapshotSuperseded { runtime_id: String },
    /// The requested exact input-state batch CAS has an invalid row/key shape.
    #[error("Invalid input-state batch compare-and-swap: {reason}")]
    InvalidInputStateBatchCas { reason: String },
    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Transactional updater for the runtime-owned OAuth login-flow payload snapshot.
pub type AuthOAuthFlowSnapshotUpdate<'a> =
    dyn FnMut(Option<&[u8]>) -> Result<Vec<u8>, RuntimeStoreError> + 'a;

/// Describes a serialized session snapshot for boundary and snapshot-only commits.
#[derive(Debug, Clone)]
pub struct SessionDelta {
    /// Serialized session snapshot (opaque to RuntimeStore).
    pub session_snapshot: Vec<u8>,
}

fn validated_compaction_projection_intents(
    session: &meerkat_core::Session,
) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
    session
        .validated_compaction_projection_intents()
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))
}

/// Runtime binding facts selected by generated MeerkatMachine authority.
///
/// RuntimeStore implementations persist and read these facts as part of a
/// machine lifecycle snapshot. The commit token that writes these facts stays
/// crate-private so compatibility callers cannot mint replacement lifecycle
/// truth.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MachineLifecycleBindingFacts {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

/// Durable identity receipt for the last completed supervisor revoke.
///
/// This is not a live supervisor binding and carries no route address. It is
/// only the identity/key/epoch witness needed to authorize an exact duplicate
/// revoke response after a cold restart; the current authenticated request
/// supplies its current route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedSupervisorReceipt {
    peer_id: String,
    signing_public_key: String,
    epoch: u64,
}

/// Durable current supervisor binding used to authenticate terminal retry
/// traffic after a cold runtime restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorBindingReceipt {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

/// Durable in-flight supervisor revocation receipt.
///
/// This is the closed-world hand-off between generated machine authority and
/// the concrete router mutation.  It deliberately retains the complete prior
/// route so a cold runtime can authenticate an exact retry and re-materialize
/// the generated remove obligation without resurrecting a live binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRevocationPendingReceipt {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRotationPersistencePhase {
    PreviousRevokePending,
    NextPublishPending,
    Completed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupervisorRotationRejection {
    OperationConflict,
    NotBound,
    SenderMismatch,
    TargetEpochNotAdvanced,
    InvalidTarget,
    UnsupportedProtocolVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRotationReceipt {
    operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
    phase: SupervisorRotationPersistencePhase,
    rejection: Option<SupervisorRotationRejection>,
    previous: SupervisorBindingReceipt,
    next: SupervisorBindingReceipt,
}

impl SupervisorRotationReceipt {
    pub(crate) fn new(
        operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        phase: SupervisorRotationPersistencePhase,
        rejection: Option<SupervisorRotationRejection>,
        previous: SupervisorBindingReceipt,
        next: SupervisorBindingReceipt,
    ) -> Self {
        Self {
            operation_id,
            phase,
            rejection,
            previous,
            next,
        }
    }

    pub fn operation_id(
        &self,
    ) -> meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId {
        self.operation_id
    }

    pub fn phase(&self) -> SupervisorRotationPersistencePhase {
        self.phase
    }

    pub fn rejection(&self) -> Option<SupervisorRotationRejection> {
        self.rejection
    }

    pub fn previous(&self) -> &SupervisorBindingReceipt {
        &self.previous
    }

    pub fn next(&self) -> &SupervisorBindingReceipt {
        &self.next
    }
}

impl SupervisorBindingReceipt {
    pub(crate) fn new(
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Self {
        Self {
            name,
            peer_id,
            address,
            signing_public_key,
            epoch,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl RevokedSupervisorReceipt {
    pub(crate) fn new(peer_id: String, signing_public_key: String, epoch: u64) -> Self {
        Self {
            peer_id,
            signing_public_key,
            epoch,
        }
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

impl SupervisorRevocationPendingReceipt {
    pub(crate) fn new(
        name: String,
        peer_id: String,
        address: String,
        signing_public_key: String,
        epoch: u64,
    ) -> Self {
        Self {
            name,
            peer_id,
            address,
            signing_public_key,
            epoch,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn signing_public_key(&self) -> &str {
        &self.signing_public_key
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// Closed durable supervisor authority state. Each variant owns one complete
/// recovery shape; terminal rotation receipts retain their exact operation and
/// participant descriptors for idempotent submission and later observation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SupervisorAuthoritySnapshot {
    #[default]
    UnboundNoReceipt,
    Bound(SupervisorBindingReceipt),
    RevocationPending(SupervisorRevocationPendingReceipt),
    RotationOperation(SupervisorRotationReceipt),
    RevokedReceipt(RevokedSupervisorReceipt),
    WithRotationHistory {
        current: Box<SupervisorAuthoritySnapshot>,
        terminal_receipts: std::collections::BTreeMap<
            meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
            SupervisorRotationReceipt,
        >,
    },
}

impl MachineLifecycleBindingFacts {
    pub(crate) fn new(
        agent_runtime_id: Option<String>,
        fence_token: Option<u64>,
        runtime_generation: Option<u64>,
        runtime_epoch_id: Option<String>,
    ) -> Self {
        Self {
            agent_runtime_id,
            fence_token,
            runtime_generation,
            runtime_epoch_id,
        }
    }

    pub fn agent_runtime_id(&self) -> Option<&str> {
        self.agent_runtime_id.as_deref()
    }

    pub fn fence_token(&self) -> Option<u64> {
        self.fence_token
    }

    pub fn runtime_generation(&self) -> Option<u64> {
        self.runtime_generation
    }

    pub fn runtime_epoch_id(&self) -> Option<&str> {
        self.runtime_epoch_id.as_deref()
    }
}

/// Durable read-back shape for machine-owned lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleSnapshot {
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFacts,
    supervisor_authority: SupervisorAuthoritySnapshot,
    unregister_progress: Option<MachineUnregisterProgressSnapshot>,
}

/// Durable generated unregister-saga progress needed to resume an interrupted
/// Draining epoch without reconstructing missing producer outcomes in shell
/// code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineUnregisterProgressSnapshot {
    runtime_loop_drain_pending: bool,
    comms_drain_exit_pending: bool,
    completion_waiter_drain_pending: bool,
    runtime_loop_forced_abort: bool,
    comms_drain_forced_abort: bool,
}

impl MachineUnregisterProgressSnapshot {
    pub(crate) fn new(
        runtime_loop_drain_pending: bool,
        comms_drain_exit_pending: bool,
        completion_waiter_drain_pending: bool,
        runtime_loop_forced_abort: bool,
        comms_drain_forced_abort: bool,
    ) -> Self {
        Self {
            runtime_loop_drain_pending,
            comms_drain_exit_pending,
            completion_waiter_drain_pending,
            runtime_loop_forced_abort,
            comms_drain_forced_abort,
        }
    }

    pub(crate) fn runtime_loop_drain_pending(&self) -> bool {
        self.runtime_loop_drain_pending
    }

    pub(crate) fn comms_drain_exit_pending(&self) -> bool {
        self.comms_drain_exit_pending
    }

    pub(crate) fn completion_waiter_drain_pending(&self) -> bool {
        self.completion_waiter_drain_pending
    }

    pub(crate) fn runtime_loop_forced_abort(&self) -> bool {
        self.runtime_loop_forced_abort
    }

    pub(crate) fn comms_drain_forced_abort(&self) -> bool {
        self.comms_drain_forced_abort
    }
}

impl MachineLifecycleSnapshot {
    pub(crate) fn new(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
    ) -> Self {
        Self::new_with_unregister_progress(runtime_state, binding, supervisor_authority, None)
    }

    pub(crate) fn new_with_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self {
            runtime_state,
            binding,
            supervisor_authority,
            unregister_progress,
        }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.runtime_state
    }

    /// Runtime binding facts selected by the owning MeerkatMachine transition.
    pub fn binding(&self) -> &MachineLifecycleBindingFacts {
        &self.binding
    }

    pub fn supervisor_authority(&self) -> &SupervisorAuthoritySnapshot {
        &self.supervisor_authority
    }

    pub fn unregister_progress(&self) -> Option<&MachineUnregisterProgressSnapshot> {
        self.unregister_progress.as_ref()
    }
}

#[allow(
    clippy::option_option,
    reason = "serde distinguishes missing from explicit null"
)]
fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(deserializer).map(Some)
}

#[allow(
    clippy::option_option,
    reason = "serde distinguishes missing from explicit null"
)]
fn require_present_nullable<T>(
    value: Option<Option<T>>,
    field: &str,
) -> Result<Option<T>, RuntimeStoreError> {
    value.ok_or_else(|| {
        RuntimeStoreError::ReadFailed(format!(
            "machine lifecycle field {field} is required (explicit null is allowed)"
        ))
    })
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleBindingFactsStoreWire {
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    agent_runtime_id: Option<Option<String>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    fence_token: Option<Option<u64>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_generation: Option<Option<u64>>,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    runtime_epoch_id: Option<Option<String>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleBindingFactsStoreWireV1 {
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

impl From<&MachineLifecycleBindingFacts> for MachineLifecycleBindingFactsStoreWire {
    fn from(binding: &MachineLifecycleBindingFacts) -> Self {
        Self {
            agent_runtime_id: Some(binding.agent_runtime_id().map(ToOwned::to_owned)),
            fence_token: Some(binding.fence_token()),
            runtime_generation: Some(binding.runtime_generation()),
            runtime_epoch_id: Some(binding.runtime_epoch_id().map(ToOwned::to_owned)),
        }
    }
}

impl TryFrom<MachineLifecycleBindingFactsStoreWire> for MachineLifecycleBindingFacts {
    type Error = RuntimeStoreError;

    fn try_from(binding: MachineLifecycleBindingFactsStoreWire) -> Result<Self, Self::Error> {
        Ok(Self::new(
            require_present_nullable(binding.agent_runtime_id, "binding.agent_runtime_id")?,
            require_present_nullable(binding.fence_token, "binding.fence_token")?,
            require_present_nullable(binding.runtime_generation, "binding.runtime_generation")?,
            require_present_nullable(binding.runtime_epoch_id, "binding.runtime_epoch_id")?,
        ))
    }
}

impl From<MachineLifecycleBindingFactsStoreWireV1> for MachineLifecycleBindingFacts {
    fn from(binding: MachineLifecycleBindingFactsStoreWireV1) -> Self {
        Self::new(
            binding.agent_runtime_id,
            binding.fence_token,
            binding.runtime_generation,
            binding.runtime_epoch_id,
        )
    }
}

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWire {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    unregister_progress: Option<MachineUnregisterProgressSnapshotStoreWire>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV3 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes a missing v3 field from explicit null progress"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    unregister_progress: Option<Option<MachineUnregisterProgressSnapshotStoreWire>>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV2 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWire,
    supervisor_authority: SupervisorAuthoritySnapshotStoreWire,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineUnregisterProgressSnapshotStoreWire {
    runtime_loop_drain_pending: bool,
    comms_drain_exit_pending: bool,
    completion_waiter_drain_pending: bool,
    runtime_loop_forced_abort: bool,
    comms_drain_forced_abort: bool,
}

impl From<&MachineUnregisterProgressSnapshot> for MachineUnregisterProgressSnapshotStoreWire {
    fn from(snapshot: &MachineUnregisterProgressSnapshot) -> Self {
        Self {
            runtime_loop_drain_pending: snapshot.runtime_loop_drain_pending(),
            comms_drain_exit_pending: snapshot.comms_drain_exit_pending(),
            completion_waiter_drain_pending: snapshot.completion_waiter_drain_pending(),
            runtime_loop_forced_abort: snapshot.runtime_loop_forced_abort(),
            comms_drain_forced_abort: snapshot.comms_drain_forced_abort(),
        }
    }
}

impl From<MachineUnregisterProgressSnapshotStoreWire> for MachineUnregisterProgressSnapshot {
    fn from(snapshot: MachineUnregisterProgressSnapshotStoreWire) -> Self {
        Self::new(
            snapshot.runtime_loop_drain_pending,
            snapshot.comms_drain_exit_pending,
            snapshot.completion_waiter_drain_pending,
            snapshot.runtime_loop_forced_abort,
            snapshot.comms_drain_forced_abort,
        )
    }
}

/// Exact pre-supervisor-authority lifecycle shape. Version 1 is decoded only
/// through this migration carrier so a missing authority on a current record
/// cannot be confused with legacy data.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MachineLifecycleSnapshotStoreWireV1 {
    record_version: u16,
    runtime_state: RuntimeState,
    binding: MachineLifecycleBindingFactsStoreWireV1,
}

#[derive(serde::Deserialize)]
struct MachineLifecycleSnapshotStoreVersionProbe {
    record_version: u16,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SupervisorAuthoritySnapshotStoreWire {
    #[default]
    UnboundNoReceipt,
    Bound {
        binding: SupervisorBindingReceiptStoreWire,
    },
    RevocationPending {
        pending: SupervisorRevocationPendingReceiptStoreWire,
    },
    RotationOperation {
        rotation: SupervisorRotationReceiptStoreWire,
    },
    RevokedReceipt {
        receipt: RevokedSupervisorReceiptStoreWire,
    },
    WithRotationHistory {
        current: Box<SupervisorAuthoritySnapshotStoreWire>,
        terminal_receipts: Vec<SupervisorRotationReceiptStoreWire>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorBindingReceiptStoreWire {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

impl From<&SupervisorBindingReceipt> for SupervisorBindingReceiptStoreWire {
    fn from(receipt: &SupervisorBindingReceipt) -> Self {
        Self {
            name: receipt.name().to_owned(),
            peer_id: receipt.peer_id().to_owned(),
            address: receipt.address().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<SupervisorBindingReceiptStoreWire> for SupervisorBindingReceipt {
    fn from(receipt: SupervisorBindingReceiptStoreWire) -> Self {
        Self::new(
            receipt.name,
            receipt.peer_id,
            receipt.address,
            receipt.signing_public_key,
            receipt.epoch,
        )
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokedSupervisorReceiptStoreWire {
    peer_id: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorRevocationPendingReceiptStoreWire {
    name: String,
    peer_id: String,
    address: String,
    signing_public_key: String,
    epoch: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SupervisorRotationReceiptStoreWire {
    operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
    phase: SupervisorRotationPersistencePhase,
    #[allow(
        clippy::option_option,
        reason = "serde distinguishes missing from explicit null"
    )]
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    rejection: Option<Option<SupervisorRotationRejection>>,
    previous: SupervisorBindingReceiptStoreWire,
    next: SupervisorBindingReceiptStoreWire,
}

impl From<&SupervisorRotationReceipt> for SupervisorRotationReceiptStoreWire {
    fn from(receipt: &SupervisorRotationReceipt) -> Self {
        Self {
            operation_id: receipt.operation_id(),
            phase: receipt.phase(),
            rejection: Some(receipt.rejection()),
            previous: receipt.previous().into(),
            next: receipt.next().into(),
        }
    }
}

impl TryFrom<SupervisorRotationReceiptStoreWire> for SupervisorRotationReceipt {
    type Error = RuntimeStoreError;

    fn try_from(receipt: SupervisorRotationReceiptStoreWire) -> Result<Self, Self::Error> {
        Ok(Self::new(
            receipt.operation_id,
            receipt.phase,
            require_present_nullable(receipt.rejection, "supervisor_authority.rotation.rejection")?,
            receipt.previous.into(),
            receipt.next.into(),
        ))
    }
}

impl From<&SupervisorRevocationPendingReceipt> for SupervisorRevocationPendingReceiptStoreWire {
    fn from(receipt: &SupervisorRevocationPendingReceipt) -> Self {
        Self {
            name: receipt.name().to_owned(),
            peer_id: receipt.peer_id().to_owned(),
            address: receipt.address().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<SupervisorRevocationPendingReceiptStoreWire> for SupervisorRevocationPendingReceipt {
    fn from(receipt: SupervisorRevocationPendingReceiptStoreWire) -> Self {
        Self::new(
            receipt.name,
            receipt.peer_id,
            receipt.address,
            receipt.signing_public_key,
            receipt.epoch,
        )
    }
}

impl From<&RevokedSupervisorReceipt> for RevokedSupervisorReceiptStoreWire {
    fn from(receipt: &RevokedSupervisorReceipt) -> Self {
        Self {
            peer_id: receipt.peer_id().to_owned(),
            signing_public_key: receipt.signing_public_key().to_owned(),
            epoch: receipt.epoch(),
        }
    }
}

impl From<RevokedSupervisorReceiptStoreWire> for RevokedSupervisorReceipt {
    fn from(receipt: RevokedSupervisorReceiptStoreWire) -> Self {
        Self::new(receipt.peer_id, receipt.signing_public_key, receipt.epoch)
    }
}

impl From<&SupervisorAuthoritySnapshot> for SupervisorAuthoritySnapshotStoreWire {
    fn from(snapshot: &SupervisorAuthoritySnapshot) -> Self {
        match snapshot {
            SupervisorAuthoritySnapshot::UnboundNoReceipt => Self::UnboundNoReceipt,
            SupervisorAuthoritySnapshot::Bound(binding) => Self::Bound {
                binding: binding.into(),
            },
            SupervisorAuthoritySnapshot::RevocationPending(pending) => Self::RevocationPending {
                pending: pending.into(),
            },
            SupervisorAuthoritySnapshot::RotationOperation(rotation) => Self::RotationOperation {
                rotation: rotation.into(),
            },
            SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => Self::RevokedReceipt {
                receipt: receipt.into(),
            },
            SupervisorAuthoritySnapshot::WithRotationHistory {
                current,
                terminal_receipts,
            } => Self::WithRotationHistory {
                current: Box::new(current.as_ref().into()),
                terminal_receipts: terminal_receipts.values().map(Into::into).collect(),
            },
        }
    }
}

fn supervisor_authority_read_error(
    context: &str,
    detail: impl std::fmt::Display,
) -> RuntimeStoreError {
    RuntimeStoreError::ReadFailed(format!("{context}: {detail}"))
}

fn validate_supervisor_descriptor(
    name: &str,
    peer_id: &str,
    address: &str,
    signing_public_key: &str,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let pubkey = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    let spec = meerkat_contracts::wire::supervisor_bridge::BridgePeerSpec {
        name: name.to_owned(),
        peer_id: peer_id.to_owned(),
        address: address.to_owned(),
        pubkey,
    };
    meerkat_core::comms::TrustedPeerDescriptor::try_from(&spec)
        .map(|_| ())
        .map_err(|error| supervisor_authority_read_error(context, error))
}

fn validate_supervisor_binding_receipt(
    receipt: &SupervisorBindingReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    validate_supervisor_descriptor(
        receipt.name(),
        receipt.peer_id(),
        receipt.address(),
        receipt.signing_public_key(),
        context,
    )
}

fn validate_revoked_supervisor_receipt(
    receipt: &RevokedSupervisorReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let pubkey =
        crate::comms_drain::decode_supervisor_signing_public_key(receipt.signing_public_key())
            .map_err(|error| supervisor_authority_read_error(context, error))?;
    if pubkey.iter().all(|byte| *byte == 0) {
        return Err(supervisor_authority_read_error(
            context,
            "supervisor signing public key must be non-zero",
        ));
    }
    let peer_id = meerkat_core::comms::PeerId::parse(receipt.peer_id())
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    let derived = meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey);
    if peer_id != derived {
        return Err(supervisor_authority_read_error(
            context,
            format!("peer id {peer_id} does not match signing-key-derived id {derived}"),
        ));
    }
    Ok(())
}

fn validate_supervisor_rotation_receipt(
    receipt: &SupervisorRotationReceipt,
    terminal_history: bool,
) -> Result<(), RuntimeStoreError> {
    let operation_id = receipt.operation_id();
    if operation_id.as_uuid().is_nil() {
        return Err(supervisor_authority_read_error(
            "supervisor rotation operation",
            "operation id must not be the nil UUID",
        ));
    }
    validate_supervisor_binding_receipt(
        receipt.previous(),
        &format!("supervisor rotation {operation_id} previous authority is invalid"),
    )?;

    let rejection_matches = matches!(
        (receipt.phase(), receipt.rejection()),
        (
            SupervisorRotationPersistencePhase::PreviousRevokePending
                | SupervisorRotationPersistencePhase::NextPublishPending
                | SupervisorRotationPersistencePhase::Completed,
            None
        ) | (SupervisorRotationPersistencePhase::Rejected, Some(_))
    );
    if !rejection_matches {
        return Err(supervisor_authority_read_error(
            "supervisor rotation operation",
            format!("{operation_id} has inconsistent rejection state"),
        ));
    }
    if terminal_history
        && !matches!(
            receipt.phase(),
            SupervisorRotationPersistencePhase::Completed
                | SupervisorRotationPersistencePhase::Rejected
        )
    {
        return Err(supervisor_authority_read_error(
            "supervisor rotation history",
            format!("{operation_id} is not terminal"),
        ));
    }

    match receipt.phase() {
        SupervisorRotationPersistencePhase::PreviousRevokePending
        | SupervisorRotationPersistencePhase::NextPublishPending => {
            validate_supervisor_binding_receipt(
                receipt.next(),
                &format!("supervisor rotation {operation_id} target is invalid"),
            )?;
            if receipt.next().epoch() <= receipt.previous().epoch() {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!(
                        "{operation_id} target epoch {} does not advance previous epoch {}",
                        receipt.next().epoch(),
                        receipt.previous().epoch()
                    ),
                ));
            }
        }
        SupervisorRotationPersistencePhase::Completed => {
            validate_supervisor_binding_receipt(
                receipt.next(),
                &format!("supervisor rotation {operation_id} target is invalid"),
            )?;
            // A legacy member may already have the exact target installed
            // before the operation protocol assigns an id. Its adoption
            // receipt is Completed with an exact previous == next witness.
            let exact_current_adoption = receipt.previous() == receipt.next();
            if !exact_current_adoption && receipt.next().epoch() <= receipt.previous().epoch() {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!(
                        "{operation_id} completed target epoch {} does not advance previous epoch {}",
                        receipt.next().epoch(),
                        receipt.previous().epoch()
                    ),
                ));
            }
        }
        SupervisorRotationPersistencePhase::Rejected => {
            let Some(rejection) = receipt.rejection() else {
                return Err(supervisor_authority_read_error(
                    "supervisor rotation operation",
                    format!("{operation_id} rejected without a rejection class"),
                ));
            };
            match rejection {
                SupervisorRotationRejection::InvalidTarget
                | SupervisorRotationRejection::UnsupportedProtocolVersion => {
                    // These two rejection classes retain the undecodable target
                    // fields as raw evidence. They are deliberately exempt from
                    // target descriptor validation and epoch comparison.
                }
                SupervisorRotationRejection::TargetEpochNotAdvanced => {
                    validate_supervisor_binding_receipt(
                        receipt.next(),
                        &format!("supervisor rotation {operation_id} rejected target is invalid"),
                    )?;
                    if receipt.next().epoch() > receipt.previous().epoch() {
                        return Err(supervisor_authority_read_error(
                            "supervisor rotation operation",
                            format!(
                                "{operation_id} rejected as non-advancing but target epoch {} advances previous epoch {}",
                                receipt.next().epoch(),
                                receipt.previous().epoch()
                            ),
                        ));
                    }
                }
                SupervisorRotationRejection::OperationConflict
                | SupervisorRotationRejection::NotBound
                | SupervisorRotationRejection::SenderMismatch => {
                    return Err(supervisor_authority_read_error(
                        "supervisor rotation operation",
                        format!(
                            "{operation_id} transient rejection {rejection:?} must not be persisted as a durable receipt"
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

type SupervisorEpochKeyIndex = std::collections::BTreeMap<u64, [u8; 32]>;

fn record_supervisor_epoch_key(
    epochs: &mut SupervisorEpochKeyIndex,
    epoch: u64,
    signing_public_key: &str,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    let key = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)
        .map_err(|error| supervisor_authority_read_error(context, error))?;
    if let Some(existing) = epochs.get(&epoch) {
        if existing != &key {
            return Err(supervisor_authority_read_error(
                context,
                format!("epoch {epoch} is bound to conflicting supervisor signing keys"),
            ));
        }
    } else {
        epochs.insert(epoch, key);
    }
    Ok(())
}

fn record_supervisor_binding_epoch(
    epochs: &mut SupervisorEpochKeyIndex,
    receipt: &SupervisorBindingReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    record_supervisor_epoch_key(
        epochs,
        receipt.epoch(),
        receipt.signing_public_key(),
        context,
    )
}

fn record_rotation_authoritative_epochs(
    epochs: &mut SupervisorEpochKeyIndex,
    receipt: &SupervisorRotationReceipt,
    context: &str,
) -> Result<(), RuntimeStoreError> {
    record_supervisor_binding_epoch(epochs, receipt.previous(), context)?;
    if matches!(
        receipt.phase(),
        SupervisorRotationPersistencePhase::PreviousRevokePending
            | SupervisorRotationPersistencePhase::NextPublishPending
            | SupervisorRotationPersistencePhase::Completed
    ) {
        record_supervisor_binding_epoch(epochs, receipt.next(), context)?;
    }
    Ok(())
}

fn record_current_authoritative_epochs(
    epochs: &mut SupervisorEpochKeyIndex,
    current: &SupervisorAuthoritySnapshot,
) -> Result<(), RuntimeStoreError> {
    match current {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => Ok(()),
        SupervisorAuthoritySnapshot::Bound(binding) => {
            record_supervisor_binding_epoch(epochs, binding, "current supervisor authority")
        }
        SupervisorAuthoritySnapshot::RevocationPending(pending) => record_supervisor_epoch_key(
            epochs,
            pending.epoch(),
            pending.signing_public_key(),
            "current pending supervisor revocation authority",
        ),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => {
            record_rotation_authoritative_epochs(
                epochs,
                rotation,
                "current supervisor rotation authority",
            )
        }
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => record_supervisor_epoch_key(
            epochs,
            receipt.epoch(),
            receipt.signing_public_key(),
            "current revoked supervisor authority",
        ),
        SupervisorAuthoritySnapshot::WithRotationHistory { .. } => {
            Err(RuntimeStoreError::ReadFailed(
                "nested supervisor rotation history is not canonical".to_string(),
            ))
        }
    }
}

fn current_supervisor_epoch(current: &SupervisorAuthoritySnapshot) -> Option<u64> {
    match current {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => None,
        SupervisorAuthoritySnapshot::Bound(binding) => Some(binding.epoch()),
        SupervisorAuthoritySnapshot::RevocationPending(pending) => Some(pending.epoch()),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => Some(match rotation.phase() {
            SupervisorRotationPersistencePhase::PreviousRevokePending
            | SupervisorRotationPersistencePhase::Rejected => rotation.previous().epoch(),
            SupervisorRotationPersistencePhase::NextPublishPending
            | SupervisorRotationPersistencePhase::Completed => rotation.next().epoch(),
        }),
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => Some(receipt.epoch()),
        SupervisorAuthoritySnapshot::WithRotationHistory { .. } => None,
    }
}

fn terminal_rotation_authority_epoch(receipt: &SupervisorRotationReceipt) -> u64 {
    match receipt.phase() {
        SupervisorRotationPersistencePhase::Completed => receipt.next().epoch(),
        SupervisorRotationPersistencePhase::Rejected => receipt.previous().epoch(),
        SupervisorRotationPersistencePhase::PreviousRevokePending
        | SupervisorRotationPersistencePhase::NextPublishPending => receipt.previous().epoch(),
    }
}

fn validate_supervisor_rotation_history_coherence(
    current: &SupervisorAuthoritySnapshot,
    terminal_receipts: &std::collections::BTreeMap<
        meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        SupervisorRotationReceipt,
    >,
) -> Result<(), RuntimeStoreError> {
    let Some(current_epoch) = current_supervisor_epoch(current) else {
        return Err(RuntimeStoreError::ReadFailed(
            "supervisor rotation history requires a current authority epoch".to_string(),
        ));
    };

    let mut epochs = SupervisorEpochKeyIndex::new();
    record_current_authoritative_epochs(&mut epochs, current)?;
    let mut history_high_water = 0;
    for receipt in terminal_receipts.values() {
        record_rotation_authoritative_epochs(
            &mut epochs,
            receipt,
            "supervisor rotation history authority",
        )?;
        history_high_water = history_high_water.max(terminal_rotation_authority_epoch(receipt));
    }
    if current_epoch < history_high_water {
        return Err(RuntimeStoreError::ReadFailed(format!(
            "current supervisor epoch {current_epoch} is below terminal rotation history high-water {history_high_water}"
        )));
    }
    Ok(())
}

fn validate_supervisor_authority_snapshot(
    snapshot: &SupervisorAuthoritySnapshot,
) -> Result<(), RuntimeStoreError> {
    match snapshot {
        SupervisorAuthoritySnapshot::UnboundNoReceipt => Ok(()),
        SupervisorAuthoritySnapshot::Bound(binding) => {
            validate_supervisor_binding_receipt(binding, "bound supervisor is invalid")
        }
        SupervisorAuthoritySnapshot::RevocationPending(pending) => validate_supervisor_descriptor(
            pending.name(),
            pending.peer_id(),
            pending.address(),
            pending.signing_public_key(),
            "pending supervisor revocation authority is invalid",
        ),
        SupervisorAuthoritySnapshot::RotationOperation(rotation) => {
            validate_supervisor_rotation_receipt(rotation, false)
        }
        SupervisorAuthoritySnapshot::RevokedReceipt(receipt) => {
            validate_revoked_supervisor_receipt(receipt, "revoked supervisor receipt is invalid")
        }
        SupervisorAuthoritySnapshot::WithRotationHistory {
            current,
            terminal_receipts,
        } => {
            if matches!(
                current.as_ref(),
                SupervisorAuthoritySnapshot::WithRotationHistory { .. }
            ) {
                return Err(RuntimeStoreError::ReadFailed(
                    "nested supervisor rotation history is not canonical".to_string(),
                ));
            }
            if terminal_receipts.is_empty() {
                return Err(RuntimeStoreError::ReadFailed(
                    "empty supervisor rotation history wrapper is not canonical".to_string(),
                ));
            }
            validate_supervisor_authority_snapshot(current)?;
            for (operation_id, receipt) in terminal_receipts {
                if operation_id != &receipt.operation_id() {
                    return Err(RuntimeStoreError::ReadFailed(format!(
                        "supervisor rotation history key {operation_id} does not match receipt id {}",
                        receipt.operation_id()
                    )));
                }
                validate_supervisor_rotation_receipt(receipt, true)?;
            }
            if let SupervisorAuthoritySnapshot::RotationOperation(active) = current.as_ref()
                && terminal_receipts.contains_key(&active.operation_id())
            {
                return Err(RuntimeStoreError::ReadFailed(
                    "active supervisor rotation is duplicated in terminal history".to_string(),
                ));
            }
            validate_supervisor_rotation_history_coherence(current, terminal_receipts)
        }
    }
}

impl TryFrom<SupervisorAuthoritySnapshotStoreWire> for SupervisorAuthoritySnapshot {
    type Error = RuntimeStoreError;

    fn try_from(snapshot: SupervisorAuthoritySnapshotStoreWire) -> Result<Self, Self::Error> {
        match snapshot {
            SupervisorAuthoritySnapshotStoreWire::UnboundNoReceipt => Ok(Self::UnboundNoReceipt),
            SupervisorAuthoritySnapshotStoreWire::Bound { binding } => {
                let binding = binding.into();
                validate_supervisor_binding_receipt(&binding, "bound supervisor is invalid")?;
                Ok(Self::Bound(binding))
            }
            SupervisorAuthoritySnapshotStoreWire::RevocationPending { pending } => {
                let pending: SupervisorRevocationPendingReceipt = pending.into();
                validate_supervisor_descriptor(
                    pending.name(),
                    pending.peer_id(),
                    pending.address(),
                    pending.signing_public_key(),
                    "pending supervisor revocation authority is invalid",
                )?;
                Ok(Self::RevocationPending(pending))
            }
            SupervisorAuthoritySnapshotStoreWire::RotationOperation { rotation } => {
                let receipt: SupervisorRotationReceipt = rotation.try_into()?;
                validate_supervisor_rotation_receipt(&receipt, false)?;
                Ok(Self::RotationOperation(receipt))
            }
            SupervisorAuthoritySnapshotStoreWire::RevokedReceipt { receipt } => {
                let receipt = receipt.into();
                validate_revoked_supervisor_receipt(
                    &receipt,
                    "revoked supervisor receipt is invalid",
                )?;
                Ok(Self::RevokedReceipt(receipt))
            }
            SupervisorAuthoritySnapshotStoreWire::WithRotationHistory {
                current,
                terminal_receipts,
            } => {
                if terminal_receipts.is_empty() {
                    return Err(RuntimeStoreError::ReadFailed(
                        "empty supervisor rotation history wrapper is not canonical".to_string(),
                    ));
                }
                let current = Self::try_from(*current)?;
                if matches!(current, Self::WithRotationHistory { .. }) {
                    return Err(RuntimeStoreError::ReadFailed(
                        "nested supervisor rotation history is not canonical".to_string(),
                    ));
                }
                let mut receipts = std::collections::BTreeMap::new();
                for wire in terminal_receipts {
                    let receipt: SupervisorRotationReceipt = wire.try_into()?;
                    validate_supervisor_rotation_receipt(&receipt, true)?;
                    if receipts.insert(receipt.operation_id(), receipt).is_some() {
                        return Err(RuntimeStoreError::ReadFailed(
                            "supervisor rotation history contains a duplicate operation id"
                                .to_string(),
                        ));
                    }
                }
                if let Self::RotationOperation(active) = &current
                    && receipts.contains_key(&active.operation_id())
                {
                    return Err(RuntimeStoreError::ReadFailed(
                        "active supervisor rotation is duplicated in terminal history".to_string(),
                    ));
                }
                let snapshot = Self::WithRotationHistory {
                    current: Box::new(current),
                    terminal_receipts: receipts,
                };
                validate_supervisor_authority_snapshot(&snapshot)?;
                Ok(snapshot)
            }
        }
    }
}

impl From<&MachineLifecycleSnapshot> for MachineLifecycleSnapshotStoreWire {
    fn from(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            record_version: MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            runtime_state: snapshot.runtime_state(),
            binding: snapshot.binding().into(),
            supervisor_authority: snapshot.supervisor_authority().into(),
            unregister_progress: snapshot.unregister_progress().map(Into::into),
        }
    }
}

fn validate_unregister_progress_snapshot(
    progress: Option<&MachineUnregisterProgressSnapshot>,
) -> Result<(), RuntimeStoreError> {
    if let Some(progress) = progress {
        if progress.runtime_loop_drain_pending() && progress.runtime_loop_forced_abort() {
            return Err(RuntimeStoreError::ReadFailed(
                "unregister runtime-loop forced disposition cannot precede obligation closure"
                    .into(),
            ));
        }
        if progress.comms_drain_exit_pending() && progress.comms_drain_forced_abort() {
            return Err(RuntimeStoreError::ReadFailed(
                "unregister comms-drain forced disposition cannot precede obligation closure"
                    .into(),
            ));
        }
    }
    Ok(())
}

impl TryFrom<MachineLifecycleSnapshotStoreWireV3> for MachineLifecycleSnapshot {
    type Error = RuntimeStoreError;

    fn try_from(record: MachineLifecycleSnapshotStoreWireV3) -> Result<Self, Self::Error> {
        if record.record_version != MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
            return Err(RuntimeStoreError::ReadFailed(format!(
                "unsupported machine lifecycle store record version {}",
                record.record_version
            )));
        }
        let unregister_progress =
            require_present_nullable(record.unregister_progress, "unregister_progress")?
                .map(Into::into);
        validate_unregister_progress_snapshot(unregister_progress.as_ref())?;
        Ok(Self::new_with_unregister_progress(
            record.runtime_state,
            record.binding.try_into()?,
            record.supervisor_authority.try_into()?,
            unregister_progress,
        ))
    }
}

fn decode_machine_lifecycle_store_record(
    bytes: &[u8],
) -> Result<MachineLifecycleSnapshot, RuntimeStoreError> {
    let version = serde_json::from_slice::<MachineLifecycleSnapshotStoreVersionProbe>(bytes)
        .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
    match version.record_version {
        LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV1>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            if record.record_version != LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "unsupported machine lifecycle store record version {}",
                    record.record_version
                )));
            }
            Ok(MachineLifecycleSnapshot::new(
                record.runtime_state,
                record.binding.into(),
                SupervisorAuthoritySnapshot::UnboundNoReceipt,
            ))
        }
        SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV2>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            if record.record_version != SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION {
                return Err(RuntimeStoreError::ReadFailed(format!(
                    "unsupported machine lifecycle store record version {}",
                    record.record_version
                )));
            }
            Ok(MachineLifecycleSnapshot::new(
                record.runtime_state,
                record.binding.try_into()?,
                record.supervisor_authority.try_into()?,
            ))
        }
        MACHINE_LIFECYCLE_STORE_RECORD_VERSION => {
            let record = serde_json::from_slice::<MachineLifecycleSnapshotStoreWireV3>(bytes)
                .map_err(|err| RuntimeStoreError::ReadFailed(err.to_string()))?;
            MachineLifecycleSnapshot::try_from(record)
        }
        unsupported => Err(RuntimeStoreError::ReadFailed(format!(
            "unsupported machine lifecycle store record version {unsupported}"
        ))),
    }
}

/// Load the last persisted runtime-state projection from a generated lifecycle
/// record.
///
/// This is a projection of [`MachineLifecycleCommit`] authority. Store
/// implementations provide only opaque record bytes; the runtime crate owns the
/// decoding and rejects compatibility rows that are not machine lifecycle
/// records.
pub async fn load_runtime_state(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<RuntimeState>, RuntimeStoreError> {
    Ok(load_machine_lifecycle(store, runtime_id)
        .await?
        .map(|snapshot| snapshot.runtime_state()))
}

pub(crate) async fn load_machine_lifecycle(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Option<MachineLifecycleSnapshot>, RuntimeStoreError> {
    store
        .load_machine_lifecycle_record(runtime_id)
        .await?
        .map(|bytes| decode_machine_lifecycle_store_record(&bytes))
        .transpose()
}

/// Declared durable store record for generated machine lifecycle truth.
///
/// Stores receive this record from [`MachineLifecycleCommit`] and may persist
/// its encoded form. Loading must decode this exact record shape; compatibility
/// runtime-state projections are not lifecycle authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleStoreRecord {
    snapshot: MachineLifecycleSnapshot,
}

impl MachineLifecycleStoreRecord {
    pub(crate) fn from_snapshot(snapshot: &MachineLifecycleSnapshot) -> Self {
        Self {
            snapshot: snapshot.clone(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, RuntimeStoreError> {
        validate_supervisor_authority_snapshot(self.snapshot.supervisor_authority())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        validate_unregister_progress_snapshot(self.snapshot.unregister_progress())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let wire = MachineLifecycleSnapshotStoreWire::from(&self.snapshot);
        serde_json::to_vec(&wire).map_err(|err| RuntimeStoreError::WriteFailed(err.to_string()))
    }
}

/// Machine-owned lifecycle commit token.
///
/// This token has no public constructor. RuntimeStore implementors can persist
/// the selected state and binding facts, but callers outside the machine/driver
/// commit path cannot select arbitrary lifecycle truth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineLifecycleCommit {
    snapshot: MachineLifecycleSnapshot,
}

impl MachineLifecycleCommit {
    #[cfg(test)]
    pub(crate) fn new_with_binding(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
    ) -> Self {
        Self::new_with_binding_and_unregister_progress(
            runtime_state,
            binding,
            supervisor_authority,
            None,
        )
    }

    pub(crate) fn new_with_binding_and_unregister_progress(
        runtime_state: RuntimeState,
        binding: MachineLifecycleBindingFacts,
        supervisor_authority: SupervisorAuthoritySnapshot,
        unregister_progress: Option<MachineUnregisterProgressSnapshot>,
    ) -> Self {
        Self {
            snapshot: MachineLifecycleSnapshot::new_with_unregister_progress(
                runtime_state,
                binding,
                supervisor_authority,
                unregister_progress,
            ),
        }
    }

    /// Runtime state selected by the owning MeerkatMachine transition.
    pub fn runtime_state(&self) -> RuntimeState {
        self.snapshot.runtime_state()
    }

    /// Durable lifecycle snapshot selected by the owning MeerkatMachine transition.
    pub fn snapshot(&self) -> &MachineLifecycleSnapshot {
        &self.snapshot
    }

    /// Durable record selected by the owning MeerkatMachine transition.
    pub fn store_record(&self) -> MachineLifecycleStoreRecord {
        MachineLifecycleStoreRecord::from_snapshot(&self.snapshot)
    }

    pub(crate) fn into_snapshot(self) -> MachineLifecycleSnapshot {
        self.snapshot
    }
}

/// Machine-authorized final-unregister persistence token.
///
/// The token bundles terminal lifecycle truth with the exact authorized input
/// snapshot. It has no public constructor and can only be minted by consuming
/// the private-field delete witness derived from the generated
/// `DeleteSnapshot` unregister verdict.
#[derive(Debug, Clone)]
pub struct UnregisterFinalizationCommit {
    machine_lifecycle: MachineLifecycleCommit,
    input_states: Vec<InputStatePersistenceRecord>,
    retired_ops_epoch: meerkat_core::RuntimeEpochId,
}

impl UnregisterFinalizationCommit {
    pub(crate) fn new(
        machine_lifecycle: MachineLifecycleCommit,
        input_states: Vec<InputStatePersistenceRecord>,
        retired_ops_epoch: meerkat_core::RuntimeEpochId,
        _authority: crate::meerkat_machine::DeleteOpsFinalizationAuthority,
    ) -> Self {
        Self {
            machine_lifecycle,
            input_states,
            retired_ops_epoch,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        MachineLifecycleSnapshot,
        Vec<InputStatePersistenceRecord>,
        meerkat_core::RuntimeEpochId,
    ) {
        (
            self.machine_lifecycle.into_snapshot(),
            self.input_states,
            self.retired_ops_epoch,
        )
    }

    /// Opaque encoded lifecycle record selected by final-unregister machine
    /// authority. External stores can persist this without gaining a way to
    /// construct or alter the authority token.
    pub fn lifecycle_store_record(&self) -> MachineLifecycleStoreRecord {
        self.machine_lifecycle.store_record()
    }

    /// Authorized input-state rows that must commit in the same transaction.
    pub fn input_states(&self) -> &[InputStatePersistenceRecord] {
        &self.input_states
    }

    /// Exact ops epoch retired by this finalization transaction.
    pub fn retired_ops_epoch(&self) -> &meerkat_core::RuntimeEpochId {
        &self.retired_ops_epoch
    }
}

/// Atomic persistence interface for runtime state.
///
/// Implementations:
/// - `InMemoryRuntimeStore` — in-memory, no durability (ephemeral/testing)
/// - `SqliteRuntimeStore` — SQLite-backed durable runtime state
///
/// A store may contain many logical runtime ids, but each id is controlled by
/// one live `MeerkatMachine` authority. Store transactions provide durable
/// atomicity; they are not a distributed lease for two machines concurrently
/// controlling the same logical runtime.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait RuntimeStore: Send + Sync {
    /// Whether [`RuntimeStore::atomic_apply`] durably records typed compaction
    /// projection intents in the same boundary as the session rewrite.
    /// Unknown/custom stores fail closed by default.
    fn supports_compaction_projection_outbox(&self) -> bool {
        false
    }

    /// Stable key for process-local auth/OAuth authority reuse across reopened
    /// handles for the same durable store.
    fn auth_authority_key(&self) -> Option<String> {
        None
    }

    /// Persist the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// The AuthMachine owns admission/consume semantics; this payload snapshot
    /// carries the PKCE verifier and device-code correlation data needed to
    /// rehydrate active flows after a persistent runtime process restart.
    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        let _ = snapshot_json;
        Err(RuntimeStoreError::Unsupported(
            "persist_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Load the runtime-owned OAuth login-flow payload snapshot, if present.
    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "load_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically update the runtime-owned OAuth login-flow payload snapshot.
    ///
    /// Stores that support OAuth snapshots must override this with a lock,
    /// transaction, or compare-and-swap boundary. A load/compute/persist
    /// fallback is not safe for admission, capacity, or consume claims.
    fn update_auth_oauth_flow_snapshot(
        &self,
        _update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "update_auth_oauth_flow_snapshot".into(),
        ))
    }

    /// Atomically persist a session snapshot that is not a run boundary.
    ///
    /// Session-control snapshots update durable session authority without
    /// producing a [`RunBoundaryReceipt`].
    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically persist a same-session transcript rewrite snapshot.
    ///
    /// Store implementations that support transcript edits must compare the
    /// currently persisted session transcript revision with `commit.parent_revision`
    /// inside the same lock or transaction that writes `session_delta`.
    async fn commit_session_transcript_rewrite_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        commit: &meerkat_core::TranscriptRewriteCommit,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, session_delta, commit);
        Err(RuntimeStoreError::Unsupported(
            "commit_session_transcript_rewrite_snapshot".into(),
        ))
    }

    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All three writes MUST commit in a single atomic operation.
    /// If any write fails, none should be visible.
    /// Atomically persist session delta + receipt + input state updates.
    ///
    /// All writes MUST commit in a single atomic operation.
    /// If `session_store_key` is `Some`, validates that the snapshot belongs
    /// to that session and, for stores that physically share a `SessionStore`
    /// table, writes that table in the same transaction. Runtime snapshot
    /// authority remains keyed only by `runtime_id`; `session_store_key` must
    /// not create a raw session UUID runtime alias.
    /// Compaction intents must be inserted as pending outbox rows in this same
    /// boundary. An intent whose exact outbox identity is already finalized is
    /// a stale snapshot replay and must be rejected without mutating any part
    /// of the boundary.
    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SessionDelta>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError>;

    /// Load exact compaction projection intents committed by atomic_apply but
    /// not yet acknowledged as finalized by the memory store.
    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "load_pending_compaction_projections".to_string(),
        ))
    }

    /// Idempotently acknowledge post-commit memory finalization.
    ///
    /// The acknowledgement and removal of this exact intent from the
    /// authoritative persisted session snapshot MUST occur in one atomic
    /// boundary. The finalized outbox row remains as a tombstone so later
    /// snapshot writes can reject stale metadata replay.
    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, projection);
        Err(RuntimeStoreError::Unsupported(
            "mark_compaction_projection_finalized".to_string(),
        ))
    }

    /// Atomically persist a failed-but-applied runtime turn.
    ///
    /// This is the machine-terminal counterpart to [`Self::atomic_apply`]:
    /// the mutated session snapshot, boundary receipt, generated machine
    /// lifecycle record, and input/outbox state must become visible in one
    /// transaction. Implementations must never compose this from separate
    /// `atomic_apply` and `commit_machine_lifecycle` calls.
    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SessionDelta,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (
            runtime_id,
            session_delta,
            receipt,
            machine_lifecycle,
            input_updates,
            session_store_key,
        );
        Err(RuntimeStoreError::Unsupported(
            "atomic_apply_with_machine_lifecycle".to_string(),
        ))
    }

    /// Load all input states for a runtime.
    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError>;

    /// Load a specific boundary receipt.
    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError>;

    /// Load the latest committed session snapshot for a runtime, if any.
    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError>;

    /// Remove the latest committed session snapshot for a runtime.
    ///
    /// This is used only as a fail-closed quarantine path after a compatibility
    /// projection write rejects a runtime snapshot that was already staged as
    /// runtime authority and the service cannot restore the previous snapshot.
    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError>;

    /// Replace the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// Used by fail-closed recovery after a compatibility projection rejected
    /// a runtime snapshot. Implementations must compare and write atomically so
    /// recovery cannot overwrite a newer runtime-authoritative snapshot.
    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError>;

    /// Remove the latest committed session snapshot only if it still matches
    /// `expected_current`.
    ///
    /// This is the conditional variant of the fail-closed quarantine path.
    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError>;

    /// Report whether the runtime-projection fallback for `runtime_id` is
    /// quarantined.
    ///
    /// This is a durable single-owner fact: when
    /// [`clear_session_snapshot_if_current`](Self::clear_session_snapshot_if_current)
    /// matches and DELETEs a rejected runtime snapshot, the same atomic boundary
    /// records the quarantine marker. A subsequent live snapshot write clears it.
    /// Recovery reads this to decide whether a store-only projection may stand in
    /// for an absent runtime snapshot. The default is fail-safe (`false`): stores
    /// that cannot record the marker durably never claim a snapshot is
    /// quarantined.
    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        let _ = runtime_id;
        Ok(false)
    }

    /// Persist a single input state (for durable-before-ack).
    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically persist a batch of machine-authorized input shell updates.
    /// Used by per-input terminal outboxes so an N-input batch can never
    /// expose a mixed provisional/finalized or finalized/published phase.
    async fn persist_input_states_atomically(
        &self,
        _runtime_id: &LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        if states.is_empty() {
            return Ok(());
        }
        Err(RuntimeStoreError::Unsupported(
            "persist_input_states_atomically".to_string(),
        ))
    }

    /// Atomically replace an exact set of input-state rows only when every
    /// currently persisted row is byte-identical to its expected
    /// [`StoredInputState`] serialization.
    ///
    /// Expected and replacement batches must contain the same unique keys and
    /// at most [`MAX_INPUT_STATE_BATCH_CAS`] rows. If every current row already
    /// equals its replacement, implementations return
    /// [`InputStateBatchCasOutcome::Swapped`] without rewriting it; this makes
    /// a committed store-first transaction retryable after caller
    /// cancellation or acknowledgement loss. Missing rows, mixed
    /// expected/replacement images, and any other changed durable rows return
    /// [`InputStateBatchCasOutcome::Stale`] without writing a replacement.
    /// Implementations must hold one lock/transaction across the complete
    /// comparison and write set.
    async fn compare_and_swap_input_states_atomically(
        &self,
        _runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        let prepared = prepare_input_state_batch_cas(expected, replacements)?;
        if prepared.is_empty() {
            return Ok(InputStateBatchCasOutcome::Swapped);
        }
        Err(RuntimeStoreError::Unsupported(
            "compare_and_swap_input_states_atomically".to_string(),
        ))
    }

    /// Load a single input state.
    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError>;

    /// Load the last persisted machine lifecycle record bytes, if any.
    ///
    /// Implementations return only the opaque bytes previously obtained from
    /// [`MachineLifecycleCommit::store_record`]. The runtime crate decodes
    /// these bytes through `load_runtime_state` or internal recovery helpers;
    /// stores must not promote compatibility rows or bare runtime states into
    /// lifecycle authority.
    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError>;

    /// Atomically commit machine-owned lifecycle state changes.
    ///
    /// Writes runtime state, generated runtime binding facts, and all input
    /// state updates in a single atomic operation. `MachineLifecycleCommit` has
    /// no public constructor, so this cannot be used by compatibility callers
    /// to pick runtime truth.
    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError>;

    /// Atomically publish final unregister lifecycle truth and retire the
    /// matching ops-lifecycle epoch.
    ///
    /// The lifecycle record, input-state updates, and ops snapshot deletion
    /// MUST commit in one store transaction (or one indivisible in-memory
    /// critical section). A terminal lifecycle record with the old ops epoch
    /// still present is forbidden: recovery would otherwise resurrect stale
    /// operation/cursor authority after unregister. The commit also carries
    /// the exact retired ops epoch; implementations MUST atomically retain a
    /// durable deletion-wins fence for it, and every later
    /// `persist_ops_lifecycle` for that epoch must return
    /// [`RuntimeStoreError::OpsLifecycleEpochRetired`] rather than recreate the
    /// row. Implementations must also be idempotent so retry after a process
    /// crash following commit converges on the same terminal lifecycle with no
    /// ops snapshot and the same epoch fence.
    ///
    /// `Ok(())` means the whole finalization is visible. Every error except
    /// [`RuntimeStoreError::UnregisterFinalizationOutcomeUnknown`] MUST mean
    /// none of it is visible. A backend with an ambiguous commit
    /// acknowledgement must first resolve that ambiguity internally by
    /// reading its transaction authority. It may use the typed unknown error
    /// only when it cannot prove either the exact final state or the exact
    /// pre-transaction state; callers then retry without a durable rollback.
    /// The opaque token also proves the generated `DeleteSnapshot` verdict and
    /// bundles the exact lifecycle and input rows selected by the machine.
    ///
    /// The returned future is also a cancellation boundary: after it is
    /// dropped, no mutation from that invocation may become visible later.
    /// An implementation may leave the prior pair untouched or finish the
    /// entire atomic commit before cancellation is observable, but it must not
    /// detach a background write that can cross a same-runtime-ID replacement.
    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, finalization);
        Err(RuntimeStoreError::Unsupported(
            "commit_unregister_finalization".into(),
        ))
    }

    /// Atomically initialize the ops lifecycle row if it is absent and return
    /// the canonical durable snapshot.
    ///
    /// The absence check, optional insert, and canonical read MUST share one
    /// store transaction (or one indivisible in-memory critical section).
    /// Concurrent initializer calls for the same runtime must therefore all
    /// observe the same epoch: exactly one candidate may become durable and
    /// every losing caller receives that winner's snapshot. The machine's
    /// stable registration transaction separately spans this store call
    /// through map publication/removal; this method is not a distributed
    /// machine lease. Implementations must also reject a candidate whose epoch
    /// is already covered by the unregister deletion-wins fence.
    ///
    /// Cancellation may leave the candidate as the canonical empty row: no
    /// bindings escape before this await completes, and the next registrar
    /// adopts the returned durable epoch. A cancelled invocation must never
    /// overwrite a row that was already present.
    ///
    /// There is intentionally no load-then-persist default. Custom stores
    /// that support durable ops lifecycle state must implement this atomic
    /// boundary or fail closed with [`RuntimeStoreError::Unsupported`].
    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &crate::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<crate::ops_lifecycle::PersistedOpsSnapshot, RuntimeStoreError> {
        let _ = (runtime_id, candidate);
        Err(RuntimeStoreError::Unsupported(
            "initialize_ops_lifecycle_if_absent".into(),
        ))
    }

    /// Persist a snapshot of the ops lifecycle registry state.
    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &crate::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (runtime_id, snapshot);
        Err(RuntimeStoreError::Unsupported(
            "persist_ops_lifecycle".into(),
        ))
    }

    /// Load a previously persisted ops lifecycle snapshot.
    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<crate::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported("load_ops_lifecycle".into()))
    }

    /// Delete a previously persisted ops lifecycle snapshot.
    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        let _ = runtime_id;
        Err(RuntimeStoreError::Unsupported(
            "delete_ops_lifecycle".into(),
        ))
    }

    // -----------------------------------------------------------------------
    // Mob host binding rows (`runtime_mob_host_bindings`, multi-host mobs R8)
    // -----------------------------------------------------------------------
    //
    // Raw record-JSON accessors only: the TYPED record and the
    // transition-derived persistence authorities live mob-side
    // (`meerkat-mob/src/runtime/host_actor.rs`); this store never interprets
    // the blob. CAS compares the full serialized record, mirroring the
    // `mob_runtime_supervisors` mechanics.

    /// Load the persisted host-binding record blob for `mob_id`, if any.
    async fn load_mob_host_binding(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _ = mob_id;
        Err(RuntimeStoreError::Unsupported(
            "load_mob_host_binding".into(),
        ))
    }

    /// List every persisted host-binding row (boot recovery).
    async fn list_mob_host_bindings(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_mob_host_bindings".into(),
        ))
    }

    /// Insert the host-binding row for `mob_id` iff absent. Returns whether
    /// the row was inserted.
    async fn put_mob_host_binding_if_absent(
        &self,
        mob_id: &str,
        record_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, record_json);
        Err(RuntimeStoreError::Unsupported(
            "put_mob_host_binding_if_absent".into(),
        ))
    }

    /// Replace the host-binding row for `mob_id` iff the stored blob equals
    /// `expected_json`. Returns whether the swap applied.
    async fn compare_and_put_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
        next_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_json, next_json);
        Err(RuntimeStoreError::Unsupported(
            "compare_and_put_mob_host_binding".into(),
        ))
    }

    /// Delete the host-binding row for `mob_id` iff the stored blob equals
    /// `expected_json`. Returns whether a row was deleted.
    async fn delete_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_json);
        Err(RuntimeStoreError::Unsupported(
            "delete_mob_host_binding".into(),
        ))
    }

    /// Load the durable receipt for an already-completed host revocation.
    ///
    /// The blob is deliberately separate from `runtime_mob_host_bindings`:
    /// boot recovery must never mistake a revoke retry receipt for a live
    /// binding or revive the materialized-member rows that the revoke
    /// removed. The typed receipt and its transition witness live mob-side;
    /// this store treats it as opaque bytes.
    async fn load_mob_host_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        let _ = mob_id;
        Err(RuntimeStoreError::Unsupported(
            "load_mob_host_revocation".into(),
        ))
    }

    /// List durable host-revocation receipts for boot recovery of exact
    /// reply-loss retries. Receipts are not bindings and carry no member
    /// revival rows.
    async fn list_mob_host_revocations(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        Err(RuntimeStoreError::Unsupported(
            "list_mob_host_revocations".into(),
        ))
    }

    /// Atomically delete the expected active binding and publish its revoke
    /// receipt. Returns `false` when the expected binding did not match; in
    /// that case neither write is visible.
    ///
    /// This is the durable terminal boundary for host revocation. A crash
    /// before it leaves the binding retryable; a crash after it leaves no
    /// binding/member rows to revive and an exact receipt to replay.
    async fn revoke_mob_host_binding(
        &self,
        mob_id: &str,
        expected_binding_json: &[u8],
        receipt_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        let _ = (mob_id, expected_binding_json, receipt_json);
        Err(RuntimeStoreError::Unsupported(
            "revoke_mob_host_binding".into(),
        ))
    }
}

pub use memory::InMemoryRuntimeStore;
#[cfg(feature = "sqlite-store")]
pub use sqlite::SqliteRuntimeStore;

#[cfg(test)]
mod lifecycle_record_compatibility_tests {
    use super::*;

    fn operation_id(
        value: u128,
    ) -> meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId {
        meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId::from_uuid(
            uuid::Uuid::from_u128(value),
        )
    }

    fn binding(seed: u8, name: &str, epoch: u64) -> SupervisorBindingReceipt {
        let pubkey = [seed; 32];
        SupervisorBindingReceipt::new(
            name.to_string(),
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&pubkey).as_str(),
            format!("inproc://{name}"),
            crate::comms_drain::encode_supervisor_signing_public_key(pubkey),
            epoch,
        )
    }

    fn rotation(
        operation_id: meerkat_contracts::wire::supervisor_bridge::SupervisorRotationOperationId,
        phase: SupervisorRotationPersistencePhase,
        rejection: Option<SupervisorRotationRejection>,
        previous: SupervisorBindingReceipt,
        next: SupervisorBindingReceipt,
    ) -> SupervisorRotationReceipt {
        SupervisorRotationReceipt::new(operation_id, phase, rejection, previous, next)
    }

    fn snapshot(authority: SupervisorAuthoritySnapshot) -> MachineLifecycleSnapshot {
        MachineLifecycleSnapshot::new(
            RuntimeState::Idle,
            MachineLifecycleBindingFacts::new(None, None, None, None),
            authority,
        )
    }

    fn encode_snapshot(snapshot: &MachineLifecycleSnapshot) -> Vec<u8> {
        MachineLifecycleStoreRecord::from_snapshot(snapshot)
            .encode()
            .expect("encode lifecycle snapshot")
    }

    fn encode_unvalidated_snapshot(snapshot: &MachineLifecycleSnapshot) -> Vec<u8> {
        serde_json::to_vec(&MachineLifecycleSnapshotStoreWire::from(snapshot))
            .expect("serialize deliberately corrupt lifecycle snapshot")
    }

    fn encoded_value(snapshot: &MachineLifecycleSnapshot) -> serde_json::Value {
        serde_json::from_slice(&encode_snapshot(snapshot)).expect("decode encoded snapshot as JSON")
    }

    fn assert_decode_fails(value: serde_json::Value) {
        let bytes = serde_json::to_vec(&value).expect("serialize corrupt lifecycle record");
        assert!(
            decode_machine_lifecycle_store_record(&bytes).is_err(),
            "corrupt lifecycle record must fail closed: {value}"
        );
    }

    #[test]
    fn version_one_record_without_supervisor_authority_migrates_explicitly_to_unbound() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "record_version": LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Retired,
            "binding": {
                "agent_runtime_id": "rt:session:legacy-v1",
                "fence_token": 19,
                "runtime_generation": 4,
                "runtime_epoch_id": "epoch-legacy-v1"
            }
        }))
        .expect("serialize legacy v1 lifecycle record");

        let decoded = decode_machine_lifecycle_store_record(&bytes)
            .expect("valid v1 record without the additive field must decode");
        assert_eq!(decoded.runtime_state(), RuntimeState::Retired);
        assert_eq!(
            decoded.supervisor_authority(),
            &SupervisorAuthoritySnapshot::UnboundNoReceipt
        );
    }

    #[test]
    fn current_record_requires_supervisor_authority_and_unregister_progress_presence() {
        assert_decode_fails(serde_json::json!({
            "record_version": MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Idle,
            "binding": {
                "agent_runtime_id": null,
                "fence_token": null,
                "runtime_generation": null,
                "runtime_epoch_id": null
            },
            "unregister_progress": null
        }));
    }

    #[test]
    fn current_nullable_fields_require_presence_but_accept_explicit_null() {
        let unbound = snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt);
        let encoded = encoded_value(&unbound);
        assert_eq!(
            decode_machine_lifecycle_store_record(
                &serde_json::to_vec(&encoded).expect("serialize valid current record")
            )
            .expect("explicit-null current binding fields must decode"),
            unbound
        );
        let mut missing_progress = encoded.clone();
        missing_progress
            .as_object_mut()
            .expect("lifecycle record object")
            .remove("unregister_progress");
        assert_decode_fails(missing_progress);

        for field in [
            "agent_runtime_id",
            "fence_token",
            "runtime_generation",
            "runtime_epoch_id",
        ] {
            let mut partial = encoded.clone();
            partial["binding"]
                .as_object_mut()
                .expect("binding object")
                .remove(field);
            assert_decode_fails(partial);
        }

        let completed = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(101),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(30, "required-null-previous", 4),
            binding(31, "required-null-next", 5),
        )));
        let mut missing_rejection = encoded_value(&completed);
        assert!(missing_rejection["supervisor_authority"]["rotation"]["rejection"].is_null());
        missing_rejection["supervisor_authority"]["rotation"]
            .as_object_mut()
            .expect("rotation object")
            .remove("rejection");
        assert_decode_fails(missing_rejection);
    }

    #[test]
    fn version_two_supervisor_record_migrates_with_no_unregister_progress() {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "record_version": SUPERVISOR_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Retired,
            "binding": {
                "agent_runtime_id": "rt:session:legacy-v2",
                "fence_token": 23,
                "runtime_generation": 5,
                "runtime_epoch_id": "epoch-legacy-v2"
            },
            "supervisor_authority": { "kind": "unbound_no_receipt" }
        }))
        .expect("serialize v2 lifecycle record");

        let decoded = decode_machine_lifecycle_store_record(&bytes)
            .expect("valid v2 supervisor record must migrate");
        assert_eq!(decoded.runtime_state(), RuntimeState::Retired);
        assert_eq!(decoded.unregister_progress(), None);
    }

    #[test]
    fn current_unregister_progress_rejects_forced_disposition_before_feedback() {
        let mut value = encoded_value(&snapshot(SupervisorAuthoritySnapshot::UnboundNoReceipt));
        value["unregister_progress"] = serde_json::json!({
            "runtime_loop_drain_pending": true,
            "comms_drain_exit_pending": false,
            "completion_waiter_drain_pending": true,
            "runtime_loop_forced_abort": true,
            "comms_drain_forced_abort": false
        });
        assert_decode_fails(value);
    }

    #[test]
    fn version_one_migration_rejects_current_authority_fields() {
        assert_decode_fails(serde_json::json!({
            "record_version": LEGACY_MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
            "runtime_state": RuntimeState::Idle,
            "binding": {
                "agent_runtime_id": null,
                "fence_token": null,
                "runtime_generation": null,
                "runtime_epoch_id": null
            },
            "supervisor_authority": { "kind": "unbound_no_receipt" }
        }));
    }

    #[test]
    fn mixed_or_unknown_supervisor_authority_fields_fail_closed() {
        let current = binding(1, "current-supervisor", 7);
        let mut value = encoded_value(&snapshot(SupervisorAuthoritySnapshot::Bound(current)));
        value["supervisor_authority"]["rotation"] = serde_json::json!({});
        assert_decode_fails(value);
    }

    #[test]
    fn completed_rotation_operation_receipt_round_trips_for_cold_observation() {
        let snapshot = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(1),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(1, "previous-supervisor", 7),
            binding(2, "next-supervisor", 8),
        )));

        let encoded = encode_snapshot(&snapshot);
        let decoded = decode_machine_lifecycle_store_record(&encoded)
            .expect("decode completed rotation receipt");

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn exact_current_completed_adoption_round_trips_but_other_equal_epoch_completion_fails() {
        let current = binding(3, "already-rotated-supervisor", 9);
        let adoption = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(2),
            SupervisorRotationPersistencePhase::Completed,
            None,
            current.clone(),
            current,
        )));
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&adoption))
                .expect("exact-current legacy adoption receipt must decode"),
            adoption
        );

        let non_advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(3),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(3, "previous-supervisor", 9),
            binding(4, "different-supervisor", 9),
        )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&non_advancing))
                .is_err()
        );
    }

    #[test]
    fn malformed_rotation_descriptors_epochs_and_operation_ids_fail_closed() {
        let invalid_previous = SupervisorBindingReceipt::new(
            String::new(),
            "not-a-uuid".to_string(),
            "not-an-address".to_string(),
            "not-a-key".to_string(),
            1,
        );
        let invalid_previous_receipt =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(4),
                SupervisorRotationPersistencePhase::Rejected,
                Some(SupervisorRotationRejection::InvalidTarget),
                invalid_previous,
                binding(5, "raw-target", 2),
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &invalid_previous_receipt,
            ))
            .is_err()
        );

        let invalid_next = SupervisorBindingReceipt::new(
            "invalid-target".to_string(),
            "not-a-uuid".to_string(),
            "not-an-address".to_string(),
            "not-a-key".to_string(),
            2,
        );
        let invalid_completed_target =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(5),
                SupervisorRotationPersistencePhase::Completed,
                None,
                binding(6, "previous-supervisor", 1),
                invalid_next,
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &invalid_completed_target,
            ))
            .is_err()
        );

        let mut invalid_id = encoded_value(&snapshot(
            SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(6),
                SupervisorRotationPersistencePhase::PreviousRevokePending,
                None,
                binding(7, "previous-supervisor", 1),
                binding(8, "next-supervisor", 2),
            )),
        ));
        invalid_id["supervisor_authority"]["rotation"]["operation_id"] =
            serde_json::json!("not-a-uuid");
        assert_decode_fails(invalid_id);

        let nil_id = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(0),
            SupervisorRotationPersistencePhase::PreviousRevokePending,
            None,
            binding(7, "previous-supervisor", 1),
            binding(8, "next-supervisor", 2),
        )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&nil_id)).is_err()
        );

        let non_advancing_pending =
            snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(13),
                SupervisorRotationPersistencePhase::PreviousRevokePending,
                None,
                binding(7, "previous-supervisor", 4),
                binding(8, "next-supervisor", 4),
            )));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &non_advancing_pending,
            ))
            .is_err()
        );
    }

    #[test]
    fn rejected_invalid_or_unsupported_target_preserves_raw_evidence() {
        for (id, rejection) in [
            (7, SupervisorRotationRejection::InvalidTarget),
            (14, SupervisorRotationRejection::UnsupportedProtocolVersion),
        ] {
            let raw_invalid_target = SupervisorBindingReceipt::new(
                "".to_string(),
                "not-a-peer-id".to_string(),
                "not-an-address".to_string(),
                "not-a-signing-key".to_string(),
                0,
            );
            let snapshot = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(id),
                SupervisorRotationPersistencePhase::Rejected,
                Some(rejection),
                binding(9, "retained-supervisor", 11),
                raw_invalid_target,
            )));
            assert_eq!(
                decode_machine_lifecycle_store_record(&encode_snapshot(&snapshot))
                    .expect("rejected raw target evidence must remain durable"),
                snapshot
            );
        }
    }

    #[test]
    fn only_raw_target_rejections_are_durable_and_epoch_rejection_must_be_genuine() {
        for (id, rejection) in [
            (102, SupervisorRotationRejection::OperationConflict),
            (103, SupervisorRotationRejection::NotBound),
            (104, SupervisorRotationRejection::SenderMismatch),
        ] {
            let impossible = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
                operation_id(id),
                SupervisorRotationPersistencePhase::Rejected,
                Some(rejection),
                binding(32, "retained-supervisor", 7),
                binding(33, "requested-supervisor", 8),
            )));
            assert!(
                MachineLifecycleStoreRecord::from_snapshot(&impossible)
                    .encode()
                    .is_err()
            );
            assert!(
                decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&impossible))
                    .is_err()
            );
        }

        let advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(105),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(34, "retained-supervisor", 9),
            binding(35, "advancing-target", 10),
        )));
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&advancing)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&advancing))
                .is_err()
        );

        let non_advancing = snapshot(SupervisorAuthoritySnapshot::RotationOperation(rotation(
            operation_id(106),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(36, "retained-supervisor", 11),
            binding(37, "non-advancing-target", 11),
        )));
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&non_advancing))
                .expect("genuine target-epoch rejection must remain durable"),
            non_advancing
        );
    }

    #[test]
    fn malformed_current_authority_variants_fail_closed() {
        let malformed = SupervisorBindingReceipt::new(
            String::new(),
            "not-a-peer-id".to_string(),
            "not-an-address".to_string(),
            "not-a-signing-key".to_string(),
            1,
        );
        let bound = snapshot(SupervisorAuthoritySnapshot::Bound(malformed.clone()));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&bound)).is_err()
        );

        let pending = snapshot(SupervisorAuthoritySnapshot::RevocationPending(
            SupervisorRevocationPendingReceipt::new(
                malformed.name().to_owned(),
                malformed.peer_id().to_owned(),
                malformed.address().to_owned(),
                malformed.signing_public_key().to_owned(),
                malformed.epoch(),
            ),
        ));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&pending)).is_err()
        );

        let revoked = snapshot(SupervisorAuthoritySnapshot::RevokedReceipt(
            RevokedSupervisorReceipt::new(
                malformed.peer_id().to_owned(),
                malformed.signing_public_key().to_owned(),
                malformed.epoch(),
            ),
        ));
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&revoked)).is_err()
        );
    }

    #[test]
    fn partial_and_nonterminal_history_records_fail_closed() {
        let receipt = rotation(
            operation_id(8),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(10, "history-previous", 1),
            binding(11, "history-next", 2),
        );
        let history = std::collections::BTreeMap::from([(receipt.operation_id(), receipt)]);
        let snapshot = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                12,
                "current-supervisor",
                3,
            ))),
            terminal_receipts: history,
        });

        let mut partial = encoded_value(&snapshot);
        partial["supervisor_authority"]["terminal_receipts"][0]
            .as_object_mut()
            .expect("history receipt object")
            .remove("next");
        assert_decode_fails(partial);

        let mut nonterminal = encoded_value(&snapshot);
        nonterminal["supervisor_authority"]["terminal_receipts"][0]["phase"] =
            serde_json::json!("next_publish_pending");
        assert_decode_fails(nonterminal);
    }

    #[test]
    fn duplicate_nested_and_active_history_conflicts_fail_closed() {
        let history_receipt = rotation(
            operation_id(9),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(13, "history-previous", 1),
            binding(14, "history-next", 2),
        );
        let history = std::collections::BTreeMap::from([(
            history_receipt.operation_id(),
            history_receipt.clone(),
        )]);
        let wrapper = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                15,
                "current-supervisor",
                3,
            ))),
            terminal_receipts: history,
        });

        let mut duplicate = encoded_value(&wrapper);
        let receipt = duplicate["supervisor_authority"]["terminal_receipts"][0].clone();
        duplicate["supervisor_authority"]["terminal_receipts"]
            .as_array_mut()
            .expect("history receipt array")
            .push(receipt);
        assert_decode_fails(duplicate);

        let mut nested = encoded_value(&wrapper);
        let nested_current = nested["supervisor_authority"].clone();
        nested["supervisor_authority"]["current"] = nested_current;
        assert_decode_fails(nested);

        let active_conflict = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::RotationOperation(
                history_receipt.clone(),
            )),
            terminal_receipts: std::collections::BTreeMap::from([(
                history_receipt.operation_id(),
                history_receipt,
            )]),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&active_conflict)
                .encode()
                .is_err()
        );

        let empty_history = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                20,
                "current-supervisor",
                4,
            ))),
            terminal_receipts: std::collections::BTreeMap::new(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&empty_history)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&empty_history))
                .is_err()
        );

        let mismatched_key = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                21,
                "current-supervisor",
                4,
            ))),
            terminal_receipts: std::collections::BTreeMap::from([(
                operation_id(99),
                rotation(
                    operation_id(98),
                    SupervisorRotationPersistencePhase::Completed,
                    None,
                    binding(22, "history-previous", 2),
                    binding(23, "history-next", 3),
                ),
            )]),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&mismatched_key)
                .encode()
                .is_err()
        );
    }

    #[test]
    fn history_current_epoch_and_same_epoch_identity_must_cohere() {
        let previous = binding(38, "history-previous", 12);
        let next = binding(39, "history-next", 13);
        let completed = rotation(
            operation_id(107),
            SupervisorRotationPersistencePhase::Completed,
            None,
            previous.clone(),
            next.clone(),
        );
        let history =
            std::collections::BTreeMap::from([(completed.operation_id(), completed.clone())]);

        let stale_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                38,
                "refreshed-history-previous",
                12,
            ))),
            terminal_receipts: history.clone(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&stale_current)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(&stale_current))
                .is_err()
        );

        let conflicting_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                40,
                "conflicting-current",
                13,
            ))),
            terminal_receipts: history.clone(),
        });
        assert!(
            MachineLifecycleStoreRecord::from_snapshot(&conflicting_current)
                .encode()
                .is_err()
        );
        assert!(
            decode_machine_lifecycle_store_record(&encode_unvalidated_snapshot(
                &conflicting_current,
            ))
            .is_err()
        );

        let route_refreshed_current = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::Bound(binding(
                39,
                "route-refreshed-history-next",
                13,
            ))),
            terminal_receipts: history,
        });
        assert_eq!(
            decode_machine_lifecycle_store_record(&encode_snapshot(&route_refreshed_current))
                .expect("same identity may refresh route metadata within one epoch"),
            route_refreshed_current
        );
    }

    #[test]
    fn terminal_history_survives_later_rotation_and_recovery() {
        let first = rotation(
            operation_id(10),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(16, "first-supervisor", 1),
            binding(17, "second-supervisor", 2),
        );
        let rejected = rotation(
            operation_id(11),
            SupervisorRotationPersistencePhase::Rejected,
            Some(SupervisorRotationRejection::TargetEpochNotAdvanced),
            binding(17, "second-supervisor", 2),
            binding(18, "rejected-supervisor", 2),
        );
        let later = rotation(
            operation_id(12),
            SupervisorRotationPersistencePhase::Completed,
            None,
            binding(17, "second-supervisor", 2),
            binding(19, "current-supervisor", 3),
        );
        let snapshot = snapshot(SupervisorAuthoritySnapshot::WithRotationHistory {
            current: Box::new(SupervisorAuthoritySnapshot::RotationOperation(later)),
            terminal_receipts: std::collections::BTreeMap::from([
                (first.operation_id(), first),
                (rejected.operation_id(), rejected),
            ]),
        });

        let decoded = decode_machine_lifecycle_store_record(&encode_snapshot(&snapshot))
            .expect("later rotation and old terminal history must recover together");
        assert_eq!(decoded, snapshot);
    }
}
