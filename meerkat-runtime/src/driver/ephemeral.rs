//! EphemeralRuntimeDriver -- in-memory runtime driver for ephemeral sessions.
//!
//! Implements `RuntimeDriver` with:
//! - Input acceptance with validation, policy resolution, dedup, supersession
//! - Input lifecycle transitions driven directly through the MeerkatMachine
//!   DSL authority (`input_phases` + associated transitions); `InputState`
//!   carries only shell-mirror metadata (history, timestamps, cached terminal
//!   outcome, compatibility retry counter)
//! - InputQueue FIFO management
//! - S24 ephemeral recovery
//! - S25 retire/reset/destroy lifecycle operations

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock as StdRwLock};

use chrono::Utc;
#[cfg(test)]
use meerkat_core::SessionId;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::HandlingMode;

use crate::accept::{
    AcceptOutcome, AdmissionPlan, AdmissionQueueAction, CoarseAdmissionFlags,
    ExistingQueuedAdmissionAction, MachineAdmissionAuthority, RejectReason, ResolvedAdmission,
};
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId, PolicyVersion};
use crate::ingress_types::{
    ContentShape, RequestId, ReservationKey, RuntimeInputProjection, RuntimeInputSemantics,
};
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateHistoryEntry,
    InputStatePersistenceRecord, InputStateSeed, InputTerminalOutcome, PolicySnapshot,
    StoredInputState,
};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::policy::PolicyDecision;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_state::RuntimeState;
use crate::store::MachineLifecycleBindingFacts;
use crate::traits::{RecoveryReport, ResetReport, RetireReport, RuntimeDriverError};

/// Typed post-admission signal that the runtime loop should act on.
///
/// Replaces the boolean `wake_requested` / `process_requested` flags with
/// an ordered enum where each variant is strictly stronger than the previous.
/// The driver accumulates the maximum signal across ingress effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PostAdmissionSignal {
    /// No action needed.
    None,
    /// Wake the runtime loop to process queued work (idle → running).
    WakeLoop,
    /// Interrupt cooperative yielding points within an active turn.
    ///
    /// This is weaker than immediate processing but still stronger than a
    /// plain wake. The current ingress authority no longer emits this
    /// independently, but the runtime/control seam still uses the noun and the
    /// stronger `RequestImmediateProcessing` implies it.
    InterruptYielding,
    /// Request immediate steer/checkpoint processing within the current turn.
    /// Implies WakeLoop — strictly strongest.
    RequestImmediateProcessing,
}

impl PostAdmissionSignal {
    /// Whether the runtime loop should be woken.
    pub fn should_wake(self) -> bool {
        self >= Self::WakeLoop
    }

    /// Whether cooperative yield points should be interrupted.
    pub fn should_interrupt_yielding(self) -> bool {
        self >= Self::InterruptYielding
    }

    /// Whether immediate in-turn processing was requested.
    pub fn should_process_immediately(self) -> bool {
        self == Self::RequestImmediateProcessing
    }
}

/// Shared coarse runtime control projection owned by the checked-in
/// `MeerkatMachine` and borrowed by concrete driver shells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeControlProjection {
    pub(crate) phase: RuntimeState,
    pub(crate) current_run_id: Option<RunId>,
    pub(crate) pre_run_phase: Option<RuntimeState>,
}

impl Default for RuntimeControlProjection {
    fn default() -> Self {
        Self {
            phase: RuntimeState::Idle,
            current_run_id: None,
            pre_run_phase: None,
        }
    }
}

struct AdmissionValidationFacts<'a> {
    input_kind: crate::identifiers::InputKind,
    input_origin: &'a crate::input::InputOrigin,
    durability: crate::input::InputDurability,
    peer_handling_mode_valid: bool,
    peer_response_terminal_structurally_valid: bool,
    peer_response_terminal_observed_status: mm_dsl::PeerResponseTerminalObservedStatus,
}

#[derive(Debug, Clone, Default)]
pub struct ReplayQueuedContributorsPlan {
    pub queue_work_ids: Vec<InputId>,
    pub steer_work_ids: Vec<InputId>,
    pub notice_kind: &'static str,
}

#[derive(Clone)]
pub(crate) struct EphemeralDriverRollbackSnapshot {
    control_projection: RuntimeControlProjection,
    dsl_snapshot: mm_dsl::MeerkatMachineAuthoritySnapshot,
    ledger: InputLedger,
    queue: InputQueue,
    steer_queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    post_admission_signal: PostAdmissionSignal,
    handling_mode: HashMap<InputId, HandlingMode>,
    runtime_semantics: HashMap<InputId, RuntimeInputSemantics>,
    primitive_projection: HashMap<InputId, RuntimeInputProjection>,
    is_prompt_set: std::collections::HashSet<InputId>,
    content_shape: HashMap<InputId, ContentShape>,
    request_id: HashMap<InputId, Option<RequestId>>,
    reservation_key: HashMap<InputId, Option<ReservationKey>>,
    policy_snapshot: HashMap<InputId, PolicyDecision>,
    admission_order: Vec<InputId>,
}

/// Ephemeral runtime driver -- all state in-memory.
#[derive(Clone)]
pub struct EphemeralRuntimeDriver {
    runtime_id: LogicalRuntimeId,
    /// Shared coarse runtime projection owned by the machine/session entry.
    ///
    /// The concrete driver may read and realize this state, but it is not the
    /// semantic owner of the lifecycle tuple.
    control: Arc<StdRwLock<RuntimeControlProjection>>,
    ledger: InputLedger,
    queue: InputQueue,
    steer_queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    /// Typed post-admission signal replacing boolean wake/process flags.
    ///
    /// Accumulates the strongest signal across all ingress effects since last
    /// drain. `RequestImmediateProcessing` is strictly stronger than `WakeLoop`.
    post_admission_signal: PostAdmissionSignal,
    /// Shared session-owned DSL authority for ingress semantics (queue/steer
    /// lanes, input phases, admission ordering).
    dsl: DslAuthority,
    /// Per-input admission metadata with no DSL home (content shape,
    /// correlation IDs, policy snapshot, handling mode). These are pure
    /// shell mechanics — they feed observability and queue routing, never
    /// decide semantics.
    handling_mode: HashMap<InputId, HandlingMode>,
    runtime_semantics: HashMap<InputId, RuntimeInputSemantics>,
    primitive_projection: HashMap<InputId, RuntimeInputProjection>,
    is_prompt_set: std::collections::HashSet<InputId>,
    content_shape: HashMap<InputId, ContentShape>,
    request_id: HashMap<InputId, Option<RequestId>>,
    reservation_key: HashMap<InputId, Option<ReservationKey>>,
    policy_snapshot: HashMap<InputId, PolicyDecision>,
    /// Admission order preserved for observability. Retained in insertion
    /// order so snapshot readers (`MeerkatAdmittedInputSnapshot`) can render
    /// inputs deterministically.
    admission_order: Vec<InputId>,
}

/// Wrapper around the DSL authority that provides `Debug` output.
///
/// The generated `MeerkatMachineAuthority` does not derive `Debug`, but
/// `EphemeralRuntimeDriver` requires `Clone` which is satisfied via the
/// custom impl below.
pub(crate) type SharedIngressDslAuthority = Arc<Mutex<mm_dsl::MeerkatMachineAuthority>>;

struct DslAuthority(SharedIngressDslAuthority);

impl Clone for DslAuthority {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl DslAuthority {
    fn lock(&self) -> std::sync::MutexGuard<'_, mm_dsl::MeerkatMachineAuthority> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub(crate) fn new_ingress_dsl_authority() -> SharedIngressDslAuthority {
    Arc::new(Mutex::new(
        crate::meerkat_machine::dsl_authority::new_initialized_authority(
            "ingress DSL authority must initialize",
        ),
    ))
}

fn recover_ingress_dsl_authority(
    state: mm_dsl::MeerkatMachineState,
) -> mm_dsl::MeerkatMachineAuthority {
    crate::meerkat_machine::recover_projected_authority(
        state,
        "projected ingress DSL state must be recoverable",
    )
}

impl EphemeralRuntimeDriver {
    fn read_control_projection(&self) -> std::sync::RwLockReadGuard<'_, RuntimeControlProjection> {
        match self.control.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner()
            }
        }
    }

    fn write_control_projection(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, RuntimeControlProjection> {
        match self.control.write() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!("runtime control projection lock poisoned");
                poisoned.into_inner()
            }
        }
    }

    pub fn new(runtime_id: LogicalRuntimeId) -> Self {
        Self::new_with_control_and_dsl(
            runtime_id,
            Arc::new(StdRwLock::new(RuntimeControlProjection::default())),
            new_ingress_dsl_authority(),
        )
    }

    pub(crate) fn new_with_control(
        runtime_id: LogicalRuntimeId,
        control: Arc<StdRwLock<RuntimeControlProjection>>,
    ) -> Self {
        Self::new_with_control_and_dsl(runtime_id, control, new_ingress_dsl_authority())
    }

    pub(crate) fn new_with_control_and_dsl(
        runtime_id: LogicalRuntimeId,
        control: Arc<StdRwLock<RuntimeControlProjection>>,
        dsl: SharedIngressDslAuthority,
    ) -> Self {
        Self {
            runtime_id,
            control,
            ledger: InputLedger::new(),
            queue: InputQueue::new(),
            steer_queue: InputQueue::new(),
            events: Vec::new(),
            post_admission_signal: PostAdmissionSignal::None,
            dsl: DslAuthority(dsl),
            handling_mode: HashMap::new(),
            runtime_semantics: HashMap::new(),
            primitive_projection: HashMap::new(),
            is_prompt_set: std::collections::HashSet::new(),
            content_shape: HashMap::new(),
            request_id: HashMap::new(),
            reservation_key: HashMap::new(),
            policy_snapshot: HashMap::new(),
            admission_order: Vec::new(),
        }
    }

    pub(crate) fn rollback_snapshot(&self) -> EphemeralDriverRollbackSnapshot {
        EphemeralDriverRollbackSnapshot {
            control_projection: self.read_control_projection().clone(),
            dsl_snapshot: self.dsl.lock().snapshot(),
            ledger: self.ledger.clone(),
            queue: self.queue.clone(),
            steer_queue: self.steer_queue.clone(),
            events: self.events.clone(),
            post_admission_signal: self.post_admission_signal,
            handling_mode: self.handling_mode.clone(),
            runtime_semantics: self.runtime_semantics.clone(),
            primitive_projection: self.primitive_projection.clone(),
            is_prompt_set: self.is_prompt_set.clone(),
            content_shape: self.content_shape.clone(),
            request_id: self.request_id.clone(),
            reservation_key: self.reservation_key.clone(),
            policy_snapshot: self.policy_snapshot.clone(),
            admission_order: self.admission_order.clone(),
        }
    }

    pub(crate) fn clone_with_isolated_dsl_authority(&self) -> Self {
        let mut clone = self.clone();
        let dsl_state = self.with_dsl_state(Clone::clone);
        clone.dsl = DslAuthority(Arc::new(Mutex::new(recover_ingress_dsl_authority(
            dsl_state,
        ))));
        clone.control = Arc::new(StdRwLock::new(self.read_control_projection().clone()));
        clone
    }

    pub(crate) fn restore_rollback_snapshot(&mut self, snapshot: EphemeralDriverRollbackSnapshot) {
        {
            let mut control = self.write_control_projection();
            *control = snapshot.control_projection;
        }
        {
            let mut authority = self.dsl.lock();
            authority.restore_snapshot(snapshot.dsl_snapshot);
        }
        self.ledger = snapshot.ledger;
        self.queue = snapshot.queue;
        self.steer_queue = snapshot.steer_queue;
        self.events = snapshot.events;
        self.post_admission_signal = snapshot.post_admission_signal;
        self.handling_mode = snapshot.handling_mode;
        self.runtime_semantics = snapshot.runtime_semantics;
        self.primitive_projection = snapshot.primitive_projection;
        self.is_prompt_set = snapshot.is_prompt_set;
        self.content_shape = snapshot.content_shape;
        self.request_id = snapshot.request_id;
        self.reservation_key = snapshot.reservation_key;
        self.policy_snapshot = snapshot.policy_snapshot;
        self.admission_order = snapshot.admission_order;
    }

    pub(crate) fn shared_dsl_authority(&self) -> SharedIngressDslAuthority {
        Arc::clone(&self.dsl.0)
    }

    pub(crate) fn session_authority_id_for_recovery(&self) -> mm_dsl::SessionId {
        self.with_dsl_state(|state| state.session_id.clone())
            .unwrap_or_else(|| self.contract_session_authority_id())
    }

    pub(crate) fn machine_lifecycle_binding_facts(&self) -> MachineLifecycleBindingFacts {
        self.with_dsl_state(|state| {
            MachineLifecycleBindingFacts::new(
                state
                    .active_runtime_id
                    .as_ref()
                    .map(|value| value.0.clone()),
                state.active_fence_token.map(|token| token.0),
                state
                    .active_runtime_generation
                    .map(|generation| generation.0),
                state
                    .active_runtime_epoch_id
                    .as_ref()
                    .map(|value| value.0.clone()),
            )
        })
    }

    pub(crate) fn recover_runtime_authority_from_binding_observation(
        &mut self,
        session_id: mm_dsl::SessionId,
        runtime_phase: RuntimeState,
        runtime_id: Option<&LogicalRuntimeId>,
        active_fence_token: Option<u64>,
        active_runtime_generation: Option<mm_dsl::Generation>,
        active_runtime_epoch_id: Option<mm_dsl::RuntimeEpochId>,
    ) -> Result<(), RuntimeDriverError> {
        let silent_intent_overrides =
            self.with_dsl_state(|state| state.silent_intent_overrides.clone());
        self.recover_runtime_authority_from_binding_observation_with_silent_intents(
            session_id,
            runtime_phase,
            runtime_id,
            active_fence_token,
            active_runtime_generation,
            active_runtime_epoch_id,
            silent_intent_overrides,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn recover_runtime_authority_from_binding_observation_with_silent_intents(
        &mut self,
        session_id: mm_dsl::SessionId,
        runtime_phase: RuntimeState,
        runtime_id: Option<&LogicalRuntimeId>,
        active_fence_token: Option<u64>,
        active_runtime_generation: Option<mm_dsl::Generation>,
        active_runtime_epoch_id: Option<mm_dsl::RuntimeEpochId>,
        silent_intent_overrides: std::collections::BTreeSet<String>,
    ) -> Result<(), RuntimeDriverError> {
        let current_run_id = self.current_run_id();
        let pre_run_phase = self.pre_run_phase();
        let recovered =
            crate::meerkat_machine::dsl_authority::recover_authority_from_runtime_observation_id(
                session_id,
                runtime_phase,
                runtime_id,
                current_run_id.as_ref(),
                pre_run_phase,
                silent_intent_overrides,
                active_fence_token,
                active_runtime_generation,
                active_runtime_epoch_id,
            )
            .map_err(|err| {
                RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
                    err,
                    "persistent runtime recovery authority",
                ))
            })?;
        {
            let authority = self.shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *authority = recovered;
        }
        self.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    /// Apply a DSL input locally, mapping any rejection into a driver error.
    /// Used inside the ingress/input lifecycle paths that previously flowed
    /// through the deleted `RuntimeIngressAuthority` helper.
    ///
    /// Effects emitted by the transition are absorbed into the driver's
    /// machine-owned projections — notably `PostAdmissionSignal` is
    /// promoted into `self.post_admission_signal` so the runtime loop
    /// observes the wake/interrupt/immediate intent without a shell
    /// accumulator. Monotonic: only stronger signals overwrite.
    fn dsl_apply(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        self.dsl_apply_effects(input, context).map(|_| ())
    }

    fn dsl_apply_effects(
        &mut self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, RuntimeDriverError> {
        let transition = {
            let mut authority = self.dsl.lock();
            mm_dsl::MeerkatMachineMutator::apply(&mut *authority, input).map_err(|err| {
                RuntimeDriverError::Internal(format!("DSL rejected {context}: {err:?}"))
            })?
        };
        self.absorb_dsl_effects(transition.effects());
        Ok(transition.into_effects())
    }

    fn dsl_preview(
        &self,
        input: mm_dsl::MeerkatMachineInput,
        context: &str,
    ) -> Result<Vec<mm_dsl::MeerkatMachineEffect>, RuntimeDriverError> {
        let state = {
            let authority = self.dsl.lock();
            authority.state().clone()
        };
        let mut preview =
            mm_dsl::MeerkatMachineAuthority::recover_from_state(state).map_err(|err| {
                RuntimeDriverError::Internal(format!("DSL rejected {context}: {err:?}"))
            })?;
        mm_dsl::MeerkatMachineMutator::apply(&mut preview, input)
            .map(|transition| transition.into_effects())
            .map_err(|err| RuntimeDriverError::Internal(format!("DSL rejected {context}: {err:?}")))
    }

    /// Walk the effects emitted by a DSL transition and project any machine-
    /// owned signals into the driver's accumulated state. `PostAdmissionSignal`
    /// is the primary consumer today: it captures the typed admission signal
    /// the DSL decided (WakeLoop / InterruptYielding / RequestImmediateProcessing)
    /// so the runtime loop's `take_post_admission_signal` / `take_wake_requested`
    /// observe exactly what the machine authorized.
    fn absorb_dsl_effects(&mut self, effects: &[mm_dsl::MeerkatMachineEffect]) {
        for effect in effects {
            if let mm_dsl::MeerkatMachineEffect::PostAdmissionSignal { signal } = effect {
                let new_signal = match signal {
                    mm_dsl::PostAdmissionSignalKind::WakeLoop => PostAdmissionSignal::WakeLoop,
                    mm_dsl::PostAdmissionSignalKind::InterruptYielding => {
                        PostAdmissionSignal::InterruptYielding
                    }
                    mm_dsl::PostAdmissionSignalKind::RequestImmediateProcessing => {
                        PostAdmissionSignal::RequestImmediateProcessing
                    }
                };
                if new_signal > self.post_admission_signal {
                    self.post_admission_signal = new_signal;
                }
            }
        }
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[mm_dsl::MeerkatMachineEffect],
    ) {
        self.absorb_dsl_effects(effects);
    }

    fn dsl_key(input_id: &InputId) -> String {
        input_id.to_string()
    }

    fn with_dsl_state<R>(&self, body: impl FnOnce(&mm_dsl::MeerkatMachineState) -> R) -> R {
        let authority = self.dsl.lock();
        body(authority.state())
    }

    /// Read the current queue lane (FIFO) in admission order, as tracked by
    /// the driver-local DSL.
    fn dsl_queue_lane(&self) -> Vec<InputId> {
        self.lane_in_admission_order(mm_dsl::InputLane::Queue)
    }

    fn dsl_steer_lane(&self) -> Vec<InputId> {
        self.lane_in_admission_order(mm_dsl::InputLane::Steer)
    }

    fn lane_in_admission_order(&self, lane: mm_dsl::InputLane) -> Vec<InputId> {
        let mut candidates: Vec<(u64, InputId)> = self.with_dsl_state(|state| {
            self.admission_order
                .iter()
                .filter(|id| state.input_lane.get(&Self::dsl_key(id)).copied() == Some(lane))
                .cloned()
                .map(|id| {
                    let seq = state
                        .input_admission_seq
                        .get(&Self::dsl_key(&id))
                        .copied()
                        .unwrap_or(u64::MAX);
                    (seq, id)
                })
                .collect()
        });
        candidates.sort_by_key(|(seq, _)| *seq);
        candidates.into_iter().map(|(_, id)| id).collect()
    }

    /// Read the tracked lifecycle phase for an input from the DSL.
    ///
    /// Authoritative: the DSL is the sole writer of `input_phases`. Returns
    /// `None` if the DSL has never seen this input (e.g. pre-admission or
    /// post-GC).
    pub fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        let key = Self::dsl_key(input_id);
        let phase = self.with_dsl_state(|state| state.input_phases.get(&key).copied())?;
        Some(Self::input_phase_to_lifecycle(phase))
    }

    fn input_phase_required(
        &self,
        input_id: &InputId,
        context: &str,
    ) -> Result<InputLifecycleState, RuntimeDriverError> {
        self.input_phase(input_id).ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "generated input lifecycle phase missing {context} for input {input_id}"
            ))
        })
    }

    /// Project the DSL's typed [`mm_dsl::InputPhase`] onto the shell-side
    /// [`InputLifecycleState`]. The DSL never writes the pre-admission
    /// `Accepted` variant — admission always lands in `Queued` — so the
    /// projection is a total function over `InputPhase`.
    fn input_phase_to_lifecycle(phase: mm_dsl::InputPhase) -> InputLifecycleState {
        match phase {
            mm_dsl::InputPhase::Queued => InputLifecycleState::Queued,
            mm_dsl::InputPhase::Staged => InputLifecycleState::Staged,
            mm_dsl::InputPhase::Applied => InputLifecycleState::Applied,
            mm_dsl::InputPhase::AppliedPendingConsumption => {
                InputLifecycleState::AppliedPendingConsumption
            }
            mm_dsl::InputPhase::Consumed => InputLifecycleState::Consumed,
            mm_dsl::InputPhase::Superseded => InputLifecycleState::Superseded,
            mm_dsl::InputPhase::Coalesced => InputLifecycleState::Coalesced,
            mm_dsl::InputPhase::Abandoned => InputLifecycleState::Abandoned,
        }
    }

    /// Read the run association an input was last staged for, from the DSL.
    pub fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        let key = Self::dsl_key(input_id);
        let raw = self.with_dsl_state(|state| state.input_run_associations.get(&key).cloned())?;
        raw.parse::<uuid::Uuid>().ok().map(RunId::from_uuid)
    }

    /// Read the committed boundary sequence for an input, from the DSL.
    pub fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        let key = Self::dsl_key(input_id);
        self.with_dsl_state(|state| state.input_boundary_sequences.get(&key).copied())
    }

    /// Read the machine-owned per-run boundary counter for a run.
    ///
    /// This is the SINGLE producer of the run-boundary receipt sequence
    /// (dogma K10): live boundary-context checkpoints advance it inside the
    /// generated machine; runs without an entry are at the base sequence 0.
    /// The driver mints final `RunBoundaryReceipt`s from this value — shells
    /// and executors never fabricate it.
    pub fn run_boundary_sequence(&self, run_id: &RunId) -> u64 {
        let key = mm_dsl::RunId::from_domain(run_id);
        self.with_dsl_state(|state| {
            state
                .live_boundary_context_sequence_by_run
                .get(&key)
                .copied()
                .unwrap_or(0)
        })
    }

    /// Read the typed terminal outcome for an input, reconstructed from the
    /// DSL's typed terminal metadata maps.
    pub fn input_terminal_outcome(&self, input_id: &InputId) -> Option<InputTerminalOutcome> {
        let key = Self::dsl_key(input_id);
        let kind = self.with_dsl_state(|state| state.input_terminal_kind.get(&key).copied())?;
        match kind {
            mm_dsl::InputTerminalKind::Consumed => Some(InputTerminalOutcome::Consumed),
            mm_dsl::InputTerminalKind::Superseded => {
                let raw =
                    self.with_dsl_state(|state| state.input_superseded_by.get(&key).cloned())?;
                let id = raw.parse::<uuid::Uuid>().ok().map(InputId::from_uuid)?;
                Some(InputTerminalOutcome::Superseded { superseded_by: id })
            }
            mm_dsl::InputTerminalKind::Coalesced => {
                let raw =
                    self.with_dsl_state(|state| state.input_aggregate_id.get(&key).cloned())?;
                let id = raw.parse::<uuid::Uuid>().ok().map(InputId::from_uuid)?;
                Some(InputTerminalOutcome::Coalesced { aggregate_id: id })
            }
            mm_dsl::InputTerminalKind::Abandoned => {
                let reason = match self
                    .with_dsl_state(|state| state.input_abandon_reason.get(&key).copied())?
                {
                    mm_dsl::InputAbandonReason::Retired => InputAbandonReason::Retired,
                    mm_dsl::InputAbandonReason::Reset => InputAbandonReason::Reset,
                    mm_dsl::InputAbandonReason::Stopped => InputAbandonReason::Stopped,
                    mm_dsl::InputAbandonReason::Destroyed => InputAbandonReason::Destroyed,
                    mm_dsl::InputAbandonReason::Cancelled => InputAbandonReason::Cancelled,
                    mm_dsl::InputAbandonReason::MaxAttemptsExhausted => {
                        let attempts = self.with_dsl_state(|state| {
                            state
                                .input_abandon_attempt_count
                                .get(&key)
                                .copied()
                                .unwrap_or(0)
                        }) as u32;
                        InputAbandonReason::MaxAttemptsExhausted { attempts }
                    }
                };
                Some(InputTerminalOutcome::Abandoned { reason })
            }
        }
    }

    pub(crate) fn input_is_terminal_by_authority(
        &self,
        input_id: &InputId,
    ) -> Result<bool, RuntimeDriverError> {
        let Some(phase) = self.input_phase(input_id) else {
            return Err(RuntimeDriverError::Internal(format!(
                "missing generated input lifecycle authority for '{input_id}'"
            )));
        };
        crate::meerkat_machine::input_phase_behavioral_terminality_via_authority(
            input_id,
            phase,
            self.input_terminal_outcome(input_id),
        )
        .map_err(RuntimeDriverError::Internal)
    }

    fn input_is_non_terminal_by_authority(&self, input_id: &InputId) -> bool {
        match self.input_is_terminal_by_authority(input_id) {
            Ok(terminal) => !terminal,
            Err(err) => {
                tracing::error!(
                    input_id = %input_id,
                    error = %err,
                    "generated input terminality authority rejected non-terminal filter"
                );
                false
            }
        }
    }

    /// Read the attempt count for an input from the DSL.
    pub fn input_attempt_count(&self, input_id: &InputId) -> u32 {
        let key = Self::dsl_key(input_id);
        self.with_dsl_state(|state| state.input_attempt_counts.get(&key).copied().unwrap_or(0))
            as u32
    }

    /// Read the machine-owned recovery lane for an input from the DSL.
    pub fn input_recovery_lane(&self, input_id: &InputId) -> Option<HandlingMode> {
        let key = Self::dsl_key(input_id);
        let lane = self.with_dsl_state(|state| state.input_recovery_lanes.get(&key).copied())?;
        Some(Self::handling_mode_from_admission_lane(lane))
    }

    // ---- Admission metadata accessors (read-only) ----

    /// The admission order as minted by the DSL's `input_admission_seq`.
    pub fn admission_order(&self) -> Vec<InputId> {
        let mut candidates: Vec<(u64, usize, InputId)> = self.with_dsl_state(|state| {
            self.admission_order
                .iter()
                .enumerate()
                .map(|(index, id)| {
                    let seq = state
                        .input_admission_seq
                        .get(&Self::dsl_key(id))
                        .copied()
                        .unwrap_or(u64::MAX);
                    (seq, index, id.clone())
                })
                .collect()
        });
        candidates.sort_by_key(|(seq, index, _)| (*seq, *index));
        candidates.into_iter().map(|(_, _, id)| id).collect()
    }

    /// Policy snapshot captured at admission time for a specific input.
    pub fn admitted_policy(&self, input_id: &InputId) -> Option<&PolicyDecision> {
        self.policy_snapshot.get(input_id)
    }

    /// Content shape captured at admission time.
    pub fn admitted_content_shape(&self, input_id: &InputId) -> Option<ContentShape> {
        self.content_shape.get(input_id).copied()
    }

    /// Request ID captured at admission time.
    pub fn admitted_request_id(&self, input_id: &InputId) -> Option<RequestId> {
        self.request_id.get(input_id).cloned().flatten()
    }

    /// Reservation key captured at admission time.
    pub fn admitted_reservation_key(&self, input_id: &InputId) -> Option<ReservationKey> {
        self.reservation_key.get(input_id).cloned().flatten()
    }

    /// Handling mode decided at admission time.
    pub fn admitted_handling_mode(&self, input_id: &InputId) -> Option<HandlingMode> {
        self.handling_mode.get(input_id).copied()
    }

    /// Runtime-loop semantics decided at admission time.
    pub fn admitted_runtime_semantics(&self, input_id: &InputId) -> Option<RuntimeInputSemantics> {
        self.runtime_semantics.get(input_id).copied()
    }
    pub fn input_admission_sequence(&self, input_id: &InputId) -> Option<u64> {
        let key = Self::dsl_key(input_id);
        self.with_dsl_state(|state| state.input_admission_seq.get(&key).copied())
    }

    /// Conversation projection decided at admission time.
    pub fn admitted_primitive_projection(
        &self,
        input_id: &InputId,
    ) -> Option<RuntimeInputProjection> {
        self.primitive_projection.get(input_id).cloned()
    }

    /// Whether the input was classified as a prompt at admission.
    pub fn admitted_is_prompt(&self, input_id: &InputId) -> bool {
        self.is_prompt_set.contains(input_id)
    }

    /// Current DSL-tracked queue lane (FIFO in admission order).
    pub fn queue_lane(&self) -> Vec<InputId> {
        self.dsl_queue_lane()
    }

    /// Current DSL-tracked steer lane (FIFO in admission order).
    pub fn steer_lane(&self) -> Vec<InputId> {
        self.dsl_steer_lane()
    }

    /// DSL-tracked lifecycle phase for the given input, if known.
    pub fn ingress_lifecycle(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.input_phase(input_id)
    }

    pub(crate) fn control_handle(&self) -> Arc<StdRwLock<RuntimeControlProjection>> {
        self.control.clone()
    }

    fn control_snapshot(&self) -> RuntimeControlProjection {
        self.read_control_projection().clone()
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.with_dsl_state(|state| state.silent_intent_overrides.iter().cloned().collect())
    }

    fn matches_silent_intent_authority(&self, input: &Input) -> bool {
        self.with_dsl_state(|state| {
            let intents = state
                .silent_intent_overrides
                .iter()
                .cloned()
                .collect::<Vec<_>>();
            crate::silent_intent::matches_silent_intent(input, &intents)
        })
    }

    fn build_projection_queue(&self, ids: &[InputId], lane: &str) -> InputQueue {
        let mut queue = InputQueue::new();
        for input_id in ids {
            match self
                .ledger
                .get(input_id)
                .and_then(|state| state.persisted_input.clone())
            {
                Some(input) => queue.enqueue(input_id.clone(), input),
                None => {
                    tracing::error!(
                        input_id = ?input_id,
                        lane,
                        "ingress queue references input without persisted payload"
                    );
                    debug_assert!(
                        false,
                        "ingress queue projection missing persisted payload for {input_id:?} in {lane}"
                    );
                }
            }
        }
        queue
    }

    fn rebuild_queue_projections(&mut self) {
        let queue_ids = self.dsl_queue_lane();
        let steer_ids = self.dsl_steer_lane();
        self.queue = self.build_projection_queue(&queue_ids, "queue");
        self.steer_queue = self.build_projection_queue(&steer_ids, "steer_queue");
    }

    pub(crate) fn rebuild_queue_projections_after_recovery(&mut self) {
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
    }

    fn debug_assert_queue_projection_alignment(&self) {
        debug_assert_eq!(
            self.queue.input_ids(),
            self.dsl_queue_lane().as_slice(),
            "physical queue must match DSL queue lane"
        );
        debug_assert_eq!(
            self.steer_queue.input_ids(),
            self.dsl_steer_lane().as_slice(),
            "physical steer queue must match DSL steer lane"
        );
    }

    /// Admit a store-recovered input into the driver's ingress tracking.
    /// Called by the persistent driver during crash recovery to ensure the
    /// driver knows about inputs loaded from the store before `Recover` fires.
    ///
    /// Important: this first submits a recovered-admission witness to
    /// MeerkatMachine. Mechanical metadata is re-materialized only after that
    /// generated authority accepts the persisted kind/lane/semantics tuple.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn admit_recovered_to_ingress(
        &mut self,
        work_id: InputId,
        runtime_semantics: RuntimeInputSemantics,
        recovered_state: &InputState,
        recovered_seed: &InputStateSeed,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
        admission_sequence_recovery: Option<mm_dsl::RecoveredInputNormalizationReasonKind>,
    ) -> Result<(), RuntimeDriverError> {
        let persisted_input = recovered_state.persisted_input.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "store corruption: recovered input '{work_id}' has no persisted input; cannot validate recovered admission witness"
            ))
        })?;
        let input_kind = persisted_input.kind();
        let handling_mode = recovered_seed.recovery_lane.ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "store corruption: recovered input '{work_id}' missing generated recovery lane witness"
            ))
        })?;
        let content_shape = ContentShape::from_kind(input_kind);
        let primitive_projection = crate::input::runtime_input_projection(persisted_input);
        let is_prompt = matches!(persisted_input, Input::Prompt(_));

        self.apply_recovered_admission_witness(
            &work_id,
            input_kind,
            handling_mode,
            runtime_semantics,
        )?;
        self.apply_recovered_lifecycle(&work_id, recovered_seed, admission_sequence_recovery)?;
        self.register_accepted_idempotency(&work_id, recovered_state.idempotency_key.as_ref())?;
        self.record_admission_metadata(
            &work_id,
            &content_shape,
            handling_mode,
            runtime_semantics,
            primitive_projection,
            is_prompt,
            None,
            request_id.as_ref(),
            reservation_key.as_ref(),
        );
        Ok(())
    }

    fn apply_recovered_admission_witness(
        &mut self,
        work_id: &InputId,
        input_kind: crate::identifiers::InputKind,
        handling_mode: HandlingMode,
        runtime_semantics: RuntimeInputSemantics,
    ) -> Result<(), RuntimeDriverError> {
        let terminal_apply_intent = runtime_semantics
            .peer_response_terminal_apply_intent
            .map(mm_dsl::RecoveredPeerResponseTerminalApplyIntent::from);
        let runtime_boundary =
            mm_dsl::RecoveredRunApplyBoundary::try_from(runtime_semantics.boundary).map_err(
                |err| {
                    RuntimeDriverError::Internal(format!(
                        "store corruption: recovered input '{work_id}' has unsupported runtime boundary: {err}"
                    ))
                },
            )?;

        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RecoverAdmittedInput {
                input_id: Self::dsl_key(work_id),
                input_kind: mm_dsl::RecoveredInputKind::from(input_kind),
                runtime_boundary,
                runtime_execution_kind: mm_dsl::RecoveredRuntimeExecutionKind::from(
                    runtime_semantics.execution_kind,
                ),
                runtime_peer_response_terminal_apply_intent: terminal_apply_intent,
                lane: mm_dsl::InputLane::from(handling_mode),
            },
            "RecoverAdmittedInput",
        )
        .map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "store corruption: recovered input '{work_id}' rejected by generated recovered-admission authority: {err}"
            ))
        })
    }

    pub(crate) fn recover_terminal_input_lifecycle(
        &mut self,
        work_id: &InputId,
        recovered_seed: &InputStateSeed,
        idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<(), RuntimeDriverError> {
        let terminal = crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
            work_id,
            recovered_seed,
        )
        .map_err(RuntimeDriverError::Internal)?;
        if !terminal {
            return Err(RuntimeDriverError::Internal(format!(
                "terminal recovery path received non-terminal input '{work_id}'"
            )));
        }
        self.apply_recovered_lifecycle(work_id, recovered_seed, None)?;
        self.register_accepted_idempotency(work_id, idempotency_key)
    }

    fn lifecycle_to_input_phase(lifecycle: InputLifecycleState) -> mm_dsl::InputPhase {
        match lifecycle {
            // The DSL never represents the pre-admission `Accepted` state —
            // admission lands directly in `Queued` so recovery normalizes
            // both shell variants onto the same DSL slot.
            InputLifecycleState::Accepted | InputLifecycleState::Queued => {
                mm_dsl::InputPhase::Queued
            }
            InputLifecycleState::Staged => mm_dsl::InputPhase::Staged,
            InputLifecycleState::Applied => mm_dsl::InputPhase::Applied,
            InputLifecycleState::AppliedPendingConsumption => {
                mm_dsl::InputPhase::AppliedPendingConsumption
            }
            InputLifecycleState::Consumed => mm_dsl::InputPhase::Consumed,
            InputLifecycleState::Superseded => mm_dsl::InputPhase::Superseded,
            InputLifecycleState::Coalesced => mm_dsl::InputPhase::Coalesced,
            InputLifecycleState::Abandoned => mm_dsl::InputPhase::Abandoned,
        }
    }

    fn apply_recovered_lifecycle(
        &mut self,
        work_id: &InputId,
        recovered_seed: &InputStateSeed,
        admission_sequence_recovery: Option<mm_dsl::RecoveredInputNormalizationReasonKind>,
    ) -> Result<(), RuntimeDriverError> {
        let key = Self::dsl_key(work_id);
        let lifecycle_state = recovered_seed.phase;
        crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
            work_id,
            recovered_seed,
        )
        .map_err(RuntimeDriverError::Internal)?;
        let (terminal_kind, superseded_by, aggregate_id, abandon_reason, abandon_attempt_count) =
            match recovered_seed.terminal_outcome.clone() {
                Some(InputTerminalOutcome::Consumed) => (
                    Some(mm_dsl::InputTerminalKind::Consumed),
                    None,
                    None,
                    None,
                    0,
                ),
                Some(InputTerminalOutcome::Superseded { superseded_by }) => (
                    Some(mm_dsl::InputTerminalKind::Superseded),
                    Some(superseded_by.to_string()),
                    None,
                    None,
                    0,
                ),
                Some(InputTerminalOutcome::Coalesced { aggregate_id }) => (
                    Some(mm_dsl::InputTerminalKind::Coalesced),
                    None,
                    Some(aggregate_id.to_string()),
                    None,
                    0,
                ),
                Some(InputTerminalOutcome::Abandoned { reason }) => {
                    let abandon_attempt_count = match &reason {
                        InputAbandonReason::MaxAttemptsExhausted { attempts } => {
                            u64::from(*attempts)
                        }
                        _ => u64::from(recovered_seed.attempt_count),
                    };
                    (
                        Some(mm_dsl::InputTerminalKind::Abandoned),
                        None,
                        None,
                        Some(mm_dsl::InputAbandonReason::from(&reason)),
                        abandon_attempt_count,
                    )
                }
                None => (None, None, None, None, 0),
            };
        let recovery_lane = recovered_seed.recovery_lane.map(mm_dsl::InputLane::from);
        let lane = matches!(lifecycle_state, InputLifecycleState::Queued)
            .then_some(recovery_lane)
            .flatten();
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RecoverInputLifecycle {
                input_id: key,
                phase: Self::lifecycle_to_input_phase(lifecycle_state),
                terminal_kind,
                superseded_by,
                aggregate_id,
                abandon_reason,
                abandon_attempt_count,
                attempt_count: u64::from(recovered_seed.attempt_count),
                run_id: recovered_seed
                    .last_run_id
                    .as_ref()
                    .map(std::string::ToString::to_string),
                boundary_sequence: recovered_seed.last_boundary_sequence,
                admission_sequence: recovered_seed.admission_sequence,
                admission_sequence_recovery,
                recovery_lane,
                lane,
            },
            "RecoverInputLifecycle",
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn record_admission_metadata(
        &mut self,
        work_id: &InputId,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        runtime_semantics: RuntimeInputSemantics,
        primitive_projection: RuntimeInputProjection,
        is_prompt: bool,
        policy: Option<&PolicyDecision>,
        request_id: Option<&RequestId>,
        reservation_key: Option<&ReservationKey>,
    ) {
        if !self.admission_order.contains(work_id) {
            self.admission_order.push(work_id.clone());
        }
        self.content_shape.insert(work_id.clone(), *content_shape);
        self.handling_mode.insert(work_id.clone(), handling_mode);
        self.runtime_semantics
            .insert(work_id.clone(), runtime_semantics);
        if let Some(state) = self.ledger.get_mut(work_id) {
            state.runtime_semantics = Some(runtime_semantics);
        }
        self.primitive_projection
            .insert(work_id.clone(), primitive_projection);
        if is_prompt {
            self.is_prompt_set.insert(work_id.clone());
        } else {
            self.is_prompt_set.remove(work_id);
        }
        self.request_id.insert(work_id.clone(), request_id.cloned());
        self.reservation_key
            .insert(work_id.clone(), reservation_key.cloned());
        if let Some(policy) = policy {
            self.policy_snapshot.insert(work_id.clone(), policy.clone());
        } else {
            self.policy_snapshot.remove(work_id);
        }
    }

    fn sync_terminal_projection_from_machine(
        &mut self,
        input_id: &InputId,
        from_phase: InputLifecycleState,
        expected_phase: InputLifecycleState,
        reason: &'static str,
    ) -> Result<(), RuntimeDriverError> {
        let phase = self.input_phase(input_id).ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "machine terminal projection missing input phase for {input_id}"
            ))
        })?;
        if phase != expected_phase {
            return Err(RuntimeDriverError::Internal(format!(
                "machine terminal projection for {input_id} was {phase:?}, expected {expected_phase:?}"
            )));
        }

        let terminal_outcome = self.input_terminal_outcome(input_id).ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "machine terminal projection missing terminal outcome for {input_id}"
            ))
        })?;
        let terminal_matches_phase = matches!(
            (&phase, &terminal_outcome),
            (
                InputLifecycleState::Superseded,
                InputTerminalOutcome::Superseded { .. }
            ) | (
                InputLifecycleState::Coalesced,
                InputTerminalOutcome::Coalesced { .. }
            ) | (
                InputLifecycleState::Consumed,
                InputTerminalOutcome::Consumed
            ) | (
                InputLifecycleState::Abandoned,
                InputTerminalOutcome::Abandoned { .. }
            )
        );
        if !terminal_matches_phase {
            return Err(RuntimeDriverError::Internal(format!(
                "machine terminal projection for {input_id} had incoherent outcome {terminal_outcome:?}"
            )));
        }

        let Some(state) = self.ledger.get_mut(input_id) else {
            return Err(RuntimeDriverError::Internal(format!(
                "machine terminal projection missing ledger row for {input_id}"
            )));
        };
        let now = Utc::now();
        state.history.push(InputStateHistoryEntry {
            timestamp: now,
            from: from_phase,
            to: phase,
            reason: Some(reason.into()),
        });
        state.updated_at = now;
        Ok(())
    }

    /// Apply the already-decided "persist and queue" admission plan.
    ///
    /// Mirrors the deleted `RuntimeIngressEffect::PersistAndQueue` +
    /// `EnqueueTo/Front` + `Coalesce/Supersede` sequence — each step runs
    /// inline here, directly against the DSL and per-input state, without
    /// going through a helper authority.
    #[allow(clippy::too_many_arguments)]
    fn apply_persist_and_queue(
        &mut self,
        input_id: &InputId,
        input: &Input,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        runtime_semantics: RuntimeInputSemantics,
        primitive_projection: RuntimeInputProjection,
        is_prompt: bool,
        policy: &PolicyDecision,
        mut state: InputState,
        queue_action: AdmissionQueueAction,
        existing_action: Option<&ExistingQueuedAdmissionAction>,
    ) -> Result<(), RuntimeDriverError> {
        // 1. DSL phase transition (authoritative). Admission lane mirrors
        //    the resolved handling_mode; the DSL owns lane membership from
        //    first touch.
        let admission_lane = mm_dsl::InputLane::from(handling_mode);
        let admission_key = Self::dsl_key(input_id);
        let (admission_input, admission_label) = match admission_lane {
            mm_dsl::InputLane::Queue => (
                mm_dsl::MeerkatMachineInput::QueueAccepted {
                    input_id: admission_key,
                },
                "QueueAccepted",
            ),
            mm_dsl::InputLane::Steer => (
                mm_dsl::MeerkatMachineInput::SteerAccepted {
                    input_id: admission_key,
                },
                "SteerAccepted",
            ),
        };
        self.dsl_apply(admission_input, admission_label)?;

        // 2. Handle supersession / coalescing of an existing queued input.
        //    The new input goes into the queue after generated authority has
        //    accepted every side effect; the existing one
        //    transitions to its terminal state here. The DSL transitions
        //    own lane removal.
        if let Some(action) = existing_action {
            match action {
                ExistingQueuedAdmissionAction::Coalesce { existing_id } => {
                    let existing_key = Self::dsl_key(existing_id);
                    let aggregate_key = Self::dsl_key(input_id);
                    let from_phase = self.input_phase_required(existing_id, "before coalescing")?;
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::CoalesceInput {
                            input_id: existing_key,
                            aggregate_id: aggregate_key,
                        },
                        "CoalesceInput",
                    )?;
                    self.sync_terminal_projection_from_machine(
                        existing_id,
                        from_phase,
                        InputLifecycleState::Coalesced,
                        "Coalesce",
                    )?;
                    let _ = self.queue.remove(existing_id);
                    let _ = self.steer_queue.remove(existing_id);
                }
                ExistingQueuedAdmissionAction::Supersede { existing_id } => {
                    let existing_key = Self::dsl_key(existing_id);
                    let superseded_by = Self::dsl_key(input_id);
                    let from_phase =
                        self.input_phase_required(existing_id, "before superseding")?;
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::SupersedeInput {
                            input_id: existing_key,
                            superseded_by,
                        },
                        "SupersedeInput",
                    )?;
                    self.sync_terminal_projection_from_machine(
                        existing_id,
                        from_phase,
                        InputLifecycleState::Superseded,
                        "Supersede",
                    )?;
                    let _ = self.queue.remove(existing_id);
                    let _ = self.steer_queue.remove(existing_id);
                }
            }
        }

        // 3. Apply generated queue ordering/reroute facts. When the
        //    queue_action's target differs from the admission lane (e.g.
        //    priority reroute), the shell emits a `ChangeLane` transition
        //    rather than writing `input_lane` directly.
        match queue_action.clone() {
            AdmissionQueueAction::None => {}
            AdmissionQueueAction::EnqueueTo { target } => {
                let target_lane = mm_dsl::InputLane::from(target);
                if target_lane != admission_lane {
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::ChangeLane {
                            input_id: Self::dsl_key(input_id),
                            new_lane: target_lane,
                        },
                        "ChangeLane",
                    )?;
                }
            }
            AdmissionQueueAction::EnqueueFront { target } => {
                let key = Self::dsl_key(input_id);
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::PrioritizeInput {
                        input_id: key.clone(),
                    },
                    "PrioritizeInput",
                )?;
                let target_lane = mm_dsl::InputLane::from(target);
                if target_lane != admission_lane {
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::ChangeLane {
                            input_id: key,
                            new_lane: target_lane,
                        },
                        "ChangeLane",
                    )?;
                }
            }
        }

        self.register_accepted_idempotency(input_id, input.header().idempotency_key.as_ref())?;

        let now = Utc::now();
        state.persisted_input = Some(input.clone());
        state.policy = Some(PolicySnapshot {
            version: policy.policy_version,
            decision: policy.clone(),
        });
        state.history.push(InputStateHistoryEntry {
            timestamp: now,
            from: InputLifecycleState::Accepted,
            to: InputLifecycleState::Queued,
            reason: Some("QueueAccepted".into()),
        });
        state.updated_at = now;
        self.ledger.accept(state);
        self.record_admission_metadata(
            input_id,
            content_shape,
            handling_mode,
            runtime_semantics,
            primitive_projection,
            is_prompt,
            Some(policy),
            None,
            None,
        );

        match queue_action {
            AdmissionQueueAction::None => {}
            AdmissionQueueAction::EnqueueTo { target } => match target {
                HandlingMode::Queue => self.queue.enqueue(input_id.clone(), input.clone()),
                HandlingMode::Steer => {
                    self.steer_queue.enqueue(input_id.clone(), input.clone());
                }
            },
            AdmissionQueueAction::EnqueueFront { target } => match target {
                HandlingMode::Queue => {
                    self.queue.enqueue_front(input_id.clone(), input.clone());
                }
                HandlingMode::Steer => {
                    self.steer_queue
                        .enqueue_front(input_id.clone(), input.clone());
                }
            },
        }

        self.emit_event(RuntimeEvent::InputLifecycle(
            InputLifecycleEvent::Accepted {
                input_id: input_id.clone(),
            },
        ));
        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
            input_id: input_id.clone(),
        }));

        Ok(())
    }

    pub fn is_idle(&self) -> bool {
        self.runtime_phase_snapshot() == RuntimeState::Idle
    }

    pub fn phase(&self) -> RuntimeState {
        self.runtime_phase_snapshot()
    }

    fn runtime_phase_snapshot(&self) -> RuntimeState {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority)
    }

    pub fn current_run_id(&self) -> Option<RunId> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority)
    }

    pub fn pre_run_phase(&self) -> Option<RuntimeState> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority)
    }

    fn contract_session_authority_id(&self) -> mm_dsl::SessionId {
        mm_dsl::SessionId::from(self.runtime_id.to_string())
    }

    pub(crate) fn ensure_contract_session_authority(
        &mut self,
    ) -> Result<mm_dsl::SessionId, RuntimeDriverError> {
        let existing = {
            let authority = self.shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            authority.state().session_id.clone()
        };
        if let Some(session_id) = existing {
            return Ok(session_id);
        }

        let session_id = self.contract_session_authority_id();
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RegisterSession {
                session_id: session_id.clone(),
            },
            "ContractRegisterSession",
        )?;
        self.sync_control_projection_from_dsl_authority();
        Ok(session_id)
    }

    /// Contract helper for external tests that need to start a run through the
    /// same DSL authority used by the runtime loop.
    #[doc(hidden)]
    pub fn contract_begin_run_authority(
        &mut self,
        run_id: RunId,
    ) -> Result<(), RuntimeDriverError> {
        let from = self.runtime_phase_snapshot();
        if from == RuntimeState::Running && self.current_run_id().as_ref() == Some(&run_id) {
            return Ok(());
        }

        let session_id = self.ensure_contract_session_authority()?;
        if from == RuntimeState::Retired {
            let authority = self.shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            authority
                .apply_signal(mm_dsl::MeerkatMachineSignal::DrainQueuedRun {
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                })
                .map(|_| ())
                .map_err(|err| {
                    RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
                        err,
                        "ContractDrainQueuedRun",
                    ))
                })?;
        } else {
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::Prepare {
                    session_id,
                    run_id: mm_dsl::RunId::from_domain(&run_id),
                },
                "ContractPrepareRun",
            )?;
        }
        self.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    fn set_phase(&mut self, next_phase: RuntimeState) -> RuntimeState {
        let mut control = self.write_control_projection();
        let from_phase = control.phase;
        control.phase = next_phase;
        from_phase
    }

    fn transition_phase(&mut self, next_phase: RuntimeState) {
        let from_phase = self.set_phase(next_phase);
        self.emit_event(RuntimeEvent::RuntimeStateChange(RuntimeStateChangeEvent {
            from: from_phase,
            to: next_phase,
        }));
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        if self.control_snapshot().phase == next_phase {
            self.write_control_projection().phase = next_phase;
        } else {
            self.transition_phase(next_phase);
        }
        let mut control = self.write_control_projection();
        control.current_run_id = current_run_id;
        control.pre_run_phase = pre_run_phase;
    }

    pub(crate) fn sync_control_projection_from_dsl_authority(&mut self) {
        let (phase, current_run_id, pre_run_phase) = {
            let authority = self.shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority),
                crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority),
                crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority),
            )
        };
        self.set_control_projection(phase, current_run_id, pre_run_phase);
    }

    /// Contract-only authority override for tests that need to seed impossible
    /// or already-realized runtime phases. Production recovery must replay
    /// durable lifecycle facts through DSL inputs instead of calling this.
    #[cfg(test)]
    #[doc(hidden)]
    pub(crate) fn contract_force_runtime_authority(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        {
            let authority = self.shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let session_id = authority
                .state()
                .session_id
                .as_ref()
                .and_then(|session_id| {
                    uuid::Uuid::parse_str(&session_id.0)
                        .ok()
                        .map(SessionId::from_uuid)
                });
            let silent_intent_overrides = authority.state().silent_intent_overrides.clone();
            let active_fence_token = authority
                .state()
                .active_fence_token
                .as_ref()
                .map(|token| token.0);
            let active_runtime_generation = authority.state().active_runtime_generation;
            let active_runtime_epoch_id = authority.state().active_runtime_epoch_id.clone();
            *authority =
                crate::meerkat_machine::dsl_authority::recover_authority_from_runtime_observation(
                    &session_id.unwrap_or_default(),
                    next_phase,
                    Some(&self.runtime_id),
                    current_run_id.as_ref(),
                    pre_run_phase,
                    silent_intent_overrides,
                    active_fence_token,
                    active_runtime_generation,
                    active_runtime_epoch_id,
                )
                .expect("contract runtime authority observation must recover");
        }
        self.set_control_projection(next_phase, current_run_id, pre_run_phase);
    }

    pub(crate) fn apply_runtime_executor_exited_authority(
        &mut self,
    ) -> Result<(), RuntimeDriverError> {
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RuntimeExecutorExited,
            "RuntimeExecutorExited",
        )
    }

    /// Drain and return the accumulated post-admission signal.
    ///
    /// Returns the strongest signal seen since the last drain and resets to `None`.
    pub fn take_post_admission_signal(&mut self) -> PostAdmissionSignal {
        std::mem::replace(&mut self.post_admission_signal, PostAdmissionSignal::None)
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub fn post_admission_signal(&self) -> PostAdmissionSignal {
        self.post_admission_signal
    }

    /// Drain the typed signal and return whether wake is needed (backward-compat).
    ///
    /// **Deprecated**: prefer `take_post_admission_signal()` for typed semantics.
    /// This drains the signal and returns `should_wake()`.
    pub fn take_wake_requested(&mut self) -> bool {
        // Note: we DON'T drain here — take_process_requested is always
        // called immediately after and expects to see the same signal.
        self.post_admission_signal.should_wake()
    }

    /// Return whether immediate processing was requested and drain (backward-compat).
    ///
    /// **Deprecated**: prefer `take_post_admission_signal()` for typed semantics.
    /// Must be called after `take_wake_requested()`. Drains the signal.
    pub fn take_process_requested(&mut self) -> bool {
        let signal = std::mem::replace(&mut self.post_admission_signal, PostAdmissionSignal::None);
        signal.should_process_immediately()
    }
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        std::mem::take(&mut self.events)
    }
    pub fn queue(&self) -> &InputQueue {
        &self.queue
    }
    pub fn steer_queue(&self) -> &InputQueue {
        &self.steer_queue
    }

    #[cfg(test)]
    pub fn queue_mut(&mut self) -> &mut InputQueue {
        &mut self.queue
    }

    #[cfg(test)]
    pub fn steer_queue_mut(&mut self) -> &mut InputQueue {
        &mut self.steer_queue
    }

    #[cfg(test)]
    pub(crate) fn clear_admitted_runtime_semantics_for_test(&mut self, input_id: &InputId) {
        self.runtime_semantics.remove(input_id);
        if let Some(state) = self.ledger.get_mut(input_id) {
            state.runtime_semantics = None;
        }
    }

    pub fn has_queued_input(&self, input_id: &InputId) -> bool {
        let key = Self::dsl_key(input_id);
        self.with_dsl_state(|state| state.input_lane.contains_key(&key))
    }
    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        let excluded_keys: std::collections::HashSet<String> =
            excluded.iter().map(Self::dsl_key).collect();
        self.with_dsl_state(|state| {
            state
                .input_lane
                .keys()
                .any(|queued_key| !excluded_keys.contains(queued_key))
        })
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::DeferInputBehindBacklog {
                    input_id: Self::dsl_key(input_id),
                },
                "DeferInputBehindBacklog",
            )?;
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(())
    }

    fn existing_superseded_input(
        &self,
        input: &Input,
    ) -> Option<(InputId, crate::coalescing::CoalescingResult)> {
        let candidates: Vec<InputId> = self
            .dsl_queue_lane()
            .into_iter()
            .chain(self.dsl_steer_lane())
            .collect();
        candidates.into_iter().find_map(|queued_id| {
            let existing = self.ledger.get(&queued_id)?.persisted_input.as_ref()?;
            let result = crate::coalescing::check_supersession(input, existing, &self.runtime_id);
            match result {
                crate::coalescing::CoalescingResult::Supersedes { .. } => Some((queued_id, result)),
                crate::coalescing::CoalescingResult::Standalone => None,
            }
        })
    }
    pub fn ledger(&self) -> &InputLedger {
        &self.ledger
    }
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }
    pub(crate) fn ledger_mut(&mut self) -> &mut InputLedger {
        &mut self.ledger
    }
    /// Build a `StoredInputState` bundle for a specific input, pairing the
    /// ledger-side shell with the DSL-owned seed (phase / run association /
    /// boundary sequence / recovery lane). Used by persistence callsites and
    /// test helpers.
    pub fn stored_input_state(&self, input_id: &InputId) -> Option<StoredInputState> {
        let mut state = self.ledger.get(input_id)?.clone();
        if state.runtime_semantics.is_none() {
            state.runtime_semantics = self.admitted_runtime_semantics(input_id);
        }
        let phase = self.input_phase(input_id)?;
        let seed = InputStateSeed {
            phase,
            last_run_id: self.input_last_run_id(input_id),
            last_boundary_sequence: self.input_last_boundary_sequence(input_id),
            admission_sequence: self.input_admission_sequence(input_id),
            terminal_outcome: self.input_terminal_outcome(input_id),
            attempt_count: self.input_attempt_count(input_id),
            recovery_lane: self.input_recovery_lane(input_id),
        };
        Some(StoredInputState { state, seed })
    }

    /// Snapshot of every ledger entry paired with its DSL seed.
    pub fn stored_input_states_snapshot(
        &self,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        self.ledger
            .iter()
            .map(|(input_id, state)| {
                let mut state = state.clone();
                if state.runtime_semantics.is_none() {
                    state.runtime_semantics = self.admitted_runtime_semantics(input_id);
                }
                let phase = self.input_phase(input_id).ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "generated input lifecycle phase missing for persisted input {input_id}"
                    ))
                })?;
                let seed = InputStateSeed {
                    phase,
                    last_run_id: self.input_last_run_id(input_id),
                    last_boundary_sequence: self.input_last_boundary_sequence(input_id),
                    admission_sequence: self.input_admission_sequence(input_id),
                    terminal_outcome: self.input_terminal_outcome(input_id),
                    attempt_count: self.input_attempt_count(input_id),
                    recovery_lane: self.input_recovery_lane(input_id),
                };
                Ok(StoredInputState { state, seed })
            })
            .collect()
    }

    /// Snapshot of every ledger entry paired with generated persistence
    /// authority for the DSL-owned seed facts.
    pub fn authorized_stored_input_states_snapshot(
        &self,
    ) -> Result<Vec<InputStatePersistenceRecord>, RuntimeDriverError> {
        self.stored_input_states_snapshot()?
            .into_iter()
            .map(|bundle| {
                InputStatePersistenceRecord::from_machine_snapshot(bundle)
                    .map_err(RuntimeDriverError::Internal)
            })
            .collect()
    }

    /// Store-write record for one input's generated seed snapshot.
    pub fn authorized_stored_input_state(
        &self,
        input_id: &InputId,
    ) -> Result<Option<InputStatePersistenceRecord>, RuntimeDriverError> {
        self.stored_input_state(input_id)
            .map(InputStatePersistenceRecord::from_machine_snapshot)
            .transpose()
            .map_err(RuntimeDriverError::Internal)
    }

    /// Replay a recovered store bundle through generated recovery authority and
    /// return the machine-owned persistence snapshot. This is for migration and
    /// recovery tests that must seed a store before a persistent driver exists;
    /// direct store writes still cannot mint records from raw seed facts.
    pub fn recover_input_state_persistence_record(
        &mut self,
        mut bundle: StoredInputState,
    ) -> Result<InputStatePersistenceRecord, RuntimeDriverError> {
        let delta = crate::meerkat_machine::driver::machine_apply_recovered_input_normalization(
            &mut bundle,
            None,
        )?;
        let input_id = bundle.state.input_id.clone();
        if self.ledger.get(&input_id).is_some() {
            return Err(RuntimeDriverError::Internal(format!(
                "input-state persistence recovery record requested for duplicate input {input_id}"
            )));
        }

        let terminal = crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
            &input_id,
            &bundle.seed,
        )
        .map_err(RuntimeDriverError::Internal)?;

        if terminal {
            self.recover_terminal_input_lifecycle(
                &input_id,
                &bundle.seed,
                bundle.state.idempotency_key.as_ref(),
            )?;
        } else {
            let Some(entry) = crate::meerkat_machine::driver::machine_build_recovered_ingress_entry(
                &bundle.state,
                &bundle.seed,
            ) else {
                return Err(RuntimeDriverError::Internal(format!(
                    "input-state persistence recovery record for '{input_id}' missing recovered admission witness"
                )));
            };
            self.admit_recovered_to_ingress(
                input_id.clone(),
                entry.runtime_semantics,
                &bundle.state,
                &bundle.seed,
                None,
                None,
                delta.admission_sequence_recovery,
            )?;
        }

        self.ledger.recover(bundle.state);
        self.rebuild_queue_projections_after_recovery();
        self.authorized_stored_input_state(&input_id)?
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "generated input-state persistence recovery emitted no record for {input_id}"
                ))
            })
    }
    /// Clear the physical queue projections without touching canonical ingress
    /// truth. Used by recovery contract tests to simulate projection loss.
    pub fn clear_queue_projections(&mut self) {
        self.queue = InputQueue::new();
        self.steer_queue = InputQueue::new();
    }
    pub fn dequeue_next(&mut self) -> Option<(InputId, Input)> {
        let queued = self
            .steer_queue
            .dequeue()
            .or_else(|| self.queue.dequeue())?;
        Some((queued.input_id, queued.input))
    }

    /// Dequeue a specific input by ID from whichever queue contains it.
    pub fn dequeue_by_id(&mut self, input_id: &InputId) -> Option<(InputId, Input)> {
        self.steer_queue
            .dequeue_by_id(input_id)
            .or_else(|| self.queue.dequeue_by_id(input_id))
    }

    pub fn stage_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        self.stage_batch(std::slice::from_ref(input_id), run_id)
    }

    /// Stage a batch of inputs via DSL `StageForRun` transitions.
    pub fn stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_stage_batch(input_ids, run_id)
    }

    /// Machine-owned realization for a validated staged contributor batch.
    pub(crate) fn machine_realize_stage_batch(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            let key = Self::dsl_key(input_id);
            // Snapshot the pre-transition phase off the DSL for history
            // bookkeeping before the StageForRun apply flips it to Staged.
            let from_phase = self.input_phase_required(input_id, "before staging")?;
            // `StageForRun` is the sole writer of `input_lane` on stage.
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::StageForRun {
                    input_id: key,
                    run_id: run_id.to_string(),
                },
                "StageForRun",
            )?;
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::IncrementAttemptCount {
                    input_id: Self::dsl_key(input_id),
                },
                "IncrementAttemptCount",
            )?;

            let now = Utc::now();
            if let Some(state) = self.ledger.get_mut(input_id) {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: from_phase,
                    to: InputLifecycleState::Staged,
                    reason: Some(format!("StageForRun({run_id})")),
                });
                state.updated_at = now;
            }
            self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Staged {
                input_id: input_id.clone(),
                run_id: run_id.clone(),
            }));
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        Ok(())
    }

    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        let key = Self::dsl_key(input_id);
        // Snapshot the phase the input is coming from (typically Staged) off
        // the DSL before MarkApplied flips it to Applied.
        let from_phase = self.input_phase_required(input_id, "before applying")?;
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::MarkApplied {
                input_id: key.clone(),
            },
            "MarkApplied",
        )?;
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::MarkAppliedPendingConsumption { input_id: key },
            "MarkAppliedPendingConsumption",
        )?;

        let now = Utc::now();
        if let Some(state) = self.ledger.get_mut(input_id) {
            state.history.push(InputStateHistoryEntry {
                timestamp: now,
                from: from_phase,
                to: InputLifecycleState::Applied,
                reason: Some(format!("MarkApplied({run_id})")),
            });
            state.history.push(InputStateHistoryEntry {
                timestamp: now,
                from: InputLifecycleState::Applied,
                to: InputLifecycleState::AppliedPendingConsumption,
                reason: Some("MarkAppliedPendingConsumption(boundary_sequence=0)".into()),
            });
            state.updated_at = now;
        }

        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Applied {
            input_id: input_id.clone(),
            run_id: run_id.clone(),
        }));
        Ok(())
    }

    pub(crate) fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            let phase = self.input_phase(input_id);
            if phase != Some(InputLifecycleState::AppliedPendingConsumption) {
                continue;
            }
            let from_phase = phase.ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "generated input lifecycle phase missing before consuming input {input_id}"
                ))
            })?;

            let key = Self::dsl_key(input_id);
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::ConsumeInput { input_id: key },
                "ConsumeInput",
            )?;

            self.sync_terminal_projection_from_machine(
                input_id,
                from_phase,
                InputLifecycleState::Consumed,
                "Consume",
            )?;
            self.events
                .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Consumed {
                        input_id: input_id.clone(),
                        run_id: run_id.clone(),
                    },
                )));
        }
        Ok(())
    }

    fn machine_resolve_live_boundary_context_receipt(
        &mut self,
        run_id: &RunId,
        input_id: &InputId,
    ) -> Result<RunBoundaryReceipt, RuntimeDriverError> {
        let input_key = Self::dsl_key(input_id);
        let expected_run_id = mm_dsl::RunId::from_domain(run_id);
        let effects = self.dsl_apply_effects(
            mm_dsl::MeerkatMachineInput::ResolveLiveBoundaryContextReceipt {
                run_id: expected_run_id.clone(),
                input_id: input_key.clone(),
            },
            "ResolveLiveBoundaryContextReceipt",
        )?;

        let Some((effect_run_id, effect_input_id, boundary, boundary_sequence)) =
            effects.into_iter().find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::LiveBoundaryContextReceiptResolved {
                    run_id,
                    input_id,
                    boundary,
                    boundary_sequence,
                } => Some((run_id, input_id, boundary, boundary_sequence)),
                _ => None,
            })
        else {
            return Err(RuntimeDriverError::Internal(format!(
                "generated machine emitted no live-boundary receipt for input {input_id}"
            )));
        };

        if effect_run_id != expected_run_id || effect_input_id != input_key {
            return Err(RuntimeDriverError::Internal(format!(
                "generated machine emitted mismatched live-boundary receipt for input {input_id}"
            )));
        }

        Ok(RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: boundary.into(),
            contributing_input_ids: vec![input_id.clone()],
            conversation_digest: None,
            message_count: 0,
            sequence: boundary_sequence,
        })
    }

    pub(crate) fn machine_realize_live_boundary_context_injected(
        &mut self,
        run_id: &RunId,
        input_ids: &[InputId],
    ) -> Result<RunBoundaryReceipt, RuntimeDriverError> {
        let [input_id] = input_ids else {
            return Err(RuntimeDriverError::Internal(format!(
                "generated live-boundary receipt authority requires exactly one input, got {}",
                input_ids.len()
            )));
        };
        let checkpoint = self.rollback_snapshot();
        let result = self
            .machine_resolve_live_boundary_context_receipt(run_id, input_id)
            .and_then(|receipt| {
                self.machine_realize_stage_batch(input_ids, run_id)
                    .and_then(|()| self.machine_realize_boundary_applied(run_id, &receipt))
                    .and_then(|()| self.machine_realize_run_completed(run_id, input_ids))
                    .map(|()| receipt)
            });
        match result {
            Ok(receipt) => Ok(receipt),
            Err(err) => {
                self.restore_rollback_snapshot(checkpoint);
                Err(err)
            }
        }
    }

    pub fn rollback_staged(&mut self, input_ids: &[InputId]) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            // Skip inputs that are no longer in Staged (terminal or never-staged).
            if self.input_phase(input_id) != Some(InputLifecycleState::Staged) {
                continue;
            }
            let Some(_state) = self.ledger.get(input_id) else {
                continue;
            };

            let lane = self.input_recovery_lane(input_id).ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "generated recovery lane missing for rollback of staged input '{input_id}'"
                ))
            })?;
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::ResolveStagedRollback {
                    input_id: Self::dsl_key(input_id),
                    lane: mm_dsl::InputLane::from(lane),
                },
                "ResolveStagedRollback",
            )?;

            match self.input_phase_required(input_id, "after staged rollback resolution")? {
                InputLifecycleState::Queued => {
                    let now = Utc::now();
                    if let Some(state) = self.ledger.get_mut(input_id) {
                        state.history.push(InputStateHistoryEntry {
                            timestamp: now,
                            from: InputLifecycleState::Staged,
                            to: InputLifecycleState::Queued,
                            reason: Some("ResolveStagedRollback".into()),
                        });
                        state.updated_at = now;
                    }
                }
                InputLifecycleState::Abandoned => {
                    let attempts = self.input_attempt_count(input_id);
                    tracing::warn!(
                        input_id = %input_id,
                        attempts,
                        "input abandoned after generated max stage attempts decision"
                    );
                    self.sync_terminal_projection_from_machine(
                        input_id,
                        InputLifecycleState::Staged,
                        InputLifecycleState::Abandoned,
                        "ResolveStagedRollback->Abandon",
                    )?;
                    self.events
                        .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                            InputLifecycleEvent::Abandoned {
                                input_id: input_id.clone(),
                                reason: InputAbandonReason::MaxAttemptsExhausted { attempts },
                            },
                        )));
                }
                other => {
                    return Err(RuntimeDriverError::Internal(format!(
                        "generated staged rollback resolution for input {input_id} produced unexpected phase {other:?}"
                    )));
                }
            }
        }

        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(())
    }

    pub(crate) fn finalize_retire(&mut self) -> RetireReport {
        let inputs_pending_drain = self
            .ledger
            .iter()
            .filter(|(id, _)| self.input_is_non_terminal_by_authority(id))
            .count();
        RetireReport {
            inputs_abandoned: 0,
            inputs_pending_drain,
        }
    }

    pub(crate) fn reset_cleanup(&mut self) -> Result<ResetReport, RuntimeDriverError> {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset)?;
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(ResetReport {
            inputs_abandoned: abandoned,
        })
    }

    pub(crate) fn destroy_cleanup(&mut self) -> Result<usize, RuntimeDriverError> {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed)?;
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(abandoned)
    }

    pub(crate) fn stop_runtime_cleanup(&mut self) -> Result<(), RuntimeDriverError> {
        self.abandon_all_non_terminal(InputAbandonReason::Stopped)?;
        self.queue.drain();
        self.steer_queue.drain();
        Ok(())
    }

    pub(crate) fn finalize_stop_runtime(&mut self) -> Result<(), RuntimeDriverError> {
        self.stop_runtime_cleanup()
    }

    pub fn recover_ephemeral(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        crate::meerkat_machine::machine_recover_ephemeral_driver(self)
    }

    pub(crate) fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let transferred = self
            .ledger
            .iter()
            .filter(|(id, _)| self.input_is_non_terminal_by_authority(id))
            .count();
        let runtime_id = self.runtime_id.clone();
        let ledger = self.ledger.clone();
        let preserved_dsl = self.dsl.clone();
        let preserved_admission_order = std::mem::take(&mut self.admission_order);
        let preserved_handling_mode = std::mem::take(&mut self.handling_mode);
        let preserved_is_prompt = std::mem::take(&mut self.is_prompt_set);
        let preserved_content_shape = std::mem::take(&mut self.content_shape);
        let preserved_request_id = std::mem::take(&mut self.request_id);
        let preserved_reservation_key = std::mem::take(&mut self.reservation_key);
        let preserved_policy_snapshot = std::mem::take(&mut self.policy_snapshot);
        let control = self.control.clone();

        *self = Self::new_with_control(runtime_id, control);
        self.ledger = ledger;
        self.dsl = preserved_dsl;
        self.admission_order = preserved_admission_order;
        self.handling_mode = preserved_handling_mode;
        self.is_prompt_set = preserved_is_prompt;
        self.content_shape = preserved_content_shape;
        self.request_id = preserved_request_id;
        self.reservation_key = preserved_reservation_key;
        self.policy_snapshot = preserved_policy_snapshot;

        self.recover_ephemeral()?;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        Ok(transferred)
    }

    fn emit_event(&mut self, event: RuntimeEvent) {
        self.events.push(self.make_envelope(event));
    }
    fn make_envelope(&self, event: RuntimeEvent) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            id: crate::identifiers::RuntimeEventId::new(),
            timestamp: chrono::Utc::now(),
            runtime_id: self.runtime_id.clone(),
            event,
            causation_id: None,
            correlation_id: None,
        }
    }

    fn resolve_admission_plan_input(
        authority: &MachineAdmissionAuthority,
    ) -> mm_dsl::MeerkatMachineInput {
        authority.to_dsl_input()
    }

    fn handling_mode_from_admission_lane(lane: mm_dsl::InputLane) -> HandlingMode {
        match lane {
            mm_dsl::InputLane::Queue => HandlingMode::Queue,
            mm_dsl::InputLane::Steer => HandlingMode::Steer,
        }
    }

    fn admission_plan_from_machine_effect(
        plan: mm_dsl::AdmissionPlanKind,
        queue_action: mm_dsl::AdmissionQueueActionKind,
        lane: mm_dsl::InputLane,
        existing_action: mm_dsl::AdmissionExistingQueuedActionKind,
        existing_input_id: Option<String>,
    ) -> Result<AdmissionPlan, RuntimeDriverError> {
        if matches!(plan, mm_dsl::AdmissionPlanKind::ConsumedOnAccept) {
            return Ok(AdmissionPlan::ConsumedOnAccept);
        }

        let target = Self::handling_mode_from_admission_lane(lane);
        let queue_action = match queue_action {
            mm_dsl::AdmissionQueueActionKind::None => AdmissionQueueAction::None,
            mm_dsl::AdmissionQueueActionKind::EnqueueTo => {
                AdmissionQueueAction::EnqueueTo { target }
            }
            mm_dsl::AdmissionQueueActionKind::EnqueueFront => {
                AdmissionQueueAction::EnqueueFront { target }
            }
        };
        let existing_action = match (existing_action, existing_input_id) {
            (mm_dsl::AdmissionExistingQueuedActionKind::None, None) => None,
            (mm_dsl::AdmissionExistingQueuedActionKind::None, Some(existing_id)) => {
                return Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionPlan emitted existing input '{existing_id}' without existing action"
                )));
            }
            (mm_dsl::AdmissionExistingQueuedActionKind::Coalesce, Some(existing_id)) => {
                let existing_id = existing_id
                    .parse::<uuid::Uuid>()
                    .map(InputId::from_uuid)
                    .map_err(|err| {
                        RuntimeDriverError::Internal(format!(
                            "ResolveAdmissionPlan emitted invalid coalesce target id: {err}"
                        ))
                    })?;
                Some(ExistingQueuedAdmissionAction::Coalesce { existing_id })
            }
            (mm_dsl::AdmissionExistingQueuedActionKind::Supersede, Some(existing_id)) => {
                let existing_id = existing_id
                    .parse::<uuid::Uuid>()
                    .map(InputId::from_uuid)
                    .map_err(|err| {
                        RuntimeDriverError::Internal(format!(
                            "ResolveAdmissionPlan emitted invalid supersede target id: {err}"
                        ))
                    })?;
                Some(ExistingQueuedAdmissionAction::Supersede { existing_id })
            }
            (action, None) => {
                return Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionPlan emitted {action:?} without an existing input target"
                )));
            }
        };

        Ok(AdmissionPlan::Queued {
            persist_and_queue: true,
            queue_action,
            existing_action,
        })
    }

    fn resolved_idempotency_from_machine_effects(
        input_id: &InputId,
        effects: Vec<mm_dsl::MeerkatMachineEffect>,
    ) -> Result<Option<InputId>, RuntimeDriverError> {
        let Some((effect_input_id, result, existing_input_id)) =
            effects.into_iter().find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::AdmissionIdempotencyResolved {
                    input_id,
                    result,
                    existing_input_id,
                } => Some((input_id, result, existing_input_id)),
                _ => None,
            })
        else {
            return Err(RuntimeDriverError::Internal(
                "ResolveAdmissionIdempotency emitted no AdmissionIdempotencyResolved effect".into(),
            ));
        };

        if effect_input_id != input_id.to_string() {
            return Err(RuntimeDriverError::Internal(format!(
                "ResolveAdmissionIdempotency returned input id '{effect_input_id}' for '{input_id}'"
            )));
        }

        match (result, existing_input_id) {
            (mm_dsl::AdmissionIdempotencyResultKind::Accept, None) => Ok(None),
            (mm_dsl::AdmissionIdempotencyResultKind::Accept, Some(existing_id)) => {
                Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionIdempotency accepted '{input_id}' but emitted existing input '{existing_id}'"
                )))
            }
            (mm_dsl::AdmissionIdempotencyResultKind::Deduplicated, Some(existing_id)) => {
                let existing_id = existing_id
                    .parse::<uuid::Uuid>()
                    .map(InputId::from_uuid)
                    .map_err(|err| {
                        RuntimeDriverError::Internal(format!(
                            "ResolveAdmissionIdempotency emitted invalid existing input id: {err}"
                        ))
                    })?;
                Ok(Some(existing_id))
            }
            (mm_dsl::AdmissionIdempotencyResultKind::Deduplicated, None) => {
                Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionIdempotency deduplicated '{input_id}' without an existing input"
                )))
            }
        }
    }

    fn admission_validation_from_machine_effects(
        input_id: &InputId,
        effects: Vec<mm_dsl::MeerkatMachineEffect>,
    ) -> Result<Option<mm_dsl::AdmissionRejectReasonKind>, RuntimeDriverError> {
        let Some((effect_input_id, result, reject_reason)) =
            effects.into_iter().find_map(|effect| match effect {
                mm_dsl::MeerkatMachineEffect::AdmissionValidationResolved {
                    input_id,
                    result,
                    reject_reason,
                } => Some((input_id, result, reject_reason)),
                _ => None,
            })
        else {
            return Err(RuntimeDriverError::Internal(
                "ResolveAdmissionValidation emitted no AdmissionValidationResolved effect".into(),
            ));
        };

        if effect_input_id != input_id.to_string() {
            return Err(RuntimeDriverError::Internal(format!(
                "ResolveAdmissionValidation returned input id '{effect_input_id}' for '{input_id}'"
            )));
        }

        match (result, reject_reason) {
            (mm_dsl::AdmissionValidationResultKind::Accept, None) => Ok(None),
            (mm_dsl::AdmissionValidationResultKind::Accept, Some(reason)) => {
                Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionValidation accepted '{input_id}' but emitted rejection reason {reason:?}"
                )))
            }
            (mm_dsl::AdmissionValidationResultKind::Reject, Some(reason)) => Ok(Some(reason)),
            (mm_dsl::AdmissionValidationResultKind::Reject, None) => {
                Err(RuntimeDriverError::Internal(format!(
                    "ResolveAdmissionValidation rejected '{input_id}' without a typed reason"
                )))
            }
        }
    }

    fn resolve_admission_validation(
        &self,
        input_id: &InputId,
        facts: AdmissionValidationFacts<'_>,
    ) -> Result<Option<mm_dsl::AdmissionRejectReasonKind>, RuntimeDriverError> {
        let effects = self.dsl_preview(
            mm_dsl::MeerkatMachineInput::ResolveAdmissionValidation {
                input_id: Self::dsl_key(input_id),
                input_kind: mm_dsl::AdmissionInputKind::from(facts.input_kind),
                input_origin: mm_dsl::AdmissionInputOriginKind::from(facts.input_origin),
                durability: mm_dsl::InputDurabilityKind::from(facts.durability),
                peer_handling_mode_valid: facts.peer_handling_mode_valid,
                peer_response_terminal_structurally_valid: facts
                    .peer_response_terminal_structurally_valid,
                peer_response_terminal_observed_status: facts
                    .peer_response_terminal_observed_status,
            },
            "ResolveAdmissionValidation",
        )?;
        Self::admission_validation_from_machine_effects(input_id, effects)
    }

    fn peer_response_terminal_observed_status(
        input: &Input,
    ) -> mm_dsl::PeerResponseTerminalObservedStatus {
        let Input::Peer(peer) = input else {
            return mm_dsl::PeerResponseTerminalObservedStatus::NotPeerTerminal;
        };
        let Some(crate::input::PeerConvention::ResponseTerminal { status, .. }) = &peer.convention
        else {
            return mm_dsl::PeerResponseTerminalObservedStatus::NotPeerTerminal;
        };

        match status {
            meerkat_core::handles::PeerResponseTerminalProjectionStatus::Completed => {
                mm_dsl::PeerResponseTerminalObservedStatus::Completed
            }
            meerkat_core::handles::PeerResponseTerminalProjectionStatus::Failed => {
                mm_dsl::PeerResponseTerminalObservedStatus::Failed
            }
            meerkat_core::handles::PeerResponseTerminalProjectionStatus::Cancelled => {
                mm_dsl::PeerResponseTerminalObservedStatus::Cancelled
            }
        }
    }

    fn peer_response_terminal_generated_rejection_detail(input: &Input) -> Option<String> {
        let Input::Peer(peer) = input else {
            return None;
        };
        let Some(crate::input::PeerConvention::ResponseTerminal { status, .. }) = &peer.convention
        else {
            return None;
        };

        Some(format!(
            "peer response terminal status rejected by generated authority: {}",
            status.label()
        ))
    }

    /// Render the machine-emitted typed rejection reason into the domain
    /// `RejectReason`. Durability rejection text is pure rendering of the
    /// typed reason the generated authority emitted — the shell does not
    /// re-evaluate any durability rule here.
    fn reject_reason_from_machine_validation(
        reason: mm_dsl::AdmissionRejectReasonKind,
        input_kind: crate::identifiers::InputKind,
        peer_handling_mode_detail: Option<&str>,
        peer_response_terminal_detail: Option<&str>,
    ) -> Result<RejectReason, RuntimeDriverError> {
        let missing_detail = || {
            RuntimeDriverError::Internal(format!(
                "ResolveAdmissionValidation emitted {reason:?} without matching validation detail"
            ))
        };
        match reason {
            mm_dsl::AdmissionRejectReasonKind::DurabilityMissing => {
                Ok(RejectReason::DurabilityViolation {
                    detail: "input durability observation missing".to_owned(),
                })
            }
            mm_dsl::AdmissionRejectReasonKind::ExternalDerivedDurabilityForbidden => {
                Ok(RejectReason::DurabilityViolation {
                    detail: "External ingress cannot submit derived inputs".to_owned(),
                })
            }
            mm_dsl::AdmissionRejectReasonKind::DerivedDurabilityForbiddenForInputKind => {
                Ok(RejectReason::DurabilityViolation {
                    detail: format!("Derived durability forbidden for {input_kind}"),
                })
            }
            mm_dsl::AdmissionRejectReasonKind::PeerHandlingModeInvalid => {
                Ok(RejectReason::PeerHandlingModeInvalid {
                    detail: peer_handling_mode_detail
                        .ok_or_else(missing_detail)?
                        .to_owned(),
                })
            }
            mm_dsl::AdmissionRejectReasonKind::PeerResponseTerminalInvalid => {
                Ok(RejectReason::PeerResponseTerminalInvalid {
                    detail: peer_response_terminal_detail
                        .ok_or_else(missing_detail)?
                        .to_owned(),
                })
            }
        }
    }

    fn reject_peer_response_terminal_observation_if_present(
        &mut self,
        input: &Input,
        detail: &str,
    ) {
        let Input::Peer(peer) = input else {
            return;
        };
        let Some(crate::input::PeerConvention::ResponseTerminal { request_id, .. }) =
            &peer.convention
        else {
            return;
        };
        let Ok(corr_id) = uuid::Uuid::parse_str(request_id) else {
            return;
        };

        if let Err(error) = self.dsl_apply(
            mm_dsl::MeerkatMachineInput::PeerResponseRejected {
                corr_id: corr_id.into(),
            },
            "PeerResponseRejected(invalid peer terminal observation)",
        ) {
            tracing::debug!(
                request_id,
                detail,
                error = ?error,
                "generated peer response rejection did not match pending request"
            );
        }
    }

    fn resolve_idempotency(
        &mut self,
        input_id: &InputId,
        idempotency_key: Option<String>,
    ) -> Result<Option<InputId>, RuntimeDriverError> {
        let effects = self.dsl_apply_effects(
            mm_dsl::MeerkatMachineInput::ResolveAdmissionIdempotency {
                input_id: Self::dsl_key(input_id),
                idempotency_key,
            },
            "ResolveAdmissionIdempotency",
        )?;
        Self::resolved_idempotency_from_machine_effects(input_id, effects)
    }

    pub(crate) fn register_accepted_idempotency(
        &mut self,
        input_id: &InputId,
        idempotency_key: Option<&IdempotencyKey>,
    ) -> Result<(), RuntimeDriverError> {
        let Some(idempotency_key) = idempotency_key else {
            return Ok(());
        };
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::RegisterAcceptedIdempotency {
                input_id: Self::dsl_key(input_id),
                idempotency_key: idempotency_key.to_string(),
            },
            "RegisterAcceptedIdempotency",
        )
    }

    fn resolved_admission_from_machine_effects(
        &self,
        input: &Input,
        authority: MachineAdmissionAuthority,
        effects: Vec<mm_dsl::MeerkatMachineEffect>,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        let Some(effect) = effects.into_iter().find_map(|effect| match effect {
            mm_dsl::MeerkatMachineEffect::AdmissionResolved {
                input_id,
                policy_version,
                policy_apply_mode,
                policy_wake_mode,
                policy_queue_mode,
                policy_consume_point,
                policy_drain_policy,
                policy_routing_disposition,
                lane,
                plan,
                queue_action,
                existing_action,
                existing_input_id,
                requires_active_pre_admission,
                runtime_boundary,
                runtime_execution_kind,
                runtime_peer_response_terminal_apply_intent,
                record_transcript,
                request_immediate_processing,
                interrupt_yielding,
                wake_if_idle,
                execution_handling_mode,
                live_interrupt_required,
            } => Some((
                input_id,
                policy_version,
                policy_apply_mode,
                policy_wake_mode,
                policy_queue_mode,
                policy_consume_point,
                policy_drain_policy,
                policy_routing_disposition,
                lane,
                plan,
                queue_action,
                existing_action,
                existing_input_id,
                requires_active_pre_admission,
                runtime_boundary,
                runtime_execution_kind,
                runtime_peer_response_terminal_apply_intent,
                record_transcript,
                request_immediate_processing,
                interrupt_yielding,
                wake_if_idle,
                execution_handling_mode,
                live_interrupt_required,
            )),
            _ => None,
        }) else {
            return Err(RuntimeDriverError::Internal(
                "ResolveAdmissionPlan emitted no AdmissionResolved effect".into(),
            ));
        };

        let (
            input_id,
            policy_version,
            policy_apply_mode,
            policy_wake_mode,
            policy_queue_mode,
            policy_consume_point,
            policy_drain_policy,
            policy_routing_disposition,
            lane,
            plan,
            queue_action,
            existing_action,
            existing_input_id,
            requires_active_pre_admission,
            runtime_boundary,
            runtime_execution_kind,
            runtime_peer_response_terminal_apply_intent,
            record_transcript,
            request_immediate_processing,
            interrupt_yielding,
            wake_if_idle,
            execution_handling_mode,
            live_interrupt_required,
        ) = effect;

        if input_id != authority.input_id() {
            return Err(RuntimeDriverError::Internal(format!(
                "ResolveAdmissionPlan returned input id '{input_id}' for '{}'",
                authority.input_id()
            )));
        }

        let policy = PolicyDecision {
            apply_mode: policy_apply_mode.into(),
            wake_mode: policy_wake_mode.into(),
            queue_mode: policy_queue_mode.into(),
            consume_point: policy_consume_point.into(),
            drain_policy: policy_drain_policy.into(),
            routing_disposition: policy_routing_disposition.into(),
            record_transcript,
            emit_operator_content: record_transcript,
            policy_version: PolicyVersion(policy_version),
        };
        let runtime_semantics = RuntimeInputSemantics {
            boundary: runtime_boundary.into(),
            execution_kind: runtime_execution_kind.into(),
            // #24: the machine emits the idle-steer normalization directly as a
            // typed `Option<InputLane>`; project to `HandlingMode` via the
            // existing lane mapping. The shell normalizer is deleted.
            execution_handling_mode: execution_handling_mode
                .map(Self::handling_mode_from_admission_lane),
            peer_response_terminal_apply_intent: runtime_peer_response_terminal_apply_intent
                .map(Into::into),
            live_interrupt_required,
        };
        let handling_mode = Self::handling_mode_from_admission_lane(lane);
        let admission_plan = Self::admission_plan_from_machine_effect(
            plan,
            queue_action,
            lane,
            existing_action,
            existing_input_id,
        )?;

        Ok(ResolvedAdmission::from_machine_resolution(
            policy,
            handling_mode,
            runtime_semantics,
            crate::input::runtime_input_projection(input),
            admission_plan,
            CoarseAdmissionFlags {
                request_immediate_processing,
                interrupt_yielding,
                wake_if_idle,
            },
            requires_active_pre_admission,
            authority,
        ))
    }

    fn resolve_admission_with_wake_policy(
        &self,
        input: &Input,
        without_wake: bool,
        active_turn_boundary_available: bool,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        let existing_superseded_id = self.existing_superseded_input(input).map(|(id, _)| id);
        let authority = MachineAdmissionAuthority::new(
            input.id().to_string(),
            mm_dsl::AdmissionInputKind::from(input.kind()),
            input.handling_mode().map(mm_dsl::InputLane::from),
            mm_dsl::AdmissionContinuationKind::from(input.continuation_kind()),
            self.matches_silent_intent_authority(input),
            existing_superseded_id,
            self.runtime_phase_snapshot() == RuntimeState::Running,
            active_turn_boundary_available,
            without_wake,
        );
        let effects = self.dsl_preview(
            Self::resolve_admission_plan_input(&authority),
            "ResolveAdmissionPlan",
        )?;
        self.resolved_admission_from_machine_effects(input, authority, effects)
    }

    pub(crate) fn resolve_admission(
        &self,
        input: &Input,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        self.resolve_admission_with_wake_policy(input, false, false)
    }

    pub(crate) fn resolve_admission_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        self.resolve_admission_with_wake_policy(input, false, active_turn_boundary_available)
    }

    pub(crate) fn resolve_admission_without_wake_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        self.resolve_admission_with_wake_policy(input, true, active_turn_boundary_available)
    }

    pub(crate) fn machine_apply_accept_with_completion_signal(
        &mut self,
        input_id: &InputId,
        flags: crate::accept::CoarseAdmissionFlags,
    ) -> Result<(), RuntimeDriverError> {
        self.dsl_apply(
            mm_dsl::MeerkatMachineInput::AcceptWithCompletion {
                input_id: mm_dsl::InputId::from_domain(input_id),
                request_immediate_processing: flags.request_immediate_processing,
                interrupt_yielding: flags.interrupt_yielding,
                wake_if_idle: flags.wake_if_idle,
            },
            "AcceptWithCompletion(RuntimeDriver)",
        )
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let runtime_phase = self.runtime_phase_snapshot();
        let lifecycle_facts = crate::meerkat_machine::classify_runtime_lifecycle_state(
            runtime_phase,
        )
        .map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "generated runtime lifecycle admission classification failed: {err}"
            ))
        })?;
        if !lifecycle_facts.can_accept_input() {
            return match lifecycle_facts.ingress_admission {
                mm_dsl::RuntimeIngressAdmission::Destroyed => Err(RuntimeDriverError::Destroyed),
                mm_dsl::RuntimeIngressAdmission::Open
                | mm_dsl::RuntimeIngressAdmission::NotReady => Err(RuntimeDriverError::NotReady {
                    state: runtime_phase,
                }),
            };
        }

        let input_id = input.id().clone();
        let peer_handling_mode_error =
            crate::peer_handling_mode::validate_peer_handling_mode(&input)
                .err()
                .map(|error| error.to_string());
        let peer_response_terminal_structural_error =
            crate::input::validate_peer_response_terminal_fact(&input)
                .err()
                .map(|error| error.to_string());
        let peer_response_terminal_observed_status =
            Self::peer_response_terminal_observed_status(&input);
        let peer_response_terminal_detail = peer_response_terminal_structural_error
            .clone()
            .or_else(|| Self::peer_response_terminal_generated_rejection_detail(&input));
        if let Some(reason) = self.resolve_admission_validation(
            &input_id,
            AdmissionValidationFacts {
                input_kind: input.kind(),
                input_origin: &input.header().source,
                durability: input.header().durability,
                peer_handling_mode_valid: peer_handling_mode_error.is_none(),
                peer_response_terminal_structurally_valid: peer_response_terminal_structural_error
                    .is_none(),
                peer_response_terminal_observed_status,
            },
        )? {
            if matches!(
                reason,
                mm_dsl::AdmissionRejectReasonKind::PeerResponseTerminalInvalid
            ) && let Some(detail) = peer_response_terminal_detail.as_deref()
            {
                self.reject_peer_response_terminal_observation_if_present(&input, detail);
            }
            let reason = Self::reject_reason_from_machine_validation(
                reason,
                input.kind(),
                peer_handling_mode_error.as_deref(),
                peer_response_terminal_detail.as_deref(),
            )?;
            return Ok(AcceptOutcome::Rejected { reason });
        }

        if resolved.authority().input_id() != input_id.to_string() {
            return Err(RuntimeDriverError::Internal(format!(
                "resolved admission authority id '{}' did not match accepted input '{input_id}'",
                resolved.authority().input_id()
            )));
        }

        if let Some(existing_id) = self.resolve_idempotency(
            &input_id,
            input
                .header()
                .idempotency_key
                .as_ref()
                .map(std::string::ToString::to_string),
        )? {
            tracing::debug!(
                work_id = ?input_id,
                existing_id = ?existing_id,
                "input deduplicated"
            );
            self.emit_event(RuntimeEvent::InputLifecycle(
                InputLifecycleEvent::Deduplicated {
                    input_id: input_id.clone(),
                    existing_id: existing_id.clone(),
                },
            ));
            return Ok(AcceptOutcome::Deduplicated {
                input_id,
                existing_id,
            });
        }

        let mut state = InputState::new_accepted(input_id.clone());
        state.durability = Some(input.header().durability);
        state.idempotency_key = input.header().idempotency_key.clone();
        let existing_superseded_id = self.existing_superseded_input(&input).map(|(id, _)| id);
        let authority = MachineAdmissionAuthority::new(
            input_id.to_string(),
            mm_dsl::AdmissionInputKind::from(input.kind()),
            input.handling_mode().map(mm_dsl::InputLane::from),
            mm_dsl::AdmissionContinuationKind::from(input.continuation_kind()),
            self.matches_silent_intent_authority(&input),
            existing_superseded_id,
            self.runtime_phase_snapshot() == RuntimeState::Running,
            resolved.authority().active_turn_boundary_available(),
            resolved.authority().without_wake(),
        );
        let effects = self.dsl_apply_effects(
            Self::resolve_admission_plan_input(&authority),
            "ResolveAdmissionPlan",
        )?;
        let resolved = self.resolved_admission_from_machine_effects(&input, authority, effects)?;
        let (policy, handling_mode, runtime_semantics, primitive_projection, admission_plan) =
            resolved.into_parts();

        let content_shape = ContentShape::from_kind(input.kind());
        let is_prompt = matches!(input, Input::Prompt(_));
        match admission_plan {
            AdmissionPlan::ConsumedOnAccept => {
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::QueueAccepted {
                        input_id: Self::dsl_key(&input_id),
                    },
                    "QueueAccepted(consumed_on_accept)",
                )?;
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::ConsumeOnAccept {
                        input_id: Self::dsl_key(&input_id),
                    },
                    "ConsumeOnAccept",
                )?;
                self.register_accepted_idempotency(
                    &input_id,
                    input.header().idempotency_key.as_ref(),
                )?;
                let terminal_outcome = self.input_terminal_outcome(&input_id).ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "machine terminal projection missing consume-on-accept outcome for {input_id}"
                    ))
                })?;
                if terminal_outcome != InputTerminalOutcome::Consumed {
                    return Err(RuntimeDriverError::Internal(format!(
                        "machine terminal projection for consume-on-accept {input_id} was {terminal_outcome:?}"
                    )));
                }
                let now = Utc::now();
                state.policy = Some(PolicySnapshot {
                    version: policy.policy_version,
                    decision: policy.clone(),
                });
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: InputLifecycleState::Accepted,
                    to: InputLifecycleState::Consumed,
                    reason: Some("ConsumeOnAccept (Ignore+OnAccept)".into()),
                });
                state.updated_at = now;
                self.ledger.accept(state);
                self.record_admission_metadata(
                    &input_id,
                    &content_shape,
                    handling_mode,
                    runtime_semantics,
                    primitive_projection,
                    is_prompt,
                    Some(&policy),
                    None,
                    None,
                );
                self.emit_event(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Accepted {
                        input_id: input_id.clone(),
                    },
                ));
                tracing::debug!(work_id = ?input_id, "input consumed on accept");
            }
            AdmissionPlan::Queued {
                persist_and_queue,
                queue_action,
                existing_action,
            } => {
                if persist_and_queue {
                    self.apply_persist_and_queue(
                        &input_id,
                        &input,
                        &content_shape,
                        handling_mode,
                        runtime_semantics,
                        primitive_projection,
                        is_prompt,
                        &policy,
                        state,
                        queue_action,
                        existing_action.as_ref(),
                    )?;
                }
            }
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        let final_bundle = self.stored_input_state(&input_id).ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "accepted input {input_id} missing generated lifecycle seed"
            ))
        })?;
        Ok(AcceptOutcome::Accepted {
            input_id,
            policy,
            state: final_bundle.state,
            seed: final_bundle.seed,
        })
    }

    pub(crate) async fn preview_accept_resolved_input(
        &self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let mut staged = self.clone_with_isolated_dsl_authority();
        staged.ensure_contract_session_authority()?;
        staged.accept_resolved_input(input, resolved).await
    }

    pub fn abandon_all_non_terminal(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let non_terminal_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter_map(|(id, _)| {
                if self.input_is_non_terminal_by_authority(id) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        let dsl_reason = mm_dsl::InputAbandonReason::from(&reason);
        let mut count = 0;
        for id in &non_terminal_ids {
            let key = Self::dsl_key(id);
            let attempt_count = u64::from(self.input_attempt_count(id));
            let from_phase = self.input_phase(id).ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "generated input lifecycle phase missing before abandoning input {id}"
                ))
            })?;
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::AbandonInput {
                    input_id: key.clone(),
                    reason: dsl_reason,
                    attempt_count,
                },
                "AbandonInput",
            )?;

            self.sync_terminal_projection_from_machine(
                id,
                from_phase,
                InputLifecycleState::Abandoned,
                "Abandon",
            )?;
            count += 1;
            self.events
                .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Abandoned {
                        input_id: id.clone(),
                        reason: reason.clone(),
                    },
                )));
        }
        Ok(count)
    }

    pub(crate) fn abandon_staged_inputs(
        &mut self,
        input_ids: &[InputId],
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let dsl_reason = mm_dsl::InputAbandonReason::from(&reason);
        let mut count = 0;
        for input_id in input_ids {
            if self.input_phase(input_id) != Some(InputLifecycleState::Staged) {
                continue;
            }
            let from_phase =
                self.input_phase_required(input_id, "before abandoning staged input")?;
            let attempt_count = u64::from(self.input_attempt_count(input_id));
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::AbandonInput {
                    input_id: Self::dsl_key(input_id),
                    reason: dsl_reason,
                    attempt_count,
                },
                "AbandonInput(CancelledRun)",
            )?;

            self.sync_terminal_projection_from_machine(
                input_id,
                from_phase,
                InputLifecycleState::Abandoned,
                "Abandon",
            )?;
            count += 1;
            self.events
                .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Abandoned {
                        input_id: input_id.clone(),
                        reason: reason.clone(),
                    },
                )));
        }

        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(count)
    }

    pub(crate) fn abandon_pending_inputs(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        let abandoned = self.abandon_all_non_terminal(reason)?;
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        Ok(abandoned)
    }

    /// Machine-owned realization for a validated run-completion transition.
    ///
    /// Delegates to `consume_inputs`, which drives the DSL `ConsumeInput`
    /// transition and mirrors the `Consumed` phase on each contributor's
    /// shell `InputState`.
    pub(crate) fn machine_realize_run_completed(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.consume_inputs(consumed_input_ids, run_id)
    }

    /// Machine-owned realization for a validated failed-run replay plan.
    pub(crate) fn machine_realize_run_failed(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
        replay_plan: &ReplayQueuedContributorsPlan,
    ) -> Result<(), RuntimeDriverError> {
        tracing::debug!(
            run_id = ?run_id,
            kind = replay_plan.notice_kind,
            queue = replay_plan.queue_work_ids.len(),
            steer = replay_plan.steer_work_ids.len(),
            "runtime replayed queued contributors"
        );

        // `rollback_staged` drives generated `ResolveStagedRollback`
        // authority and re-inserts surviving contributors into the correct
        // lane — no separate lane seeding is needed here.
        self.rollback_staged(contributing_input_ids)
    }

    /// Machine-owned realization for a validated cancelled run.
    pub(crate) fn machine_realize_run_cancelled(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        tracing::debug!(
            run_id = ?run_id,
            contributors = contributing_input_ids.len(),
            "runtime abandoned cancelled run contributors"
        );
        let _ =
            self.abandon_staged_inputs(contributing_input_ids, InputAbandonReason::Cancelled)?;
        Ok(())
    }

    /// Machine-owned realization for a validated boundary-application step.
    pub(crate) fn machine_realize_boundary_applied(
        &mut self,
        run_id: &RunId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeDriverError> {
        tracing::debug!(
            contributors = receipt.contributing_input_ids.len(),
            sequence = receipt.sequence,
            "runtime boundary applied"
        );

        for input_id in &receipt.contributing_input_ids {
            let key = Self::dsl_key(input_id);
            if !self.with_dsl_state(|state| state.input_phases.contains_key(&key)) {
                continue;
            }
            // The matches_last_run guard on the DSL / shell is enforced by
            // the MeerkatMachine contributor-set validation before this
            // realization runs; here we just drive the transitions.
            if self.input_phase(input_id) != Some(InputLifecycleState::Staged) {
                continue;
            }

            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::MarkApplied {
                    input_id: key.clone(),
                },
                "MarkApplied",
            )?;
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::MarkAppliedPendingConsumption {
                    input_id: key.clone(),
                },
                "MarkAppliedPendingConsumption",
            )?;
            // The boundary sequence is machine-derived: RecordBoundarySeq
            // reads the canonical per-run counter inside the generated
            // machine. The receipt's `sequence` is a read-only projection of
            // that same counter, never a producer.
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::RecordBoundarySeq {
                    input_id: key,
                    run_id: mm_dsl::RunId::from_domain(run_id),
                },
                "RecordBoundarySeq",
            )?;

            let now = Utc::now();
            if let Some(state) = self.ledger.get_mut(input_id) {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: InputLifecycleState::Staged,
                    to: InputLifecycleState::Applied,
                    reason: Some(format!("MarkApplied({run_id})")),
                });
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: InputLifecycleState::Applied,
                    to: InputLifecycleState::AppliedPendingConsumption,
                    reason: Some(format!(
                        "MarkAppliedPendingConsumption(boundary_sequence={})",
                        receipt.sequence
                    )),
                });
                state.updated_at = now;
            }
            self.events
                .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Applied {
                        input_id: input_id.clone(),
                        run_id: run_id.clone(),
                    },
                )));
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeDriver for EphemeralRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let resolved = self.resolve_admission(&input)?;
        let flags = resolved.coarse_flags();
        self.ensure_contract_session_authority()?;
        let checkpoint = self.rollback_snapshot();
        let outcome = match self.accept_resolved_input(input, resolved).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.restore_rollback_snapshot(checkpoint);
                return Err(err);
            }
        };
        if let AcceptOutcome::Accepted { input_id, .. } = &outcome
            && let Err(err) = self.machine_apply_accept_with_completion_signal(input_id, flags)
        {
            self.restore_rollback_snapshot(checkpoint);
            return Err(err);
        }
        Ok(outcome)
    }

    async fn on_runtime_event(
        &mut self,
        _event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        Ok(())
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        self.recover_ephemeral()
    }
    fn runtime_state(&self) -> RuntimeState {
        self.runtime_phase_snapshot()
    }
    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.ledger.get(input_id)
    }
    fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        EphemeralRuntimeDriver::input_phase(self, input_id)
    }
    fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        EphemeralRuntimeDriver::input_last_run_id(self, input_id)
    }
    fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        EphemeralRuntimeDriver::input_last_boundary_sequence(self, input_id)
    }
    fn stored_input_state(&self, input_id: &InputId) -> Option<StoredInputState> {
        EphemeralRuntimeDriver::stored_input_state(self, input_id)
    }
    fn active_input_ids(&self) -> Vec<InputId> {
        self.ledger
            .iter()
            .filter(|(id, _)| self.input_is_non_terminal_by_authority(id))
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionValidationFacts, EphemeralRuntimeDriver};
    use crate::identifiers::{IdempotencyKey, LogicalRuntimeId, SupersessionKey};
    use crate::input::{
        Input, InputDurability, InputHeader, InputOrigin, InputVisibility, OperationInput,
        PeerConvention, PeerInput, PromptInput,
    };
    use crate::input_state::{
        InputAbandonReason, InputLifecycleState, InputStateSeed, InputTerminalOutcome,
    };
    use crate::meerkat_machine::dsl as mm_dsl;
    use crate::traits::{RuntimeDriver, RuntimeDriverError};
    use crate::{RuntimeState, WakeMode};
    use chrono::Utc;
    use meerkat_core::lifecycle::{InputId, RunId};
    use meerkat_core::ops::{OpEvent, OperationId};

    fn peer_message_input() -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            content: "peer body".into(),
            payload: None,
            handling_mode: None,
        })
    }

    fn prompt_input(text: &str) -> Input {
        Input::Prompt(PromptInput::new(text, None))
    }

    fn operation_input() -> Input {
        let operation_id = OperationId::new();
        Input::Operation(OperationInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Derived,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            operation_id: operation_id.clone(),
            event: OpEvent::Cancelled { id: operation_id },
        })
    }

    fn progress_input_with_supersession(label: &str, supersession_key: &str) -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    display_identity: None,
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: Some(SupersessionKey::new(supersession_key)),
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseProgress {
                request_id: format!("request-{label}"),
                phase: crate::input::ResponseProgressPhase::InProgress,
            }),
            content: format!("progress {label}").into(),
            payload: None,
            handling_mode: None,
        })
    }

    fn force_control_shadow(
        driver: &mut EphemeralRuntimeDriver,
        phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        let mut control = driver.write_control_projection();
        control.phase = phase;
        control.current_run_id = current_run_id;
        control.pre_run_phase = pre_run_phase;
    }

    #[test]
    fn set_control_projection_does_not_write_dsl_authority() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("projection-only"));
        let run_id = RunId::new();

        driver.set_control_projection(
            RuntimeState::Running,
            Some(run_id),
            Some(RuntimeState::Attached),
        );

        assert_eq!(
            driver.runtime_phase_snapshot(),
            RuntimeState::Idle,
            "control projection writes must not mutate DSL lifecycle truth",
        );
        assert_eq!(
            driver.current_run_id(),
            None,
            "control projection writes must not mutate DSL run binding truth",
        );
        assert_eq!(
            driver.control_snapshot().phase,
            RuntimeState::Running,
            "the shell projection still records the mechanical projection",
        );
    }

    #[tokio::test]
    async fn direct_accept_uses_dsl_phase_not_control_projection_shadow() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("admission-shadow"));
        force_control_shadow(&mut driver, RuntimeState::Stopped, None, None);

        let outcome = driver.accept_input(peer_message_input()).await.unwrap();

        assert!(
            outcome.is_accepted(),
            "direct RuntimeDriver admission should follow DSL phase, not a stale control shadow",
        );
    }

    #[tokio::test]
    async fn priority_enqueue_order_is_assigned_by_machine() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("priority-order"));

        let normal_a = prompt_input("normal-a");
        let normal_a_id = normal_a.id().clone();
        driver.accept_input(normal_a).await.unwrap();

        let normal_b = prompt_input("normal-b");
        let normal_b_id = normal_b.id().clone();
        driver.accept_input(normal_b).await.unwrap();

        let priority = prompt_input("priority");
        let priority_id = priority.id().clone();
        driver.accept_input(priority).await.unwrap();
        driver
            .dsl_apply(
                mm_dsl::MeerkatMachineInput::PrioritizeInput {
                    input_id: priority_id.to_string(),
                },
                "PrioritizeInput(test)",
            )
            .unwrap();
        driver.rebuild_queue_projections();

        assert_eq!(
            driver.dsl_queue_lane(),
            vec![
                priority_id.clone(),
                normal_a_id.clone(),
                normal_b_id.clone()
            ]
        );
        let (priority_seq, normal_a_seq) = driver.with_dsl_state(|state| {
            (
                state.input_admission_seq[&priority_id.to_string()],
                state.input_admission_seq[&normal_a_id.to_string()],
            )
        });
        assert!(
            priority_seq < normal_a_seq,
            "priority order must be represented by generated admission sequence"
        );
    }

    #[tokio::test]
    async fn backlog_deferral_order_is_assigned_by_machine() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("backlog-deferral"));

        let first = prompt_input("first");
        let first_id = first.id().clone();
        driver.accept_input(first).await.unwrap();

        let second = prompt_input("second");
        let second_id = second.id().clone();
        driver.accept_input(second).await.unwrap();

        driver
            .defer_queued_inputs_behind_backlog(std::slice::from_ref(&first_id))
            .unwrap();

        assert_eq!(
            driver.dsl_queue_lane(),
            vec![second_id.clone(), first_id.clone()]
        );
        let (first_seq, second_seq) = driver.with_dsl_state(|state| {
            (
                state.input_admission_seq[&first_id.to_string()],
                state.input_admission_seq[&second_id.to_string()],
            )
        });
        assert!(
            first_seq > second_seq,
            "deferred order must be represented by generated admission sequence"
        );
    }

    #[tokio::test]
    async fn coalesce_input_requires_generated_existing_target_authority() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("coalesce-guard"));

        let first = prompt_input("first");
        let first_id = first.id().clone();
        driver.accept_input(first).await.unwrap();

        let err = driver
            .dsl_apply(
                mm_dsl::MeerkatMachineInput::CoalesceInput {
                    input_id: first_id.to_string(),
                    aggregate_id: InputId::new().to_string(),
                },
                "CoalesceInput",
            )
            .unwrap_err();

        assert!(
            matches!(err, RuntimeDriverError::Internal(message) if message.contains("CoalesceInput")),
            "unauthorized coalesce should fail closed through generated guards"
        );
    }

    #[tokio::test]
    async fn progress_coalesce_target_is_supplied_by_generated_admission_authority() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("coalesce-authority"));

        let first = progress_input_with_supersession("first", "same-window");
        let first_id = first.id().clone();
        driver.accept_input(first).await.unwrap();

        let second = progress_input_with_supersession("second", "same-window");
        let second_id = second.id().clone();
        driver.accept_input(second).await.unwrap();

        assert_eq!(
            driver.input_phase(&first_id),
            Some(crate::input_state::InputLifecycleState::Coalesced)
        );
        assert_eq!(
            driver.input_terminal_outcome(&first_id),
            Some(crate::input_state::InputTerminalOutcome::Coalesced {
                aggregate_id: second_id.clone()
            })
        );
        assert!(!driver.has_queued_input(&first_id));
        assert!(driver.has_queued_input(&second_id));
        driver.with_dsl_state(|state| {
            assert!(
                !state
                    .admission_authorized_existing_actions
                    .contains_key(&second_id.to_string())
            );
            assert!(
                !state
                    .admission_authorized_existing_targets
                    .contains_key(&second_id.to_string())
            );
        });
    }

    #[tokio::test]
    async fn idempotency_dedup_is_resolved_by_generated_machine_map() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("dedup-authority"));
        let key = IdempotencyKey::new("machine-owned-dedup");

        let mut first = prompt_input("first");
        let first_id = first.id().clone();
        if let Input::Prompt(prompt) = &mut first {
            prompt.header.idempotency_key = Some(key.clone());
        }
        let first_outcome = driver.accept_input(first).await.unwrap();
        assert!(first_outcome.is_accepted());

        driver.with_dsl_state(|state| {
            assert_eq!(
                state.admission_idempotency_inputs.get(&key.to_string()),
                Some(&first_id.to_string()),
                "generated machine state must own the idempotency key binding"
            );
        });

        let mut duplicate = prompt_input("second");
        let duplicate_id = duplicate.id().clone();
        if let Input::Prompt(prompt) = &mut duplicate {
            prompt.header.idempotency_key = Some(key.clone());
        }
        let duplicate_outcome = driver.accept_input(duplicate).await.unwrap();

        match duplicate_outcome {
            crate::accept::AcceptOutcome::Deduplicated {
                input_id,
                existing_id,
            } => {
                assert_eq!(input_id, duplicate_id);
                assert_eq!(existing_id, first_id);
            }
            other => panic!("expected generated deduplicated outcome, got {other:?}"),
        }
        assert!(
            driver.input_state(&duplicate_id).is_none(),
            "deduplicated inputs must not be admitted into the shell ledger"
        );
        driver.with_dsl_state(|state| {
            assert_eq!(
                state.admission_idempotency_inputs.get(&key.to_string()),
                Some(&first_id.to_string()),
                "duplicate resolution must not rewrite the generated key owner"
            );
        });
    }

    #[tokio::test]
    async fn admission_validation_rejection_class_is_generated() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("validation-authority"));

        let mut input = prompt_input("derived prompt");
        let input_id = input.id().clone();
        if let Input::Prompt(prompt) = &mut input {
            prompt.header.durability = InputDurability::Derived;
        }

        let generated_reason = driver
            .resolve_admission_validation(
                &input_id,
                AdmissionValidationFacts {
                    input_kind: input.kind(),
                    input_origin: &input.header().source,
                    durability: input.header().durability,
                    peer_handling_mode_valid: true,
                    peer_response_terminal_structurally_valid: true,
                    peer_response_terminal_observed_status:
                        mm_dsl::PeerResponseTerminalObservedStatus::NotPeerTerminal,
                },
            )
            .expect("generated validation feedback should resolve");
        assert_eq!(
            generated_reason,
            Some(mm_dsl::AdmissionRejectReasonKind::ExternalDerivedDurabilityForbidden),
            "derived operator prompt must reject on the external-derived rule"
        );

        let outcome = driver.accept_input(input).await.unwrap();
        match outcome {
            crate::accept::AcceptOutcome::Rejected {
                reason: crate::accept::RejectReason::DurabilityViolation { detail },
            } => {
                assert!(
                    !detail.is_empty(),
                    "shell detail should describe the raw validation error"
                );
            }
            other => panic!("expected generated rejection class, got {other:?}"),
        }
        assert!(
            driver.input_state(&input_id).is_none(),
            "rejected inputs must not enter the shell ledger"
        );
        driver.with_dsl_state(|state| {
            assert!(
                !state.input_phases.contains_key(&input_id.to_string()),
                "rejected inputs must not create lifecycle machine facts"
            );
        });
    }

    #[test]
    fn admission_validation_durability_reasons_are_machine_emitted() {
        use crate::identifiers::InputKind;
        use mm_dsl::AdmissionRejectReasonKind as Reason;

        let driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("validation-reasons"));

        let operator = InputOrigin::Operator;
        let system = InputOrigin::System;
        let flow = InputOrigin::Flow {
            flow_id: "flow-1".into(),
            step_index: 0,
        };
        let peer = InputOrigin::Peer {
            peer_id: "peer-1".into(),
            display_identity: None,
            runtime_id: None,
        };
        let external = InputOrigin::External {
            source_name: "webhook".into(),
        };

        let cases: Vec<(InputKind, &InputOrigin, InputDurability, Option<Reason>)> = vec![
            // External-ingress origins cannot submit derived inputs at all.
            (
                InputKind::Prompt,
                &operator,
                InputDurability::Derived,
                Some(Reason::ExternalDerivedDurabilityForbidden),
            ),
            (
                InputKind::PeerMessage,
                &peer,
                InputDurability::Derived,
                Some(Reason::ExternalDerivedDurabilityForbidden),
            ),
            (
                InputKind::ExternalEvent,
                &external,
                InputDurability::Derived,
                Some(Reason::ExternalDerivedDurabilityForbidden),
            ),
            (
                InputKind::Continuation,
                &operator,
                InputDurability::Derived,
                Some(Reason::ExternalDerivedDurabilityForbidden),
            ),
            // Internal origins may not derive these input kinds.
            (
                InputKind::Prompt,
                &system,
                InputDurability::Derived,
                Some(Reason::DerivedDurabilityForbiddenForInputKind),
            ),
            (
                InputKind::PeerMessage,
                &system,
                InputDurability::Derived,
                Some(Reason::DerivedDurabilityForbiddenForInputKind),
            ),
            (
                InputKind::PeerRequest,
                &system,
                InputDurability::Derived,
                Some(Reason::DerivedDurabilityForbiddenForInputKind),
            ),
            (
                InputKind::PeerResponseTerminal,
                &system,
                InputDurability::Derived,
                Some(Reason::DerivedDurabilityForbiddenForInputKind),
            ),
            (
                InputKind::FlowStep,
                &flow,
                InputDurability::Derived,
                Some(Reason::DerivedDurabilityForbiddenForInputKind),
            ),
            // Internal origins may derive reconstructable input kinds.
            (
                InputKind::PeerResponseProgress,
                &system,
                InputDurability::Derived,
                None,
            ),
            (
                InputKind::ExternalEvent,
                &system,
                InputDurability::Derived,
                None,
            ),
            (
                InputKind::Operation,
                &system,
                InputDurability::Derived,
                None,
            ),
            // Durable/Ephemeral are always authorized.
            (InputKind::Prompt, &operator, InputDurability::Durable, None),
            (
                InputKind::Prompt,
                &operator,
                InputDurability::Ephemeral,
                None,
            ),
        ];

        for (input_kind, input_origin, durability, expected) in cases {
            let input_id = InputId::new();
            let resolved = driver
                .resolve_admission_validation(
                    &input_id,
                    AdmissionValidationFacts {
                        input_kind,
                        input_origin,
                        durability,
                        peer_handling_mode_valid: true,
                        peer_response_terminal_structurally_valid: true,
                        peer_response_terminal_observed_status:
                            mm_dsl::PeerResponseTerminalObservedStatus::NotPeerTerminal,
                    },
                )
                .expect("generated validation must resolve");
            assert_eq!(
                resolved, expected,
                "machine-emitted reason mismatch for {input_kind:?}/{input_origin:?}/{durability:?}"
            );
        }
    }

    #[tokio::test]
    async fn consume_on_accept_terminal_outcome_is_machine_owned() {
        let mut driver =
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("consume-on-accept-terminal"));

        let input = operation_input();
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();

        match outcome {
            crate::accept::AcceptOutcome::Accepted { seed, .. } => {
                assert_eq!(seed.phase, InputLifecycleState::Consumed);
                assert_eq!(
                    seed.terminal_outcome,
                    Some(InputTerminalOutcome::Consumed),
                    "accepted result must project terminal outcome from generated machine state"
                );
            }
            other => panic!("expected consume-on-accept accepted outcome, got {other:?}"),
        }
        assert_eq!(
            driver.input_terminal_outcome(&input_id),
            Some(InputTerminalOutcome::Consumed)
        );
        driver.with_dsl_state(|state| {
            assert_eq!(
                state.input_terminal_kind.get(&input_id.to_string()),
                Some(&mm_dsl::InputTerminalKind::Consumed),
                "ConsumeOnAccept must write generated terminal kind"
            );
        });
    }

    #[tokio::test]
    async fn staged_rollback_retry_exhaustion_is_machine_owned() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("rollback-resolution"));

        let input = prompt_input("retry me");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        for attempt in 1..3 {
            let run_id = RunId::new();
            driver.stage_input(&input_id, &run_id).unwrap();
            assert_eq!(driver.input_attempt_count(&input_id), attempt);

            driver
                .rollback_staged(std::slice::from_ref(&input_id))
                .unwrap();
            assert_eq!(
                driver.input_phase(&input_id),
                Some(InputLifecycleState::Queued),
                "machine should requeue while generated retry policy has attempts remaining"
            );
            assert_eq!(driver.input_terminal_outcome(&input_id), None);
        }

        let final_run_id = RunId::new();
        driver.stage_input(&input_id, &final_run_id).unwrap();
        assert_eq!(driver.input_attempt_count(&input_id), 3);
        driver
            .rollback_staged(std::slice::from_ref(&input_id))
            .unwrap();

        assert_eq!(
            driver.input_phase(&input_id),
            Some(InputLifecycleState::Abandoned)
        );
        assert_eq!(
            driver.input_terminal_outcome(&input_id),
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
            })
        );
        assert!(!driver.has_queued_input(&input_id));
    }

    #[tokio::test]
    async fn missing_input_lifecycle_authority_fails_terminality_closed() {
        let mut driver =
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("missing-input-authority"));

        let input = prompt_input("missing authority");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();
        {
            let mut authority = driver.dsl.lock();
            let mut state = authority.state().clone();
            state.input_phases.remove(&input_id.to_string());
            *authority = super::recover_ingress_dsl_authority(state);
        }

        let err = driver
            .input_is_terminal_by_authority(&input_id)
            .expect_err("missing machine lifecycle authority must fail closed");
        assert!(
            matches!(&err, RuntimeDriverError::Internal(message) if message.contains("missing generated input lifecycle authority")),
            "unexpected terminality error: {err:?}"
        );
        assert!(
            driver.active_input_ids().is_empty(),
            "active-input projection must not fabricate non-terminal truth without generated authority"
        );

        let err = driver
            .stage_input(&input_id, &RunId::new())
            .expect_err("staging must not synthesize a queued phase without machine authority");
        assert!(
            matches!(&err, RuntimeDriverError::Internal(message) if message.contains("generated input lifecycle phase missing before staging")),
            "unexpected staging error: {err:?}"
        );
    }

    #[tokio::test]
    async fn cancelled_peer_response_terminal_rejects_and_cleans_pending_via_machine() {
        let mut driver =
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("cancelled-peer-terminal"));
        let peer_id = "550e8400-e29b-41d4-a716-446655440000";
        let request_uuid = uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f1").unwrap();
        let request_id = meerkat_core::PeerCorrelationId::from_uuid(request_uuid);
        driver
            .dsl_apply(
                mm_dsl::MeerkatMachineInput::PeerRequestSent {
                    corr_id: request_id.into(),
                },
                "PeerRequestSent(test)",
            )
            .unwrap();

        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: peer_id.into(),
                    display_identity: Some("Analyst".into()),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::ResponseTerminal {
                request_id: request_uuid.to_string(),
                status: meerkat_core::handles::PeerResponseTerminalProjectionStatus::Cancelled,
            }),
            content: meerkat_core::types::ContentInput::Text(String::new()),
            payload: Some(serde_json::json!({"ok": false})),
            handling_mode: None,
        });

        let outcome = driver.accept_input(input).await.unwrap();
        match outcome {
            crate::accept::AcceptOutcome::Rejected {
                reason: crate::accept::RejectReason::PeerResponseTerminalInvalid { detail },
            } => assert!(detail.contains("rejected by generated authority")),
            other => panic!("expected generated peer terminal rejection, got {other:?}"),
        }
        driver.with_dsl_state(|state| {
            assert!(
                !state
                    .pending_peer_requests
                    .contains_key(&mm_dsl::PeerCorrelationId::from(request_uuid)),
                "invalid terminal observation must clean pending peer truth through generated authority"
            );
        });
    }

    #[tokio::test]
    async fn abandon_all_non_terminal_projects_generated_terminal_outcome() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("abandon-projection"));
        let input = prompt_input("abandon me");
        let input_id = input.id().clone();
        driver.accept_input(input).await.unwrap();

        let abandoned = driver
            .abandon_all_non_terminal(InputAbandonReason::Stopped)
            .unwrap();

        assert_eq!(abandoned, 1);
        assert_eq!(
            driver.input_terminal_outcome(&input_id),
            Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::Stopped
            }),
            "generated machine projection is the only terminal-outcome owner"
        );
    }

    #[test]
    fn recovered_terminal_lifecycle_requires_terminal_witness() {
        let mut driver =
            EphemeralRuntimeDriver::new(LogicalRuntimeId::new("recover-terminal-witness"));
        let input_id = InputId::new();
        let seed = InputStateSeed {
            phase: InputLifecycleState::Consumed,
            last_run_id: None,
            last_boundary_sequence: None,
            admission_sequence: None,
            terminal_outcome: None,
            attempt_count: 0,
            recovery_lane: None,
        };

        let err = driver
            .recover_terminal_input_lifecycle(&input_id, &seed, None)
            .expect_err("terminal recovery without terminal outcome must fail closed");
        assert!(
            matches!(&err, RuntimeDriverError::Internal(message) if message.contains("behavioral input terminality")),
            "unexpected recovery error: {err:?}"
        );

        let generated_err = driver
            .dsl_apply(
                mm_dsl::MeerkatMachineInput::RecoverInputLifecycle {
                    input_id: input_id.to_string(),
                    phase: mm_dsl::InputPhase::Consumed,
                    terminal_kind: None,
                    superseded_by: None,
                    aggregate_id: None,
                    abandon_reason: None,
                    abandon_attempt_count: 0,
                    attempt_count: 0,
                    run_id: None,
                    boundary_sequence: None,
                    admission_sequence: None,
                    admission_sequence_recovery: None,
                    recovery_lane: None,
                    lane: None,
                },
                "RecoverInputLifecycle(test)",
            )
            .expect_err("generated recovery authority must reject missing terminal kind");
        assert!(
            matches!(&generated_err, RuntimeDriverError::Internal(message) if message.contains("RecoverInputLifecycle")),
            "unexpected generated recovery error: {generated_err:?}"
        );
    }

    #[test]
    fn recovered_max_attempts_terminal_reason_owns_attempt_payload() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("recover-max-attempts"));
        let input_id = InputId::new();
        let split_seed = InputStateSeed {
            phase: InputLifecycleState::Abandoned,
            last_run_id: None,
            last_boundary_sequence: None,
            admission_sequence: None,
            terminal_outcome: Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
            }),
            attempt_count: 2,
            recovery_lane: None,
        };

        let err = driver
            .recover_terminal_input_lifecycle(&input_id, &split_seed, None)
            .expect_err("max-attempts recovery must reject a split attempt witness");
        assert!(
            matches!(&err, RuntimeDriverError::Internal(message) if message.contains("RecoverInputLifecycle")),
            "unexpected recovery error: {err:?}"
        );

        let below_policy_seed = InputStateSeed {
            phase: InputLifecycleState::Abandoned,
            last_run_id: None,
            last_boundary_sequence: None,
            admission_sequence: None,
            terminal_outcome: Some(InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 2 },
            }),
            attempt_count: 2,
            recovery_lane: None,
        };

        let err = driver
            .recover_terminal_input_lifecycle(&input_id, &below_policy_seed, None)
            .expect_err("max-attempts recovery must reject attempts below machine policy");
        assert!(
            matches!(&err, RuntimeDriverError::Internal(message) if message.contains("RecoverInputLifecycle")),
            "unexpected recovery policy error: {err:?}"
        );

        let err = crate::meerkat_machine::authorize_stored_input_state_seed(
            &input_id,
            &below_policy_seed,
        )
        .expect_err("stored max-attempts seed must reject attempts below machine policy");
        assert!(
            err.contains("stored input-state seed"),
            "unexpected stored seed policy error: {err:?}"
        );
    }

    #[test]
    fn resolve_admission_uses_generated_machine_phase_not_control_projection() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("phase-drift"));
        force_control_shadow(
            &mut driver,
            RuntimeState::Running,
            Some(RunId::new()),
            Some(RuntimeState::Attached),
        );

        let input = peer_message_input();
        let projected = driver.resolve_admission(&input).unwrap();
        assert!(projected.requires_active_runtime_pre_admission());
        let flags = projected.coarse_flags();
        let (policy, _, _, _, _) = projected.into_parts();
        assert_eq!(policy.wake_mode, WakeMode::WakeIfIdle);
        assert!(!flags.interrupt_yielding);
        assert!(!flags.request_immediate_processing);
    }
}
