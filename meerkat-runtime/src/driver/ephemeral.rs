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
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::HandlingMode;

use crate::accept::{
    AcceptOutcome, AdmissionPlan, AdmissionQueueAction, ExistingQueuedAdmissionAction,
};
use crate::durability::{DurabilityError, validate_durability};
use crate::identifiers::LogicalRuntimeId;
use crate::ingress_types::{ContentShape, RequestId, ReservationKey};
use crate::input::Input;
use crate::input_ledger::InputLedger;
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStateHistoryEntry, InputStateSeed,
    InputTerminalOutcome, MAX_STAGE_ATTEMPTS, PolicySnapshot, StoredInputState,
};
use crate::meerkat_machine::dsl as mm_dsl;
use crate::policy::PolicyDecision;
use crate::queue::InputQueue;
use crate::runtime_event::{
    InputLifecycleEvent, RuntimeEvent, RuntimeEventEnvelope, RuntimeStateChangeEvent,
};
use crate::runtime_state::RuntimeState;
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

#[derive(Debug, Clone, Default)]
pub struct ReplayQueuedContributorsPlan {
    pub queue_work_ids: Vec<InputId>,
    pub steer_work_ids: Vec<InputId>,
    pub wake_runtime: bool,
    pub notice_kind: &'static str,
}

#[derive(Clone)]
pub(crate) struct EphemeralDriverRollbackSnapshot {
    control_projection: RuntimeControlProjection,
    dsl_state: mm_dsl::MeerkatMachineState,
    ledger: InputLedger,
    queue: InputQueue,
    steer_queue: InputQueue,
    events: Vec<RuntimeEventEnvelope>,
    post_admission_signal: PostAdmissionSignal,
    silent_comms_intents: Vec<String>,
    handling_mode: HashMap<InputId, HandlingMode>,
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
    silent_comms_intents: Vec<String>,
    /// Shared session-owned DSL authority for ingress semantics (queue/steer
    /// lanes, input phases, admission ordering).
    dsl: DslAuthority,
    /// Per-input admission metadata with no DSL home (content shape,
    /// correlation IDs, policy snapshot, handling mode). These are pure
    /// shell mechanics — they feed observability and queue routing, never
    /// decide semantics.
    handling_mode: HashMap<InputId, HandlingMode>,
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
    let state = mm_dsl::MeerkatMachineState {
        lifecycle_phase: mm_dsl::MeerkatPhase::Idle,
        ..mm_dsl::MeerkatMachineState::default()
    };
    Arc::new(Mutex::new(mm_dsl::MeerkatMachineAuthority::from_state(
        state,
    )))
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
            silent_comms_intents: Vec::new(),
            dsl: DslAuthority(dsl),
            handling_mode: HashMap::new(),
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
            dsl_state: self.with_dsl_state(Clone::clone),
            ledger: self.ledger.clone(),
            queue: self.queue.clone(),
            steer_queue: self.steer_queue.clone(),
            events: self.events.clone(),
            post_admission_signal: self.post_admission_signal,
            silent_comms_intents: self.silent_comms_intents.clone(),
            handling_mode: self.handling_mode.clone(),
            is_prompt_set: self.is_prompt_set.clone(),
            content_shape: self.content_shape.clone(),
            request_id: self.request_id.clone(),
            reservation_key: self.reservation_key.clone(),
            policy_snapshot: self.policy_snapshot.clone(),
            admission_order: self.admission_order.clone(),
        }
    }

    pub(crate) fn restore_rollback_snapshot(&mut self, snapshot: EphemeralDriverRollbackSnapshot) {
        {
            let mut control = self.write_control_projection();
            *control = snapshot.control_projection;
        }
        {
            let mut authority = self.dsl.lock();
            authority.state = snapshot.dsl_state;
        }
        self.ledger = snapshot.ledger;
        self.queue = snapshot.queue;
        self.steer_queue = snapshot.steer_queue;
        self.events = snapshot.events;
        self.post_admission_signal = snapshot.post_admission_signal;
        self.silent_comms_intents = snapshot.silent_comms_intents;
        self.handling_mode = snapshot.handling_mode;
        self.is_prompt_set = snapshot.is_prompt_set;
        self.content_shape = snapshot.content_shape;
        self.request_id = snapshot.request_id;
        self.reservation_key = snapshot.reservation_key;
        self.policy_snapshot = snapshot.policy_snapshot;
        self.admission_order = snapshot.admission_order;
    }

    #[cfg(test)]
    pub(crate) fn shared_dsl_authority(&self) -> SharedIngressDslAuthority {
        Arc::clone(&self.dsl.0)
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
        let transition = {
            let mut authority = self.dsl.lock();
            mm_dsl::MeerkatMachineMutator::apply(&mut *authority, input).map_err(|err| {
                RuntimeDriverError::Internal(format!("DSL rejected {context}: {err:?}"))
            })?
        };
        self.absorb_dsl_effects(&transition.effects);
        Ok(())
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
                let new_signal = match signal.as_str() {
                    "WakeLoop" => PostAdmissionSignal::WakeLoop,
                    "InterruptYielding" => PostAdmissionSignal::InterruptYielding,
                    "RequestImmediateProcessing" => PostAdmissionSignal::RequestImmediateProcessing,
                    _ => continue,
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
        body(&authority.state)
    }

    fn with_dsl_state_mut<R>(
        &mut self,
        body: impl FnOnce(&mut mm_dsl::MeerkatMachineState) -> R,
    ) -> R {
        let mut authority = self.dsl.lock();
        body(&mut authority.state)
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

    /// Read the attempt count for an input from the DSL.
    pub fn input_attempt_count(&self, input_id: &InputId) -> u32 {
        let key = Self::dsl_key(input_id);
        self.with_dsl_state(|state| state.input_attempt_counts.get(&key).copied().unwrap_or(0))
            as u32
    }

    // ---- Admission metadata accessors (read-only) ----

    /// The admission order (insertion order of persisted-queue admissions).
    pub fn admission_order(&self) -> &[InputId] {
        &self.admission_order
    }

    /// Policy snapshot captured at admission time for a specific input.
    pub fn admitted_policy(&self, input_id: &InputId) -> Option<&PolicyDecision> {
        self.policy_snapshot.get(input_id)
    }

    /// Content shape captured at admission time.
    pub fn admitted_content_shape(&self, input_id: &InputId) -> Option<ContentShape> {
        self.content_shape.get(input_id).cloned()
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

    pub fn set_silent_comms_intents(&mut self, intents: Vec<String>) {
        if self.control_snapshot().phase == RuntimeState::Stopped {
            return;
        }
        self.silent_comms_intents = intents;
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.silent_comms_intents.clone()
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
    /// Important: this only seeds canonical ingress truth. The caller restores
    /// the recovered `InputState` into the ledger before this call, so we must
    /// not rebuild the physical queue projections here. Rebuilding before the
    /// full recovery batch is loaded would make queue projection checks observe
    /// transient partial state.
    ///
    /// Recovery directly seeds the DSL state: DSL transitions are designed
    /// around admit-and-advance flow, but a recovered input may be at any
    /// point in its lifecycle. Direct state seeding mirrors the pattern used
    /// by `ops_lifecycle::from_recovered` for the same reason.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_recovered_to_ingress(
        &mut self,
        work_id: InputId,
        content_shape: ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        recovered_state: &InputState,
        recovered_seed: &InputStateSeed,
        policy: PolicyDecision,
        request_id: Option<RequestId>,
        reservation_key: Option<ReservationKey>,
    ) -> Result<(), RuntimeDriverError> {
        self.seed_recovered_input(
            &work_id,
            &content_shape,
            handling_mode,
            is_prompt,
            recovered_state,
            recovered_seed,
            &policy,
            request_id.as_ref(),
            reservation_key.as_ref(),
        );
        Ok(())
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

    /// Seed driver state for a recovered input. No DSL transitions fire —
    /// this path directly writes the post-recovery phase into the DSL state
    /// from the deserialized seed, which the caller obtained from the store.
    #[allow(clippy::too_many_arguments)]
    fn seed_recovered_input(
        &mut self,
        work_id: &InputId,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        _recovered_state: &InputState,
        recovered_seed: &InputStateSeed,
        policy: &PolicyDecision,
        request_id: Option<&RequestId>,
        reservation_key: Option<&ReservationKey>,
    ) {
        let key = Self::dsl_key(work_id);
        let lifecycle_state = recovered_seed.phase;
        self.record_admission_metadata(
            work_id,
            content_shape,
            handling_mode,
            is_prompt,
            policy,
            request_id,
            reservation_key,
        );
        self.with_dsl_state_mut(|state| {
            state
                .input_phases
                .insert(key.clone(), Self::lifecycle_to_input_phase(lifecycle_state));
        });
        match recovered_seed.terminal_outcome.clone() {
            Some(InputTerminalOutcome::Consumed) => {
                self.with_dsl_state_mut(|state| {
                    state
                        .input_terminal_kind
                        .insert(key.clone(), mm_dsl::InputTerminalKind::Consumed);
                    state.input_superseded_by.remove(&key);
                    state.input_aggregate_id.remove(&key);
                    state.input_abandon_reason.remove(&key);
                    state.input_abandon_attempt_count.remove(&key);
                });
            }
            Some(InputTerminalOutcome::Superseded { superseded_by }) => {
                self.with_dsl_state_mut(|state| {
                    state
                        .input_terminal_kind
                        .insert(key.clone(), mm_dsl::InputTerminalKind::Superseded);
                    state
                        .input_superseded_by
                        .insert(key.clone(), superseded_by.to_string());
                    state.input_aggregate_id.remove(&key);
                    state.input_abandon_reason.remove(&key);
                    state.input_abandon_attempt_count.remove(&key);
                });
            }
            Some(InputTerminalOutcome::Coalesced { aggregate_id }) => {
                self.with_dsl_state_mut(|state| {
                    state
                        .input_terminal_kind
                        .insert(key.clone(), mm_dsl::InputTerminalKind::Coalesced);
                    state
                        .input_aggregate_id
                        .insert(key.clone(), aggregate_id.to_string());
                    state.input_superseded_by.remove(&key);
                    state.input_abandon_reason.remove(&key);
                    state.input_abandon_attempt_count.remove(&key);
                });
            }
            Some(InputTerminalOutcome::Abandoned { reason }) => {
                self.with_dsl_state_mut(|state| {
                    state
                        .input_terminal_kind
                        .insert(key.clone(), mm_dsl::InputTerminalKind::Abandoned);
                    state
                        .input_abandon_reason
                        .insert(key.clone(), mm_dsl::InputAbandonReason::from(&reason));
                    state
                        .input_abandon_attempt_count
                        .insert(key.clone(), u64::from(recovered_seed.attempt_count));
                    state.input_superseded_by.remove(&key);
                    state.input_aggregate_id.remove(&key);
                });
            }
            None => {
                self.with_dsl_state_mut(|state| {
                    state.input_terminal_kind.remove(&key);
                    state.input_superseded_by.remove(&key);
                    state.input_aggregate_id.remove(&key);
                    state.input_abandon_reason.remove(&key);
                    state.input_abandon_attempt_count.remove(&key);
                });
            }
        }
        self.with_dsl_state_mut(|state| {
            state
                .input_attempt_counts
                .insert(key.clone(), u64::from(recovered_seed.attempt_count));
        });
        match recovered_seed.last_run_id.clone() {
            Some(run_id) => {
                self.with_dsl_state_mut(|state| {
                    state
                        .input_run_associations
                        .insert(key.clone(), run_id.to_string());
                });
            }
            None => {
                self.with_dsl_state_mut(|state| {
                    state.input_run_associations.remove(&key);
                });
            }
        }
        match recovered_seed.last_boundary_sequence {
            Some(seq) => {
                self.with_dsl_state_mut(|state| {
                    state.input_boundary_sequences.insert(key.clone(), seq);
                });
            }
            None => {
                self.with_dsl_state_mut(|state| {
                    state.input_boundary_sequences.remove(&key);
                });
            }
        }
        self.with_dsl_state_mut(|state| {
            if !state.input_admission_seq.contains_key(&key) {
                let seq = state.next_admission_seq;
                state.input_admission_seq.insert(key.clone(), seq);
                state.next_admission_seq = seq.saturating_add(1);
            }
        });
        // Recovery seeding is a direct DSL-state write from the persisted
        // snapshot by design — there are no transitions to replay. Only
        // Queued inputs carry lane membership; staged/terminal inputs are
        // not in any work lane.
        self.with_dsl_state_mut(|state| {
            state.input_lane.remove(&key);
            if matches!(lifecycle_state, InputLifecycleState::Queued) {
                state
                    .input_lane
                    .insert(key, mm_dsl::InputLane::from(handling_mode));
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn record_admission_metadata(
        &mut self,
        work_id: &InputId,
        content_shape: &ContentShape,
        handling_mode: HandlingMode,
        is_prompt: bool,
        policy: &PolicyDecision,
        request_id: Option<&RequestId>,
        reservation_key: Option<&ReservationKey>,
    ) {
        if !self.admission_order.contains(work_id) {
            self.admission_order.push(work_id.clone());
        }
        self.content_shape
            .insert(work_id.clone(), content_shape.clone());
        self.handling_mode.insert(work_id.clone(), handling_mode);
        if is_prompt {
            self.is_prompt_set.insert(work_id.clone());
        } else {
            self.is_prompt_set.remove(work_id);
        }
        self.request_id.insert(work_id.clone(), request_id.cloned());
        self.reservation_key
            .insert(work_id.clone(), reservation_key.cloned());
        self.policy_snapshot.insert(work_id.clone(), policy.clone());
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
        is_prompt: bool,
        policy: &PolicyDecision,
        queue_action: AdmissionQueueAction,
        existing_action: Option<&ExistingQueuedAdmissionAction>,
    ) -> Result<(), RuntimeDriverError> {
        // 1. Persist admission metadata on the shell side.
        self.record_admission_metadata(
            input_id,
            content_shape,
            handling_mode,
            is_prompt,
            policy,
            None,
            None,
        );

        // 2. Persist the input payload on the ledger and record the
        //    history transition. The DSL apply below is the authoritative
        //    phase writer; the shell only carries the history/metadata cache.
        let now = Utc::now();
        if let Some(s) = self.ledger.get_mut(input_id) {
            s.persisted_input = Some(input.clone());
            s.history.push(InputStateHistoryEntry {
                timestamp: now,
                from: InputLifecycleState::Accepted,
                to: InputLifecycleState::Queued,
                reason: Some("QueueAccepted".into()),
            });
            s.updated_at = now;
        }

        // 3. DSL phase transition (authoritative). Admission lane mirrors
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

        // 4. Handle supersession / coalescing of an existing queued input.
        //    The new input goes into the queue (step 5); the existing one
        //    transitions to its terminal state here. The DSL transitions
        //    own lane removal.
        if let Some(action) = existing_action {
            match action {
                ExistingQueuedAdmissionAction::Coalesce { existing_id } => {
                    let existing_key = Self::dsl_key(existing_id);
                    let aggregate_key = Self::dsl_key(input_id);
                    let from_phase = self
                        .input_phase(existing_id)
                        .unwrap_or(InputLifecycleState::Queued);
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::CoalesceInput {
                            input_id: existing_key,
                            aggregate_id: aggregate_key,
                        },
                        "CoalesceInput",
                    )?;
                    let _ = self.queue.remove(existing_id);
                    let _ = self.steer_queue.remove(existing_id);
                    if let Some(existing_state) = self.ledger.get_mut(existing_id) {
                        crate::coalescing::apply_coalescing(
                            existing_state,
                            from_phase,
                            input_id.clone(),
                        );
                    }
                }
                ExistingQueuedAdmissionAction::Supersede { existing_id } => {
                    let existing_key = Self::dsl_key(existing_id);
                    let superseded_by = Self::dsl_key(input_id);
                    let from_phase = self
                        .input_phase(existing_id)
                        .unwrap_or(InputLifecycleState::Queued);
                    self.dsl_apply(
                        mm_dsl::MeerkatMachineInput::SupersedeInput {
                            input_id: existing_key,
                            superseded_by,
                        },
                        "SupersedeInput",
                    )?;
                    let _ = self.queue.remove(existing_id);
                    let _ = self.steer_queue.remove(existing_id);
                    if let Some(existing_state) = self.ledger.get_mut(existing_id) {
                        crate::coalescing::apply_supersession(
                            existing_state,
                            from_phase,
                            input_id.clone(),
                        );
                    }
                }
            }
        }

        // 5. Enqueue the new input into the correct lane. When the
        //    queue_action's target differs from the admission lane (e.g.
        //    priority reroute), the shell emits a `ChangeLane` transition
        //    rather than writing `input_lane` directly.
        match queue_action {
            AdmissionQueueAction::None => {}
            AdmissionQueueAction::EnqueueTo { target } => {
                match target {
                    HandlingMode::Queue => self.queue.enqueue(input_id.clone(), input.clone()),
                    HandlingMode::Steer => {
                        self.steer_queue.enqueue(input_id.clone(), input.clone());
                    }
                }
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
                match target {
                    HandlingMode::Queue => {
                        self.queue.enqueue_front(input_id.clone(), input.clone());
                    }
                    HandlingMode::Steer => {
                        self.steer_queue
                            .enqueue_front(input_id.clone(), input.clone());
                    }
                }
                let key = Self::dsl_key(input_id);
                // Priority routing: reassign the admission sequence so this
                // input sorts ahead of existing entries in the lane.
                self.with_dsl_state_mut(|state| {
                    let min_seq = state
                        .input_admission_seq
                        .values()
                        .min()
                        .copied()
                        .unwrap_or(0);
                    state
                        .input_admission_seq
                        .insert(key.clone(), min_seq.saturating_sub(1));
                });
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

        self.emit_event(RuntimeEvent::InputLifecycle(InputLifecycleEvent::Queued {
            input_id: input_id.clone(),
        }));

        Ok(())
    }

    pub fn is_idle(&self) -> bool {
        self.control_snapshot().phase == RuntimeState::Idle
    }
    pub fn is_idle_or_attached(&self) -> bool {
        self.control_snapshot().phase.is_idle_or_attached()
    }

    pub fn phase(&self) -> RuntimeState {
        self.control_snapshot().phase
    }

    pub fn current_run_id(&self) -> Option<RunId> {
        self.control_snapshot().current_run_id
    }

    pub fn pre_run_phase(&self) -> Option<RuntimeState> {
        self.control_snapshot().pre_run_phase
    }

    pub fn can_process_queue(&self) -> bool {
        self.control_snapshot().phase.can_process_queue()
    }

    /// Low-level control projection shim for external contract tests.
    ///
    /// This does not decide lifecycle legality; it only applies an already
    /// chosen MeerkatMachine control projection to the concrete driver shell.
    #[doc(hidden)]
    pub fn contract_set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.set_control_projection(next_phase, current_run_id, pre_run_phase);
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
    /// boundary sequence). Used by persistence callsites and test helpers.
    pub fn stored_input_state(&self, input_id: &InputId) -> Option<StoredInputState> {
        let state = self.ledger.get(input_id)?.clone();
        let phase = self
            .input_phase(input_id)
            .unwrap_or(InputLifecycleState::Accepted);
        let seed = InputStateSeed {
            phase,
            last_run_id: self.input_last_run_id(input_id),
            last_boundary_sequence: self.input_last_boundary_sequence(input_id),
            terminal_outcome: self.input_terminal_outcome(input_id),
            attempt_count: self.input_attempt_count(input_id),
        };
        Some(StoredInputState { state, seed })
    }

    /// Snapshot of every ledger entry paired with its DSL seed.
    pub fn stored_input_states_snapshot(&self) -> Vec<StoredInputState> {
        self.ledger
            .iter()
            .map(|(input_id, state)| {
                let phase = self
                    .input_phase(input_id)
                    .unwrap_or(InputLifecycleState::Accepted);
                let seed = InputStateSeed {
                    phase,
                    last_run_id: self.input_last_run_id(input_id),
                    last_boundary_sequence: self.input_last_boundary_sequence(input_id),
                    terminal_outcome: self.input_terminal_outcome(input_id),
                    attempt_count: self.input_attempt_count(input_id),
                };
                StoredInputState {
                    state: state.clone(),
                    seed,
                }
            })
            .collect()
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

    /// Look up the persisted input for a given ID (from the ledger).
    #[allow(dead_code)] // Used by runtime_loop boundary classification via authority
    pub fn persisted_input(&self, input_id: &InputId) -> Option<&Input> {
        self.ledger
            .get(input_id)
            .and_then(|state| state.persisted_input.as_ref())
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
            let from_phase = self
                .input_phase(input_id)
                .unwrap_or(InputLifecycleState::Queued);
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
                state.attempt_count = state.attempt_count.saturating_add(1);
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
        let from_phase = self
            .input_phase(input_id)
            .unwrap_or(InputLifecycleState::Staged);
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

    pub fn consume_inputs(
        &mut self,
        input_ids: &[InputId],
        run_id: &RunId,
    ) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            let phase = self.input_phase(input_id);
            if phase != Some(InputLifecycleState::AppliedPendingConsumption) {
                continue;
            }
            let from_phase = phase.unwrap_or(InputLifecycleState::AppliedPendingConsumption);

            let key = Self::dsl_key(input_id);
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::ConsumeInput { input_id: key },
                "ConsumeInput",
            )?;

            let now = Utc::now();
            if let Some(state) = self.ledger.get_mut(input_id) {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: from_phase,
                    to: InputLifecycleState::Consumed,
                    reason: Some("Consume".into()),
                });
                state.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                state.updated_at = now;
            }
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

    pub fn rollback_staged(&mut self, input_ids: &[InputId]) -> Result<(), RuntimeDriverError> {
        for input_id in input_ids {
            // Skip inputs that are no longer in Staged (terminal or never-staged).
            if self.input_phase(input_id) != Some(InputLifecycleState::Staged) {
                continue;
            }
            let Some(_state) = self.ledger.get(input_id) else {
                continue;
            };

            let attempts = self.input_attempt_count(input_id);
            let key = Self::dsl_key(input_id);

            if attempts >= MAX_STAGE_ATTEMPTS {
                // Max stage attempts exhausted — Abandon instead of
                // RollbackStaged.
                let attempts_count = u64::from(attempts);
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::AbandonInput {
                        input_id: key.clone(),
                        reason: mm_dsl::InputAbandonReason::MaxAttemptsExhausted,
                        attempt_count: attempts_count,
                    },
                    "AbandonInput(MaxAttemptsExhausted)",
                )?;

                let now = Utc::now();
                tracing::warn!(
                    input_id = %input_id,
                    attempts,
                    "input abandoned after max stage attempts"
                );
                let outcome = InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::MaxAttemptsExhausted { attempts },
                };
                if let Some(state) = self.ledger.get_mut(input_id) {
                    state.history.push(InputStateHistoryEntry {
                        timestamp: now,
                        from: InputLifecycleState::Staged,
                        to: InputLifecycleState::Abandoned,
                        reason: Some(format!("RollbackStaged→Abandon(attempts={attempts})")),
                    });
                    state.terminal_outcome = Some(outcome.clone());
                    state.updated_at = now;
                }
                self.events.push(
                    self.make_envelope(RuntimeEvent::InputLifecycle(
                        InputLifecycleEvent::Abandoned {
                            input_id: input_id.clone(),
                            reason: mm_dsl::InputAbandonReason::MaxAttemptsExhausted
                                .as_str()
                                .to_string(),
                        },
                    )),
                );
            } else {
                // Still have attempts — rollback to Queued. The shell
                // passes the input's recorded `HandlingMode` so
                // `RollbackStaged` can re-admit it to the correct lane.
                let lane = self
                    .handling_mode
                    .get(input_id)
                    .copied()
                    .map(mm_dsl::InputLane::from)
                    .unwrap_or(mm_dsl::InputLane::Queue);
                self.dsl_apply(
                    mm_dsl::MeerkatMachineInput::RollbackStaged {
                        input_id: key,
                        lane,
                    },
                    "RollbackStaged",
                )?;

                let now = Utc::now();
                if let Some(state) = self.ledger.get_mut(input_id) {
                    state.history.push(InputStateHistoryEntry {
                        timestamp: now,
                        from: InputLifecycleState::Staged,
                        to: InputLifecycleState::Queued,
                        reason: Some("RollbackStaged".into()),
                    });
                    state.updated_at = now;
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
            .filter(|(id, _)| {
                self.input_phase(id)
                    .map(|phase| !phase.is_terminal())
                    .unwrap_or(false)
            })
            .count();
        RetireReport {
            inputs_abandoned: 0,
            inputs_pending_drain,
        }
    }

    /// Low-level retire realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the control projection first.
    #[doc(hidden)]
    pub fn contract_finalize_retire(&mut self) -> RetireReport {
        self.finalize_retire()
    }

    pub(crate) fn reset_cleanup(&mut self) -> ResetReport {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Reset);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.silent_comms_intents.clear();
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        ResetReport {
            inputs_abandoned: abandoned,
        }
    }

    /// Low-level reset realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the post-reset control projection.
    #[doc(hidden)]
    pub fn contract_reset_cleanup(&mut self) -> ResetReport {
        self.reset_cleanup()
    }

    pub(crate) fn destroy_cleanup(&mut self) -> usize {
        let abandoned = self.abandon_all_non_terminal(InputAbandonReason::Destroyed);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.silent_comms_intents.clear();
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        abandoned
    }

    /// Low-level destroy realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the destroyed control projection.
    #[doc(hidden)]
    pub fn contract_destroy_cleanup(&mut self) -> usize {
        self.destroy_cleanup()
    }

    pub(crate) fn stop_runtime_cleanup(&mut self) {
        self.abandon_all_non_terminal(InputAbandonReason::Stopped);
        self.silent_comms_intents.clear();
        self.queue.drain();
        self.steer_queue.drain();
    }

    pub(crate) fn finalize_stop_runtime(&mut self) {
        self.stop_runtime_cleanup();
    }

    /// Low-level stop realization shim for external contract tests.
    ///
    /// Callers are responsible for setting the stopped control projection.
    #[doc(hidden)]
    pub fn contract_finalize_stop_runtime(&mut self) {
        self.finalize_stop_runtime();
    }

    pub fn recover_ephemeral(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        crate::meerkat_machine::machine_recover_ephemeral_driver(self)
    }

    pub fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        let transferred = self
            .ledger
            .iter()
            .filter(|(id, _)| {
                self.input_phase(id)
                    .map(|phase| !phase.is_terminal())
                    .unwrap_or(false)
            })
            .count();
        let runtime_id = self.runtime_id.clone();
        let silent_comms_intents = self.silent_comms_intents.clone();
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
        self.silent_comms_intents = silent_comms_intents;
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

    pub(crate) fn resolve_admission_for_runtime_idle(
        &self,
        input: &Input,
        runtime_idle: bool,
    ) -> crate::accept::ResolvedAdmission {
        let existing_superseded_id = self.existing_superseded_input(input).map(|(id, _)| id);
        crate::accept::resolve_admission(
            input,
            runtime_idle,
            &self.silent_comms_intents,
            existing_superseded_id,
        )
    }

    pub(crate) fn resolve_admission(&self, input: &Input) -> crate::accept::ResolvedAdmission {
        let runtime_idle = self.read_control_projection().phase.is_idle_or_attached();
        self.resolve_admission_for_runtime_idle(input, runtime_idle)
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self.read_control_projection().phase {
            RuntimeState::Retired | RuntimeState::Stopped => {
                return Err(RuntimeDriverError::NotReady {
                    state: self.read_control_projection().phase,
                });
            }
            RuntimeState::Destroyed => return Err(RuntimeDriverError::Destroyed),
            RuntimeState::Initializing
            | RuntimeState::Idle
            | RuntimeState::Attached
            | RuntimeState::Running => {}
        }

        if let Err(e) = validate_durability(&input) {
            match e {
                DurabilityError::DerivedForbidden { .. }
                | DurabilityError::ExternalDerivedForbidden => {
                    return Ok(AcceptOutcome::Rejected {
                        reason: crate::accept::RejectReason::DurabilityViolation {
                            detail: e.to_string(),
                        },
                    });
                }
            }
        }
        if let Err(e) = crate::peer_handling_mode::validate_peer_handling_mode(&input) {
            return Ok(AcceptOutcome::Rejected {
                reason: crate::accept::RejectReason::PeerHandlingModeInvalid {
                    detail: e.to_string(),
                },
            });
        }

        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.durability = Some(input.header().durability);
        state.idempotency_key = input.header().idempotency_key.clone();
        if let Some(ref key) = input.header().idempotency_key {
            if let Some(existing_id) = self
                .ledger
                .accept_with_idempotency(state.clone(), key.clone())
            {
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
        } else {
            self.ledger.accept(state.clone());
        }

        if let Some(s) = self.ledger.get_mut(&input_id) {
            s.policy = Some(PolicySnapshot {
                version: resolved.policy.policy_version,
                decision: resolved.policy.clone(),
            });
        }
        self.emit_event(RuntimeEvent::InputLifecycle(
            InputLifecycleEvent::Accepted {
                input_id: input_id.clone(),
            },
        ));

        let runtime_idle = self.read_control_projection().phase.is_idle_or_attached();
        let handling_mode = resolved.handling_mode;
        let content_shape = ContentShape(input.kind_id().to_string());
        let is_prompt = matches!(input, Input::Prompt(_));
        match resolved.admission_plan {
            AdmissionPlan::ConsumedOnAccept => {
                self.record_admission_metadata(
                    &input_id,
                    &content_shape,
                    handling_mode,
                    is_prompt,
                    &resolved.policy,
                    None,
                    None,
                );
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
                let now = Utc::now();
                if let Some(s) = self.ledger.get_mut(&input_id) {
                    s.history.push(InputStateHistoryEntry {
                        timestamp: now,
                        from: InputLifecycleState::Accepted,
                        to: InputLifecycleState::Consumed,
                        reason: Some("ConsumeOnAccept (Ignore+OnAccept)".into()),
                    });
                    s.terminal_outcome = Some(InputTerminalOutcome::Consumed);
                    s.updated_at = now;
                }
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
                        is_prompt,
                        &resolved.policy,
                        queue_action,
                        existing_action.as_ref(),
                    )?;
                }
            }
        }
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();

        if !runtime_idle
            && matches!(resolved.policy.wake_mode, crate::WakeMode::WakeIfIdle)
            && PostAdmissionSignal::WakeLoop > self.post_admission_signal
        {
            self.post_admission_signal = PostAdmissionSignal::WakeLoop;
        }

        let final_state = self.ledger.get(&input_id).cloned().unwrap_or(state);
        Ok(AcceptOutcome::Accepted {
            input_id,
            policy: resolved.policy,
            state: final_state,
        })
    }

    pub fn abandon_all_non_terminal(&mut self, reason: InputAbandonReason) -> usize {
        let non_terminal_ids: Vec<InputId> = self
            .ledger
            .iter()
            .filter_map(|(id, _)| {
                if self
                    .input_phase(id)
                    .map(|phase| !phase.is_terminal())
                    .unwrap_or(false)
                {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect();
        let dsl_reason = mm_dsl::InputAbandonReason::from(&reason);
        let reason_label = dsl_reason.as_str();
        let mut count = 0;
        for id in &non_terminal_ids {
            let key = Self::dsl_key(id);
            let attempt_count = u64::from(self.input_attempt_count(id));
            let from_phase = self
                .input_phase(id)
                .unwrap_or(InputLifecycleState::Accepted);
            if self
                .dsl_apply(
                    mm_dsl::MeerkatMachineInput::AbandonInput {
                        input_id: key.clone(),
                        reason: dsl_reason,
                        attempt_count,
                    },
                    "AbandonInput",
                )
                .is_err()
            {
                continue;
            }

            let now = Utc::now();
            if let Some(state) = self.ledger.get_mut(id) {
                state.history.push(InputStateHistoryEntry {
                    timestamp: now,
                    from: from_phase,
                    to: InputLifecycleState::Abandoned,
                    reason: Some(format!("Abandon({reason_label})")),
                });
                state.terminal_outcome = Some(InputTerminalOutcome::Abandoned {
                    reason: reason.clone(),
                });
                state.updated_at = now;
            }
            count += 1;
            self.events
                .push(self.make_envelope(RuntimeEvent::InputLifecycle(
                    InputLifecycleEvent::Abandoned {
                        input_id: id.clone(),
                        reason: reason_label.to_string(),
                    },
                )));
        }
        count
    }

    pub fn abandon_pending_inputs(&mut self, reason: InputAbandonReason) -> usize {
        let abandoned = self.abandon_all_non_terminal(reason);
        self.queue.drain();
        self.steer_queue.drain();
        self.post_admission_signal = PostAdmissionSignal::None;
        self.rebuild_queue_projections();
        self.debug_assert_queue_projection_alignment();
        abandoned
    }

    pub async fn run_completed(
        &mut self,
        run_id: RunId,
        consumed_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_completed(&run_id, &consumed_input_ids)
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

    pub async fn run_failed(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        replay_plan: ReplayQueuedContributorsPlan,
        _error: String,
        _recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_run_failed(&run_id, &contributing_input_ids, &replay_plan)
    }

    /// Machine-owned realization for a validated failed-run replay plan.
    pub(crate) fn machine_realize_run_failed(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
        replay_plan: &ReplayQueuedContributorsPlan,
    ) -> Result<(), RuntimeDriverError> {
        if replay_plan.wake_runtime && self.post_admission_signal < PostAdmissionSignal::WakeLoop {
            self.post_admission_signal = PostAdmissionSignal::WakeLoop;
        }
        tracing::debug!(
            run_id = ?run_id,
            kind = replay_plan.notice_kind,
            queue = replay_plan.queue_work_ids.len(),
            steer = replay_plan.steer_work_ids.len(),
            "runtime replayed queued contributors"
        );

        // `rollback_staged` drives the DSL `RollbackStaged` / `AbandonInput`
        // transitions and re-inserts surviving contributors into the correct
        // lane — no separate lane seeding is needed here.
        self.rollback_staged(contributing_input_ids)
    }

    pub async fn boundary_applied(
        &mut self,
        run_id: RunId,
        receipt: RunBoundaryReceipt,
        _session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        self.machine_realize_boundary_applied(&run_id, &receipt)
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
            self.dsl_apply(
                mm_dsl::MeerkatMachineInput::RecordBoundarySeq {
                    input_id: key,
                    seq: receipt.sequence,
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
        let resolved = self.resolve_admission(&input);
        self.accept_resolved_input(input, resolved).await
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
        self.control_snapshot().phase
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
            .filter_map(|(id, _)| match self.input_phase(id) {
                Some(phase) if !phase.is_terminal() => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::EphemeralRuntimeDriver;
    use crate::identifiers::LogicalRuntimeId;
    use crate::input::{
        Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention,
        PeerInput,
    };
    use crate::{RuntimeState, WakeMode};
    use chrono::Utc;
    use meerkat_core::lifecycle::{InputId, RunId};

    fn peer_message_input() -> Input {
        Input::Peer(PeerInput {
            header: InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: InputOrigin::Peer {
                    peer_id: "peer-1".into(),
                    runtime_id: None,
                },
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            body: "peer body".into(),
            payload: None,
            blocks: None,
            handling_mode: None,
        })
    }

    #[test]
    fn resolve_admission_for_runtime_idle_uses_explicit_machine_phase_not_control_projection() {
        let mut driver = EphemeralRuntimeDriver::new(LogicalRuntimeId::new("phase-drift"));
        driver.set_control_projection(
            RuntimeState::Running,
            Some(RunId::new()),
            Some(RuntimeState::Attached),
        );

        let input = peer_message_input();
        let projected = driver.resolve_admission(&input);
        assert_eq!(projected.policy.wake_mode, WakeMode::InterruptYielding);
        assert!(projected.coarse_flags.interrupt_yielding);

        let machine_owned = driver.resolve_admission_for_runtime_idle(&input, true);
        assert_eq!(machine_owned.policy.wake_mode, WakeMode::WakeIfIdle);
        assert!(!machine_owned.coarse_flags.interrupt_yielding);
        assert!(!machine_owned.coarse_flags.request_immediate_processing);
    }
}
