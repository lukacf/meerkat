//! MeerkatMachine DSL definition with real bridging types.
use meerkat_machine_dsl::machine;

trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}

impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Bridging types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionId {
    pub fn from_domain(id: &meerkat_core::types::SessionId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct AgentRuntimeId(pub String);

impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl AgentRuntimeId {
    pub fn from_domain(id: &crate::identifiers::LogicalRuntimeId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl FenceToken {
    pub fn from_domain(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl Generation {
    pub fn from_domain(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RunId(pub String);

impl<T: Into<String>> From<T> for RunId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl RunId {
    pub fn from_domain(id: &meerkat_core::lifecycle::RunId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct InputId(pub String);

impl<T: Into<String>> From<T> for InputId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl InputId {
    pub fn from_domain(id: &meerkat_core::lifecycle::InputId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkId(pub String);

impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl WorkId {
    pub fn from_domain(id: &meerkat_core::lifecycle::InputId) -> Self {
        Self(id.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OperationId(pub String);

impl<T: Into<String>> From<T> for OperationId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl OperationId {
    pub fn from_domain(id: &meerkat_core::ops::OperationId) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "\"unknown\"".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct OperationKind(pub String);

impl<T: Into<String>> From<T> for OperationKind {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl OperationKind {
    pub fn from_domain(id: &meerkat_core::ops_lifecycle::OperationKind) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "\"unknown\"".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionLlmIdentity(pub String);

impl<T: Into<String>> From<T> for SessionLlmIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionLlmIdentity {
    pub fn from_domain(id: &meerkat_core::SessionLlmIdentity) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "{}".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionToolVisibilityState(pub String);

impl<T: Into<String>> From<T> for SessionToolVisibilityState {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionToolVisibilityState {
    pub fn from_domain(id: &meerkat_core::SessionToolVisibilityState) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "{}".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionLlmCapabilitySurface(pub String);

impl<T: Into<String>> From<T> for SessionLlmCapabilitySurface {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionLlmCapabilitySurface {
    pub fn from_domain(id: &crate::meerkat_machine_types::SessionLlmCapabilitySurface) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "{}".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionLlmCapabilitySurfaceStatus(pub String);

impl<T: Into<String>> From<T> for SessionLlmCapabilitySurfaceStatus {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionLlmCapabilitySurfaceStatus {
    pub fn from_domain(
        id: &crate::meerkat_machine_types::SessionLlmCapabilitySurfaceStatus,
    ) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "\"unknown\"".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct SessionToolVisibilityDelta(pub String);

impl<T: Into<String>> From<T> for SessionToolVisibilityDelta {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionToolVisibilityDelta {
    pub fn from_domain(id: &crate::meerkat_machine_types::SessionToolVisibilityDelta) -> Self {
        Self::from(format!("{id:?}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ToolFilter(pub String);

impl<T: Into<String>> From<T> for ToolFilter {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl ToolFilter {
    pub fn from_domain(id: &meerkat_core::ToolFilter) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "\"all\"".to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ToolVisibilityWitness(pub String);

impl<T: Into<String>> From<T> for ToolVisibilityWitness {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl ToolVisibilityWitness {
    pub fn from_domain(id: &meerkat_core::ToolVisibilityWitness) -> Self {
        Self::from(serde_json::to_string(id).unwrap_or_else(|_| "{}".to_string()))
    }
}

// Ensure we keep the exact generated schema DSL body from the catalog source.
machine! {
    machine MeerkatMachine {
        version: 1,
        rust: "meerkat-runtime" / "meerkat_machine::dsl",

        state {
            lifecycle_phase: MeerkatPhase,
            session_id: Option<SessionId>,
            active_runtime_id: Option<AgentRuntimeId>,
            active_fence_token: Option<FenceToken>,
            current_run_id: Option<RunId>,
            pre_run_phase: Option<String>,
            turn_phase: String,
            primitive_kind: Option<String>,
            admitted_content_shape: Option<String>,
            vision_enabled: bool,
            image_tool_results_enabled: bool,
            tool_calls_pending: u64,
            pending_op_refs: Set<String>,
            barrier_operation_ids: Set<String>,
            has_barrier_ops: bool,
            barrier_satisfied: bool,
            boundary_count: u64,
            cancel_after_boundary: bool,
            terminal_outcome: Option<String>,
            extraction_attempts: u64,
            max_extraction_retries: u64,
            silent_intent_overrides: Set<String>,

            // --- Registration substate ---
            registration_phase: String,

            // --- Comms drain substate ---
            drain_phase: String,
            drain_mode: Option<String>,

            // --- Visibility substate ---
            active_filter: String,
            staged_filter: String,
            active_visibility_revision: u64,
            staged_visibility_revision: u64,
            active_deferred_names: Set<String>,
            staged_deferred_names: Set<String>,

            // --- Input lifecycle substate ---
            input_phases: Map<String, String>,
            input_terminal_kind: Map<String, String>,
            input_superseded_by: Map<String, String>,
            input_aggregate_id: Map<String, String>,
            input_abandon_reason: Map<String, String>,
            input_abandon_attempt_count: Map<String, u64>,
            input_attempt_counts: Map<String, u64>,
            input_run_associations: Map<String, String>,
            input_boundary_sequences: Map<String, u64>,
            next_admission_seq: u64,
            input_admission_seq: Map<String, u64>,
            queue_lane: Set<String>,
            steer_lane: Set<String>,

            // --- Ops lifecycle substate ---
            op_statuses: Map<String, String>,
            op_completion_seq: Map<String, u64>,
            op_terminal_outcomes: Map<String, String>,
            op_kinds: Map<String, String>,
            op_peer_ready: Map<String, bool>,
            op_progress_counts: Map<String, u64>,
            active_op_count: u64,
            wait_active: bool,
            wait_operation_ids: Set<String>,
            next_completion_seq: u64,

            // --- External tool surface substate ---
            known_surfaces: Set<String>,
            visible_surfaces: Set<String>,
            surface_base_state: Map<String, String>,
            surface_pending_op: Map<String, String>,
            surface_staged_op: Map<String, String>,
            surface_staged_intent_sequence: Map<String, u64>,
            next_staged_intent_sequence: u64,
            surface_pending_task_sequence: Map<String, u64>,
            next_pending_task_sequence: u64,
            surface_pending_lineage_sequence: Map<String, u64>,
            surface_inflight_calls: Map<String, u64>,
            surface_last_delta_operation: Map<String, String>,
            surface_last_delta_phase: Map<String, String>,
            snapshot_epoch: u64,
            snapshot_aligned_epoch: u64,
            surface_draining_since_ms: Map<String, u64>,
            surface_removal_timeout_at_ms: Map<String, u64>,
            surface_removal_applied_at_turn: Map<String, u64>,
            surface_phase: String,
            removal_timeout_ms: u64,
        }

        init(Initializing) {
            session_id = None,
            active_runtime_id = None,
            active_fence_token = None,
            current_run_id = None,
            pre_run_phase = None,
            turn_phase = "Ready",
            primitive_kind = None,
            admitted_content_shape = None,
            vision_enabled = false,
            image_tool_results_enabled = false,
            tool_calls_pending = 0,
            pending_op_refs = EmptySet,
            barrier_operation_ids = EmptySet,
            has_barrier_ops = false,
            barrier_satisfied = false,
            boundary_count = 0,
            cancel_after_boundary = false,
            terminal_outcome = None,
            extraction_attempts = 0,
            max_extraction_retries = 0,
            silent_intent_overrides = EmptySet,
            // Registration substate
            registration_phase = "Queuing",
            // Comms drain substate
            drain_phase = "Inactive",
            drain_mode = None,
            // Visibility substate
            active_filter = "",
            staged_filter = "",
            active_visibility_revision = 0,
            staged_visibility_revision = 0,
            active_deferred_names = EmptySet,
            staged_deferred_names = EmptySet,
            // Input lifecycle substate
            input_phases = EmptyMap,
            input_terminal_kind = EmptyMap,
            input_superseded_by = EmptyMap,
            input_aggregate_id = EmptyMap,
            input_abandon_reason = EmptyMap,
            input_abandon_attempt_count = EmptyMap,
            input_attempt_counts = EmptyMap,
            input_run_associations = EmptyMap,
            input_boundary_sequences = EmptyMap,
            next_admission_seq = 0,
            input_admission_seq = EmptyMap,
            queue_lane = EmptySet,
            steer_lane = EmptySet,
            // Ops lifecycle substate
            op_statuses = EmptyMap,
            op_completion_seq = EmptyMap,
            op_terminal_outcomes = EmptyMap,
            op_kinds = EmptyMap,
            op_peer_ready = EmptyMap,
            op_progress_counts = EmptyMap,
            active_op_count = 0,
            wait_active = false,
            wait_operation_ids = EmptySet,
            next_completion_seq = 0,
            known_surfaces = EmptySet,
            visible_surfaces = EmptySet,
            surface_base_state = EmptyMap,
            surface_pending_op = EmptyMap,
            surface_staged_op = EmptyMap,
            surface_staged_intent_sequence = EmptyMap,
            next_staged_intent_sequence = 0,
            surface_pending_task_sequence = EmptyMap,
            next_pending_task_sequence = 0,
            surface_pending_lineage_sequence = EmptyMap,
            surface_inflight_calls = EmptyMap,
            surface_last_delta_operation = EmptyMap,
            surface_last_delta_phase = EmptyMap,
            snapshot_epoch = 0,
            snapshot_aligned_epoch = 0,
            surface_draining_since_ms = EmptyMap,
            surface_removal_timeout_at_ms = EmptyMap,
            surface_removal_applied_at_turn = EmptyMap,
            surface_phase = "Operating",
            removal_timeout_ms = 30000,
        }

        terminal [Destroyed]

        phase MeerkatPhase {
            Initializing,
            Idle,
            Attached,
            Running,
            Retired,
            Stopped,
            Destroyed,
        }

        input MeerkatMachineInput {
            // Direct inputs
            RegisterSession { session_id: SessionId },
            UnregisterSession { session_id: SessionId },
            ReconfigureSessionLlmIdentity {
                previous_identity: SessionLlmIdentity,
                previous_visibility_state: SessionToolVisibilityState,
                previous_capability_surface: Option<SessionLlmCapabilitySurface>,
                previous_capability_surface_status: SessionLlmCapabilitySurfaceStatus,
                target_identity: SessionLlmIdentity,
                target_capability_surface: SessionLlmCapabilitySurface,
                next_visibility_state: SessionToolVisibilityState,
                next_capability_base_filter: ToolFilter,
                next_active_visibility_revision: u64,
                tool_visibility_delta: SessionToolVisibilityDelta,
            },
            PrepareBindings { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            SetPeerIngressContext { keep_alive: bool },
            NotifyDrainExited { reason: String },
            InterruptCurrentRun,
            CancelAfterBoundary,
            StagePersistentFilter { filter: ToolFilter, witnesses: Map<String, ToolVisibilityWitness> },
            RequestDeferredTools { names: Set<String>, witnesses: Map<String, ToolVisibilityWitness> },
            PublishCommittedVisibleSet {
                active_filter: ToolFilter,
                staged_filter: ToolFilter,
                active_requested_deferred_names: Set<String>,
                staged_requested_deferred_names: Set<String>,
                active_visibility_revision: u64,
                staged_visibility_revision: u64,
            },
            Recover,
            Retire,
            Reset,
            StopRuntimeExecutor,
            RuntimeExecutorExited,
            Destroy,
            // Absorbed inputs
            EnsureSessionWithExecutor { session_id: SessionId },
            SetSilentIntents { session_id: SessionId, intents: Set<String> },
            ContainsSession { session_id: SessionId },
            SessionHasExecutor { session_id: SessionId },
            SessionHasComms { session_id: SessionId },
            OpsLifecycleRegistry { session_id: SessionId },
            InputState { session_id: SessionId, input_id: InputId },
            ListActiveInputs { session_id: SessionId },
            Abort { session_id: SessionId },
            AbortAll,
            Wait { session_id: SessionId },
            Ingest { runtime_id: AgentRuntimeId, work_id: WorkId, origin: String },
            PublishEvent { kind: String },
            RuntimeState { runtime_id: String },
            LoadBoundaryReceipt { runtime_id: String, sequence: u64 },
            AcceptWithCompletion { input_id: InputId, request_immediate_processing: bool, interrupt_yielding: bool, run_id: RunId },
            AcceptWithoutWake { input_id: InputId },
            Prepare { session_id: SessionId, run_id: RunId },
            Commit { input_id: InputId, run_id: RunId },
            Fail { run_id: RunId },
            Recycle,
            StartConversationRun {
                run_id: RunId,
                primitive_kind: String,
                admitted_content_shape: String,
                vision_enabled: bool,
                image_tool_results_enabled: bool,
                max_extraction_retries: u64,
            },
            StartImmediateAppend { run_id: RunId, admitted_content_shape: String },
            StartImmediateContext { run_id: RunId, admitted_content_shape: String },
            PrimitiveApplied,
            LlmReturnedToolCalls { tool_count: u64 },
            LlmReturnedTerminal,
            RegisterPendingOps { op_refs: Set<String>, barrier_operation_ids: Set<String> },
            ToolCallsResolved,
            OpsBarrierSatisfied { operation_ids: Set<String> },
            BoundaryContinue,
            BoundaryComplete,
            EnterExtraction,
            ExtractionStart,
            ExtractionValidationPassed,
            ExtractionValidationFailed { error: String },
            RecoverableFailure { error: String },
            FatalFailure { error: String },
            RetryRequested,
            CancelNow,
            RequestCancelAfterBoundary,
            CancellationObserved,
            AcknowledgeTerminal { outcome: String },
            TurnLimitReached,
            BudgetExhausted,
            TimeBudgetExceeded,
            ForceCancelNoRun,
            RunCompleted { run_id: RunId },
            RunFailed { run_id: RunId, error: String },
            RunCancelled { run_id: RunId },
            // Input lifecycle inputs
            QueueAccepted { input_id: String },
            StageForRun { input_id: String, run_id: String },
            IncrementAttemptCount { input_id: String },
            RollbackStaged { input_id: String },
            MarkApplied { input_id: String },
            MarkAppliedPendingConsumption { input_id: String },
            ConsumeInput { input_id: String },
            ConsumeOnAccept { input_id: String },
            SupersedeInput { input_id: String, superseded_by: String },
            CoalesceInput { input_id: String, aggregate_id: String },
            AbandonInput {
                input_id: String,
                reason: String,
                attempt_count: u64,
            },
            RecordBoundarySeq { input_id: String, seq: u64 },
            // Ops lifecycle inputs
            RegisterOp { operation_id: String, kind: String },
            StartOp { operation_id: String },
            CompleteOp { operation_id: String, outcome: String },
            FailOp { operation_id: String, outcome: String },
            CancelOp { operation_id: String, outcome: String },
            AbortOp { operation_id: String, outcome: String },
            PeerReadyOp { operation_id: String },
            ProgressReportedOp { operation_id: String },
            RetireRequestedOp { operation_id: String },
            RetireCompletedOp { operation_id: String, outcome: String },
            TerminateOp { operation_id: String, outcome: String },
            RequestWaitAll { operation_ids: Set<String> },
            SatisfyWaitAll,
            // Comms drain inputs
            SpawnDrain { mode: String },
            StopDrain,
            DrainExitedClean,
            DrainExitedRespawnable,
            // Visibility inputs
            StageVisibilityFilter { filter: String, revision: u64 },
            CommitVisibilityFilter { filter: String, revision: u64 },
            StageDeferredNames { names: Set<String> },
            CommitDeferredNames { names: Set<String> },
            SurfaceRegister { surface_id: String },
            SurfaceStageAdd { surface_id: String, now_ms: u64 },
            SurfaceStageRemove { surface_id: String, now_ms: u64 },
            SurfaceStageReload { surface_id: String, now_ms: u64 },
            SurfaceApplyBoundary { surface_id: String, now_ms: u64, current_turn: u64 },
            SurfaceMarkPendingSucceeded {
                surface_id: String,
                pending_task_sequence: u64,
                staged_intent_sequence: u64,
            },
            SurfaceMarkPendingFailed { surface_id: String, reason: String },
            SurfaceCallStarted { surface_id: String },
            SurfaceCallFinished { surface_id: String },
            SurfaceFinalizeRemovalClean { surface_id: String },
            SurfaceFinalizeRemovalForced { surface_id: String },
            SurfaceSnapshotAligned { epoch: u64 },
            SurfaceShutdown,
        }

        surface_only [
            ContainsSession,
            SessionHasExecutor,
            SessionHasComms,
            OpsLifecycleRegistry,
            InputState,
            ListActiveInputs,
            RuntimeState,
            LoadBoundaryReceipt,
            Recover
        ]

        signal MeerkatMachineSignal {
            Initialize,
            BoundaryApplied { revision: u64 },
            DrainQueuedRun { run_id: RunId },
            ClassifyExternalEnvelope,
            ClassifyPlainEvent,
            EnsureDrainRunning,
        }

        effect MeerkatMachineEffect {
            RuntimeBound { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            TurnRunStarted { run_id: RunId },
            TurnBoundaryApplied { run_id: RunId, boundary_sequence: u64 },
            TurnRunCompleted { run_id: RunId, outcome: String },
            TurnRunFailed { run_id: RunId, error: String },
            TurnRunCancelled { run_id: RunId, reason: String },
            TurnCheckCompaction,
            RequestCancellationAtBoundary,
            WakeInterrupt,
            CommittedVisibleSetPublished { revision: u64 },
            RuntimeNotice { kind: String, detail: String },
            // Absorbed effects
            ResolveAdmission,
            SubmitAdmittedIngressEffect,
            SubmitRunPrimitive,
            ResolveCompletionAsTerminated,
            ApplyControlPlaneCommand,
            InitiateRecycle,
            IngressAccepted,
            PostAdmissionSignal { signal: String },
            ReadyForRun,
            InputLifecycleNotice,
            CompletionResolved,
            IngressNotice,
            SilentIntentApplied,
            CheckCompaction,
            RecordTerminalOutcome,
            RecordRunAssociation,
            RecordBoundarySequence,
            SubmitOpEvent { operation_id: String },
            NotifyOpWatcher { operation_id: String },
            ExposeOperationPeer { operation_id: String },
            RetainTerminalRecord { operation_id: String },
            EvictCompletedRecord { operation_id: String },
            CompletionProduced { seq: u64, operation_id: OperationId, kind: OperationKind },
            WaitAllSatisfied,
            CollectCompletedResult,
            EnqueueClassifiedEntry,
            SpawnDrainTask,
            ScheduleSurfaceCompletion {
                surface_id: String,
                operation: String,
                pending_task_sequence: u64,
                staged_intent_sequence: u64,
                applied_at_turn: u64,
            },
            RefreshVisibleSurfaceSet,
            EmitExternalToolDelta { surface_id: String, operation: String, phase: String },
            CloseSurfaceConnection { surface_id: String },
            RejectSurfaceCall { surface_id: String, reason: String },
        }

        // =====================================================================
        // Effect dispositions
        // =====================================================================

        disposition RuntimeBound => routed [MobMachine],
        disposition RuntimeRetired => routed [MobMachine],
        disposition RuntimeDestroyed => routed [MobMachine],
        disposition TurnRunStarted => local,
        disposition TurnBoundaryApplied => local,
        disposition TurnRunCompleted => local,
        disposition TurnRunFailed => local,
        disposition TurnRunCancelled => local,
        disposition TurnCheckCompaction => local,
        disposition RequestCancellationAtBoundary => local,
        disposition WakeInterrupt => local,
        disposition CommittedVisibleSetPublished => external,
        disposition RuntimeNotice => external,
        // Absorbed effect dispositions
        disposition ResolveAdmission => local,
        disposition SubmitAdmittedIngressEffect => local,
        disposition SubmitRunPrimitive => local,
        disposition ResolveCompletionAsTerminated => local,
        disposition ApplyControlPlaneCommand => local,
        disposition InitiateRecycle => local,
        disposition IngressAccepted => external,
        disposition PostAdmissionSignal => local,
        disposition ReadyForRun => local,
        disposition InputLifecycleNotice => external,
        disposition CompletionResolved => local,
        disposition IngressNotice => external,
        disposition SilentIntentApplied => external,
        disposition CheckCompaction => local,
        disposition RecordTerminalOutcome => local,
        disposition RecordRunAssociation => local,
        disposition RecordBoundarySequence => local,
        disposition SubmitOpEvent => local,
        disposition NotifyOpWatcher => local,
        disposition ExposeOperationPeer => local,
        disposition RetainTerminalRecord => local,
        disposition EvictCompletedRecord => local,
        disposition CompletionProduced => local,
        disposition WaitAllSatisfied => local,
        disposition CollectCompletedResult => local,
        disposition EnqueueClassifiedEntry => local,
        disposition SpawnDrainTask => local,
        disposition ScheduleSurfaceCompletion => local,
        disposition RefreshVisibleSurfaceSet => external,
        disposition EmitExternalToolDelta => external,
        disposition CloseSurfaceConnection => local,
        disposition RejectSurfaceCall => external,

        // =====================================================================
        // Invariants
        // =====================================================================

        invariant fence_requires_bound_runtime {
            self.active_fence_token == None || self.active_runtime_id != None
        }

        invariant running_has_current_run {
            self.lifecycle_phase != Phase::Running || self.current_run_id != None
        }

        invariant current_run_only_while_running_or_retired {
            self.current_run_id == None
            || self.lifecycle_phase == Phase::Running
            || self.lifecycle_phase == Phase::Retired
        }

        // =====================================================================
        // Direct transitions
        // =====================================================================

        // 1. Initialize: Initializing → Idle
        transition Initialize {
            on signal Initialize
            guard { self.lifecycle_phase == Phase::Initializing }
            update {}
            to Idle
        }

        // 2. RegisterSession: per-phase self-loop, no guard
        transition RegisterSession {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RegisterSession { session_id }
            update {
                self.session_id = Some(session_id);
            }
            to Idle
        }

        // 3. UnregisterSession: per-phase → Idle (NOT a self-loop, goes to Idle)
        // Cannot use per_phase because target is always Idle, not source phase.
        transition UnregisterSessionIdle {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = "Queuing";
            }
            to Idle
        }
        transition UnregisterSessionAttached {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = "Queuing";
            }
            to Idle
        }
        transition UnregisterSessionRunning {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = "Queuing";
            }
            to Idle
        }
        transition UnregisterSessionRetired {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = "Queuing";
            }
            to Idle
        }
        transition UnregisterSessionStopped {
            on input UnregisterSession { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_matches_current" { self.session_id == Some(session_id) }
            update {
                self.session_id = None;
                self.active_runtime_id = None;
                self.active_fence_token = None;
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.registration_phase = "Queuing";
            }
            to Idle
        }

        // 4. ReconfigureSessionLlmIdentity: Attached + Running self-loops
        transition ReconfigureSessionLlmIdentityAttached {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Attached
        }
        transition ReconfigureSessionLlmIdentityRunning {
            on input ReconfigureSessionLlmIdentity {
                previous_identity, previous_visibility_state,
                previous_capability_surface, previous_capability_surface_status,
                target_identity, target_capability_surface,
                next_visibility_state, next_capability_base_filter,
                next_active_visibility_revision, tool_visibility_delta
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {}
            to Running
        }

        // 5. StagePersistentFilter: per-phase self-loop, guard session_registered
        transition StagePersistentFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StagePersistentFilter { filter, witnesses }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 6. RequestDeferredTools: per-phase self-loop, guard session_registered
        transition RequestDeferredTools {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequestDeferredTools { names, witnesses }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 7. PrepareBindings: different source→target mappings per phase
        // Initializing → Initializing (no guard, emits RuntimeBound)
        transition PrepareBindingsInitializing {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Initializing }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Initializing
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Idle → Attached
        transition PrepareBindingsIdle {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Attached → Attached
        transition PrepareBindingsAttached {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Running → Running
        transition PrepareBindingsRunning {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Running
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Retired → Retired
        transition PrepareBindingsRetired {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Retired
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }
        // Stopped → Stopped (inline in hand-written catalog)
        transition PrepareBindingsStopped {
            on input PrepareBindings { agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Stopped
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 8. SetPeerIngressContext: per-phase self-loop, guard session_registered
        transition SetPeerIngressContext {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SetPeerIngressContext { keep_alive }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 9. NotifyDrainExited: per-phase self-loop, guard session_registered, emit RuntimeNotice
        transition NotifyDrainExited {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input NotifyDrainExited { reason }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit RuntimeNotice { kind: "drain", detail: "drain exited" }
        }

        // 10. InterruptCurrentRun: Attached + Running self-loops
        transition InterruptCurrentRunAttached {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }
        transition InterruptCurrentRun {
            on input InterruptCurrentRun
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit WakeInterrupt
            emit RequestCancellationAtBoundary
        }

        // 11. CancelAfterBoundary: Attached + Running self-loops
        transition CancelAfterBoundaryAttached {
            on input CancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit RequestCancellationAtBoundary
        }
        transition CancelAfterBoundary {
            on input CancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit RequestCancellationAtBoundary
        }

        // 12. BoundaryAppliedPublish: Running self-loop (signal)
        transition BoundaryAppliedPublish {
            on signal BoundaryApplied { revision }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit CommittedVisibleSetPublished { revision: revision }
        }

        // 13. PublishCommittedVisibleSet: per-phase self-loop, complex guards
        transition PublishCommittedVisibleSetIdle {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Idle
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetAttached {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Attached
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRunning {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Running
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetRetired {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Retired
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }
        transition PublishCommittedVisibleSetStopped {
            on input PublishCommittedVisibleSet {
                active_filter, staged_filter,
                active_requested_deferred_names, staged_requested_deferred_names,
                active_visibility_revision, staged_visibility_revision
            }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            guard "active_not_behind_staged" { active_visibility_revision >= staged_visibility_revision }
            guard "equal_revision_requires_equal_active_and_staged_input" {
                active_visibility_revision != staged_visibility_revision
                || (active_filter == staged_filter
                    && active_requested_deferred_names == staged_requested_deferred_names)
            }
            guard "active_requested_subset_of_staged_requested" {
                for_all(requested_name in active_requested_deferred_names, staged_requested_deferred_names.contains(requested_name))
            }
            update {}
            to Stopped
            emit CommittedVisibleSetPublished { revision: active_visibility_revision }
        }

        // 14. Retire: from [Idle, Attached, Running] → Retired
        transition RetireRequestedFromIdle {
            on input Retire
            guard {
                self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
            }
            update {}
            to Retired
            emit RuntimeRetired { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 15. Reset: from [Initializing, Idle, Attached, Retired] → Idle
        transition Reset {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.active_fence_token = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Idle
            emit RuntimeNotice { kind: "reset", detail: "runtime reset" }
        }

        // 16. StopRuntimeExecutor: different behavior per phase
        // Unbound (Initializing, Idle, Retired) → Stopped
        transition StopRuntimeExecutorUnbound {
            on input StopRuntimeExecutor
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Retired
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: "stop", detail: "runtime executor stopped" }
        }
        // Attached → Attached (self-loop)
        transition StopRuntimeExecutorAttached {
            on input StopRuntimeExecutor
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Attached
            emit RuntimeNotice { kind: "stop", detail: "runtime executor stopped" }
        }
        // Running → Running (self-loop)
        transition StopRuntimeExecutorRunning {
            on input StopRuntimeExecutor
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Running
            emit RuntimeNotice { kind: "stop", detail: "runtime executor stopped" }
        }

        // RuntimeExecutorExited: async finalization of the stop-runtime path.
        // Fired by the runtime loop after apply_executor_control sees the
        // StopRuntimeExecutor control command complete and the driver has
        // flipped to Stopped. Moves the DSL phase from whatever it was at
        // stop-request time to Stopped.
        transition RuntimeExecutorExitedFromAttached {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: "exit", detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromRunning {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: "exit", detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromIdle {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: "exit", detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromRetired {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Stopped
            emit RuntimeNotice { kind: "exit", detail: "runtime executor exited" }
        }
        transition RuntimeExecutorExitedFromStopped {
            on input RuntimeExecutorExited
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        // 17. Destroy: from all non-Destroyed → Destroyed
        transition Destroy {
            on input Destroy
            guard {
                self.lifecycle_phase == Phase::Initializing
                || self.lifecycle_phase == Phase::Idle
                || self.lifecycle_phase == Phase::Attached
                || self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Retired
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
                self.registration_phase = "Queuing";
            }
            to Destroyed
            emit RuntimeDestroyed { agent_runtime_id: self.active_runtime_id.get("value"), fence_token: self.active_fence_token.get("value") }
        }

        // 18. Recover: preserve coarse phase while replaying runtime state.
        transition RecoverInitializing {
            on input Recover
            guard { self.lifecycle_phase == Phase::Initializing }
            update {}
            to Initializing
            emit RuntimeNotice { kind: "recover", detail: "runtime recovered" }
        }
        transition RecoverIdle {
            on input Recover
            guard { self.lifecycle_phase == Phase::Idle }
            update {}
            to Idle
            emit RuntimeNotice { kind: "recover", detail: "runtime recovered" }
        }
        transition RecoverAttached {
            on input Recover
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
            emit RuntimeNotice { kind: "recover", detail: "runtime recovered" }
        }
        transition RecoverRetired {
            on input Recover
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
            emit RuntimeNotice { kind: "recover", detail: "runtime recovered" }
        }
        transition RecoverStopped {
            on input Recover
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit RuntimeNotice { kind: "recover", detail: "runtime recovered" }
        }

        // =====================================================================
        // Absorbed transitions
        // =====================================================================

        // 19. EnsureSessionWithExecutor
        // Idle → Attached (phase change), sets registration to Active
        transition EnsureSessionWithExecutorIdle {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.registration_phase = "Active";
            }
            to Attached
        }
        // Attached, Running: self-loop (already Active)
        transition EnsureSessionWithExecutorAttached {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.registration_phase = "Active";
            }
            to Attached
        }
        transition EnsureSessionWithExecutorRunning {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.registration_phase = "Active";
            }
            to Running
        }
        // Retired, Stopped: self-loop
        transition EnsureSessionWithExecutorRetired {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {}
            to Retired
        }
        transition EnsureSessionWithExecutorStopped {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        // 19. SetSilentIntents: per-phase, guard session_registered
        // Idle, Attached, Running, Retired: update intents
        transition SetSilentIntentsIdle {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Idle
        }
        transition SetSilentIntentsAttached {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Attached
        }
        transition SetSilentIntentsRunning {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Running
        }
        transition SetSilentIntentsRetired {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Retired }
            guard "session_registered" { self.session_id != None }
            update { self.silent_intent_overrides = intents; }
            to Retired
        }
        // Stopped: no-op (no update)
        transition SetSilentIntentsStopped {
            on input SetSilentIntents { session_id, intents }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_registered" { self.session_id != None }
            update {}
            to Stopped
        }

        // 20. Abort: per-phase self-loop, guard session_registered
        transition Abort {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Abort { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 20b. Wait: per-phase self-loop, guard session_registered
        transition Wait {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input Wait { session_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
        }

        // 21. AbortAll: per-phase self-loop, no guard
        transition AbortAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortAll
            update {}
            to Idle
        }

        // 22. EnsureDrainRunning: Attached/Running self-loops, emit SpawnDrainTask
        transition EnsureDrainRunningAttached {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SpawnDrainTask
        }
        transition EnsureDrainRunningRunning {
            on signal EnsureDrainRunning
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit SpawnDrainTask
        }

        // 23. Ingest: Idle/Attached/Running self-loops, emit ResolveAdmission
        transition Ingest {
            per_phase [Idle, Attached, Running]
            on input Ingest { runtime_id, work_id, origin }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit ResolveAdmission
        }

        // 24. PublishEvent: per-phase self-loop, emit IngressNotice
        transition PublishEvent {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PublishEvent { kind }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressNotice
        }

        // 25. AcceptWithCompletion: complex, multiple variants per phase
        // Idle + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionIdleQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "WakeLoop" }
        }
        // Idle + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionIdleImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Idle
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
        }
        // Attached + immediate → Running (phase change!)
        transition AcceptWithCompletionAttachedImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("attached");
            }
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
            emit SubmitRunPrimitive
        }
        // Attached + queued (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionAttachedQueued {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Attached
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "WakeLoop" }
        }
        // Running + queued passive (immediate=false, interrupt_yielding=false)
        transition AcceptWithCompletionRunningQueuedPassive {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Running
            emit IngressAccepted
        }
        // Running + interrupt_yielding (immediate=false, interrupt_yielding=true)
        transition AcceptWithCompletionRunningInterruptYielding {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == false }
            guard "interrupt_yielding" { interrupt_yielding == true }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "InterruptYielding" }
        }
        // Running + immediate (immediate=true, interrupt_yielding=false)
        transition AcceptWithCompletionRunningImmediate {
            on input AcceptWithCompletion { input_id, request_immediate_processing, interrupt_yielding, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            guard "request_immediate_processing" { request_immediate_processing == true }
            guard "interrupt_yielding" { interrupt_yielding == false }
            update {}
            to Running
            emit IngressAccepted
            emit PostAdmissionSignal { signal: "RequestImmediateProcessing" }
        }

        // 26. AcceptWithoutWake: Idle/Attached/Running self-loops
        transition AcceptWithoutWake {
            per_phase [Idle, Attached, Running]
            on input AcceptWithoutWake { input_id }
            guard "session_registered" { self.session_id != None }
            update {}
            to Idle
            emit IngressAccepted
        }

        // 27. ClassifyExternalEnvelope/ClassifyPlainEvent: Attached/Running, emit EnqueueClassifiedEntry
        transition ClassifyExternalEnvelopeAttached {
            on signal ClassifyExternalEnvelope
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
        }
        transition ClassifyExternalEnvelopeRunning {
            on signal ClassifyExternalEnvelope
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EnqueueClassifiedEntry
        }
        transition ClassifyPlainEventAttached {
            on signal ClassifyPlainEvent
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EnqueueClassifiedEntry
        }
        transition ClassifyPlainEventRunning {
            on signal ClassifyPlainEvent
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EnqueueClassifiedEntry
        }

        // 28. Prepare: Idle→Running, Attached→Running
        transition PrepareIdle {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Idle }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("idle");
            }
            to Running
            emit SubmitRunPrimitive
        }
        transition PrepareAttached {
            on input Prepare { session_id, run_id }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("attached");
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 29. DrainQueuedRun: Retired→Running (signal)
        transition DrainQueuedRunRetired {
            on signal DrainQueuedRun { run_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.current_run_id = Some(run_id);
                self.pre_run_phase = Some("retired");
            }
            to Running
            emit SubmitRunPrimitive
        }

        // 30. Turn execution absorption
        transition StartConversationRunInitializing {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartConversationRunAttached {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartConversationRunRunning {
            on input StartConversationRun { run_id, primitive_kind, admitted_content_shape, vision_enabled, image_tool_results_enabled, max_extraction_retries }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some(primitive_kind);
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = vision_enabled;
                self.image_tool_results_enabled = image_tool_results_enabled;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = max_extraction_retries;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition StartImmediateAppendInitializing {
            on input StartImmediateAppend { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateAppendAttached {
            on input StartImmediateAppend { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateAppendRunning {
            on input StartImmediateAppend { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition StartImmediateContextInitializing {
            on input StartImmediateContext { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Initializing }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateContextAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateContextAttached {
            on input StartImmediateContext { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateContextAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }
        transition StartImmediateContextRunning {
            on input StartImmediateContext { run_id, admitted_content_shape }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_resettable" {
                self.turn_phase == "Ready"
                || self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.current_run_id = Some(run_id);
                self.turn_phase = "ApplyingPrimitive";
                self.primitive_kind = Some("ImmediateContextAppend");
                self.admitted_content_shape = Some(admitted_content_shape);
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = None;
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
            emit TurnRunStarted { run_id: run_id }
        }

        transition PrimitiveAppliedConversation {
            on input PrimitiveApplied
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_applying_conversation" {
                self.turn_phase == "ApplyingPrimitive"
                && self.primitive_kind == Some("ConversationTurn")
            }
            update {
                self.turn_phase = "CallingLlm";
            }
            to Running
            emit TurnCheckCompaction
        }

        transition PrimitiveAppliedImmediate {
            on input PrimitiveApplied
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_applying_immediate" {
                self.turn_phase == "ApplyingPrimitive"
                && (self.primitive_kind == Some("ImmediateAppend")
                    || self.primitive_kind == Some("ImmediateContextAppend"))
            }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = "Completed";
                self.terminal_outcome = Some("Completed");
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: "Completed" }
            emit TurnCheckCompaction
        }

        transition LlmReturnedToolCallsPositive {
            on input LlmReturnedToolCalls { tool_count }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == "CallingLlm" }
            guard "tool_count_positive" { tool_count > 0 }
            update {
                self.turn_phase = "WaitingForOps";
                self.tool_calls_pending = tool_count;
            }
            to Running
        }

        transition LlmReturnedToolCallsZero {
            on input LlmReturnedToolCalls { tool_count }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == "CallingLlm" }
            guard "tool_count_zero" { tool_count == 0 }
            update {
                self.turn_phase = "DrainingBoundary";
                self.tool_calls_pending = 0;
            }
            to Running
        }

        transition LlmReturnedTerminal {
            on input LlmReturnedTerminal
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_calling_llm" { self.turn_phase == "CallingLlm" }
            update {
                self.turn_phase = "DrainingBoundary";
            }
            to Running
        }

        transition RegisterPendingOps {
            on input RegisterPendingOps { op_refs, barrier_operation_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_or_calling" { self.turn_phase == "CallingLlm" || self.turn_phase == "WaitingForOps" }
            update {
                self.turn_phase = "WaitingForOps";
                self.pending_op_refs = op_refs;
                self.barrier_operation_ids = barrier_operation_ids;
                self.has_barrier_ops = self.barrier_operation_ids != EmptySet;
                self.barrier_satisfied = self.barrier_operation_ids == EmptySet;
                self.tool_calls_pending = 0;
            }
            to Running
        }

        transition ToolCallsResolvedToCalling {
            on input ToolCallsResolved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == "WaitingForOps" }
            guard "barrier_not_satisfied" { self.barrier_satisfied == false }
            update {
                self.turn_phase = "CallingLlm";
            }
            to Running
        }

        transition ToolCallsResolvedToBoundary {
            on input ToolCallsResolved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == "WaitingForOps" }
            guard "barrier_satisfied" { self.barrier_satisfied == true }
            update {
                self.turn_phase = "DrainingBoundary";
            }
            to Running
        }

        transition OpsBarrierSatisfied {
            on input OpsBarrierSatisfied { operation_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_waiting_for_ops" { self.turn_phase == "WaitingForOps" }
            guard "matching_barrier_ids" { operation_ids == self.barrier_operation_ids }
            update {
                self.barrier_satisfied = true;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
            }
            to Running
        }

        transition BoundaryContinue {
            on input BoundaryContinue
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == "DrainingBoundary" }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = "CallingLlm";
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnCheckCompaction
        }

        transition BoundaryComplete {
            on input BoundaryComplete
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == "DrainingBoundary" }
            update {
                self.boundary_count = self.boundary_count + 1;
                self.turn_phase = "Completed";
                self.terminal_outcome = Some("Completed");
            }
            to Running
            emit TurnBoundaryApplied { run_id: self.current_run_id.get("value"), boundary_sequence: self.boundary_count }
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: "Completed" }
            emit TurnCheckCompaction
        }

        transition EnterExtraction {
            on input EnterExtraction
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_draining_boundary" { self.turn_phase == "DrainingBoundary" }
            update {
                self.turn_phase = "Extracting";
                self.extraction_attempts = self.extraction_attempts + 1;
            }
            to Running
        }

        transition ExtractionStart {
            on input ExtractionStart
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == "Extracting" }
            update {}
            to Running
        }

        transition ExtractionValidationPassed {
            on input ExtractionValidationPassed
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == "Extracting" }
            update {
                self.turn_phase = "Completed";
                self.terminal_outcome = Some("Completed");
            }
            to Running
            emit TurnRunCompleted { run_id: self.current_run_id.get("value"), outcome: "Completed" }
            emit TurnCheckCompaction
        }

        transition ExtractionValidationFailedRetry {
            on input ExtractionValidationFailed { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == "Extracting" }
            guard "retries_remaining" { self.extraction_attempts < self.max_extraction_retries }
            update {
                self.turn_phase = "CallingLlm";
            }
            to Running
            emit TurnCheckCompaction
        }

        transition ExtractionValidationFailedExhausted {
            on input ExtractionValidationFailed { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_extracting" { self.turn_phase == "Extracting" }
            guard "retries_exhausted" { self.extraction_attempts >= self.max_extraction_retries }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some("ExtractionExhausted");
            }
            to Running
            emit TurnRunFailed { run_id: self.current_run_id.get("value"), error: "ExtractionExhausted" }
        }

        transition RecoverableFailure {
            on input RecoverableFailure { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_non_terminal" {
                self.turn_phase == "CallingLlm"
                || self.turn_phase == "WaitingForOps"
                || self.turn_phase == "DrainingBoundary"
                || self.turn_phase == "Extracting"
            }
            update {
                self.turn_phase = "ErrorRecovery";
            }
            to Running
        }

        transition FatalFailure {
            on input FatalFailure { error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != "Completed" && self.turn_phase != "Failed" && self.turn_phase != "Cancelled" }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some(error);
            }
            to Running
            emit TurnRunFailed { run_id: self.current_run_id.get("value"), error: error }
        }

        transition RetryRequested {
            on input RetryRequested
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_error_recovery" { self.turn_phase == "ErrorRecovery" }
            update {
                self.turn_phase = "CallingLlm";
            }
            to Running
            emit TurnCheckCompaction
        }

        transition CancelNow {
            on input CancelNow
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancellable" {
                self.turn_phase != "Ready"
                && self.turn_phase != "Completed"
                && self.turn_phase != "Failed"
                && self.turn_phase != "Cancelled"
            }
            update {
                self.turn_phase = "Cancelling";
            }
            to Running
        }

        transition RequestCancelAfterBoundary {
            on input RequestCancelAfterBoundary
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancellable" {
                self.turn_phase != "Ready"
                && self.turn_phase != "Completed"
                && self.turn_phase != "Failed"
                && self.turn_phase != "Cancelled"
            }
            update {
                self.cancel_after_boundary = true;
            }
            to Running
        }

        transition CancellationObserved {
            on input CancellationObserved
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_cancelling" { self.turn_phase == "Cancelling" }
            update {
                self.turn_phase = "Cancelled";
                self.terminal_outcome = Some("Cancelled");
            }
            to Running
            emit TurnRunCancelled { run_id: self.current_run_id.get("value"), reason: "observed" }
        }

        transition AcknowledgeTerminal {
            on input AcknowledgeTerminal { outcome }
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_terminal" {
                self.turn_phase == "Completed"
                || self.turn_phase == "Failed"
                || self.turn_phase == "Cancelled"
            }
            update {
                self.turn_phase = "Ready";
                self.primitive_kind = None;
                self.admitted_content_shape = None;
                self.vision_enabled = false;
                self.image_tool_results_enabled = false;
                self.tool_calls_pending = 0;
                self.pending_op_refs = EmptySet;
                self.barrier_operation_ids = EmptySet;
                self.has_barrier_ops = false;
                self.barrier_satisfied = false;
                self.boundary_count = 0;
                self.cancel_after_boundary = false;
                self.terminal_outcome = Some(outcome);
                self.extraction_attempts = 0;
                self.max_extraction_retries = 0;
            }
            to Running
        }

        transition TurnLimitReached {
            on input TurnLimitReached
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != "Completed" && self.turn_phase != "Failed" && self.turn_phase != "Cancelled" }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some("TurnLimitReached");
            }
            to Running
            emit TurnRunFailed { run_id: self.current_run_id.get("value"), error: "TurnLimitReached" }
        }

        transition BudgetExhausted {
            on input BudgetExhausted
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != "Completed" && self.turn_phase != "Failed" && self.turn_phase != "Cancelled" }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some("BudgetExhausted");
            }
            to Running
            emit TurnRunFailed { run_id: self.current_run_id.get("value"), error: "BudgetExhausted" }
        }

        transition TimeBudgetExceeded {
            on input TimeBudgetExceeded
            guard { self.lifecycle_phase == Phase::Running }
            guard "turn_not_terminal" { self.turn_phase != "Completed" && self.turn_phase != "Failed" && self.turn_phase != "Cancelled" }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some("TimeBudgetExceeded");
            }
            to Running
            emit TurnRunFailed { run_id: self.current_run_id.get("value"), error: "TimeBudgetExceeded" }
        }

        transition ForceCancelNoRun {
            on input ForceCancelNoRun
            guard { self.lifecycle_phase == Phase::Running }
            guard "no_run_bound" { self.current_run_id == None }
            guard "turn_ready" { self.turn_phase == "Ready" }
            update {
                self.turn_phase = "Cancelled";
                self.terminal_outcome = Some("ForceCancelNoRun");
            }
            to Running
        }

        transition RunCompleted {
            on input RunCompleted { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.turn_phase = "Completed";
                self.terminal_outcome = Some("Completed");
            }
            to Running
        }

        transition RunFailed {
            on input RunFailed { run_id, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.turn_phase = "Failed";
                self.terminal_outcome = Some(error);
            }
            to Running
        }

        transition RunCancelled {
            on input RunCancelled { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.turn_phase = "Cancelled";
                self.terminal_outcome = Some("RunCancelled");
            }
            to Running
        }

        transition SurfaceRegisterAttached {
            on input SurfaceRegister { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.known_surfaces.insert(surface_id);
            }
            to Attached
        }
        transition SurfaceRegisterRunning {
            on input SurfaceRegister { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.known_surfaces.insert(surface_id);
            }
            to Running
        }

        transition SurfaceStageAddAttached {
            on input SurfaceStageAdd { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Add");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageAddRunning {
            on input SurfaceStageAdd { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Add");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceStageRemoveAttached {
            on input SurfaceStageRemove { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Remove");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageRemoveRunning {
            on input SurfaceStageRemove { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Remove");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceStageReloadAttached {
            on input SurfaceStageReload { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Attached }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Reload");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Attached
        }
        transition SurfaceStageReloadRunning {
            on input SurfaceStageReload { surface_id, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "surface_operating" { self.surface_phase == "Operating" }
            update {
                self.known_surfaces.insert(surface_id);
                self.surface_staged_op.insert(surface_id, "Reload");
                self.next_staged_intent_sequence = self.next_staged_intent_sequence + 1;
                self.surface_staged_intent_sequence.insert(surface_id, self.next_staged_intent_sequence);
            }
            to Running
        }

        transition SurfaceApplyBoundaryAttached {
            on input SurfaceApplyBoundary { surface_id, now_ms, current_turn }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Attached
        }
        transition SurfaceApplyBoundaryRunning {
            on input SurfaceApplyBoundary { surface_id, now_ms, current_turn }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.snapshot_epoch = self.snapshot_epoch + 1;
            }
            to Running
        }

        transition SurfaceMarkPendingSucceededAttached {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_pending_op.insert(surface_id, "None");
                self.surface_last_delta_phase.insert(surface_id, "Applied");
            }
            to Attached
        }
        transition SurfaceMarkPendingSucceededRunning {
            on input SurfaceMarkPendingSucceeded { surface_id, pending_task_sequence, staged_intent_sequence }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_pending_op.insert(surface_id, "None");
                self.surface_last_delta_phase.insert(surface_id, "Applied");
            }
            to Running
        }

        transition SurfaceMarkPendingFailedAttached {
            on input SurfaceMarkPendingFailed { surface_id, reason }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_pending_op.insert(surface_id, "None");
                self.surface_last_delta_phase.insert(surface_id, "Failed");
            }
            to Attached
        }
        transition SurfaceMarkPendingFailedRunning {
            on input SurfaceMarkPendingFailed { surface_id, reason }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_pending_op.insert(surface_id, "None");
                self.surface_last_delta_phase.insert(surface_id, "Failed");
            }
            to Running
        }

        transition SurfaceCallStartedAttached {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_inflight_calls.increment(surface_id, 1);
            }
            to Attached
        }
        transition SurfaceCallStartedRunning {
            on input SurfaceCallStarted { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_inflight_calls.increment(surface_id, 1);
            }
            to Running
        }

        transition SurfaceCallFinishedAttached {
            on input SurfaceCallFinished { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_inflight_calls.insert(surface_id, 0);
            }
            to Attached
        }
        transition SurfaceCallFinishedRunning {
            on input SurfaceCallFinished { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_inflight_calls.insert(surface_id, 0);
            }
            to Running
        }

        transition SurfaceFinalizeRemovalCleanAttached {
            on input SurfaceFinalizeRemovalClean { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_base_state.insert(surface_id, "Removed");
            }
            to Attached
            emit CloseSurfaceConnection { surface_id: surface_id }
        }
        transition SurfaceFinalizeRemovalCleanRunning {
            on input SurfaceFinalizeRemovalClean { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_base_state.insert(surface_id, "Removed");
            }
            to Running
            emit CloseSurfaceConnection { surface_id: surface_id }
        }

        transition SurfaceFinalizeRemovalForcedAttached {
            on input SurfaceFinalizeRemovalForced { surface_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_base_state.insert(surface_id, "Removed");
                self.surface_last_delta_phase.insert(surface_id, "Forced");
            }
            to Attached
            emit CloseSurfaceConnection { surface_id: surface_id }
        }
        transition SurfaceFinalizeRemovalForcedRunning {
            on input SurfaceFinalizeRemovalForced { surface_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_base_state.insert(surface_id, "Removed");
                self.surface_last_delta_phase.insert(surface_id, "Forced");
            }
            to Running
            emit CloseSurfaceConnection { surface_id: surface_id }
        }

        transition SurfaceSnapshotAlignedAttached {
            on input SurfaceSnapshotAligned { epoch }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.snapshot_aligned_epoch = epoch;
            }
            to Attached
        }
        transition SurfaceSnapshotAlignedRunning {
            on input SurfaceSnapshotAligned { epoch }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.snapshot_aligned_epoch = epoch;
            }
            to Running
        }

        transition SurfaceShutdownAttached {
            on input SurfaceShutdown
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.surface_phase = "Shutdown";
            }
            to Attached
        }
        transition SurfaceShutdownRunning {
            on input SurfaceShutdown
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.surface_phase = "Shutdown";
            }
            to Running
        }

        // 31. Commit: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition CommitRunningToIdle {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some("idle") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
        }
        transition CommitRunningToAttached {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some("attached") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
        }
        transition CommitRunningToRetired {
            on input Commit { input_id, run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some("retired") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
        }

        // 32. Fail: Running → Idle/Attached/Retired (guard pre_run_phase + run_id match)
        transition FailRunningToIdle {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_idle" { self.pre_run_phase == Some("idle") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Idle
            emit RecordTerminalOutcome
        }
        transition FailRunningToAttached {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_attached" { self.pre_run_phase == Some("attached") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Attached
            emit RecordTerminalOutcome
        }
        transition FailRunningToRetired {
            on input Fail { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pre_run_phase_matches_retired" { self.pre_run_phase == Some("retired") }
            guard "current_run_id_matches_binding" { self.current_run_id == Some(run_id) }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
            }
            to Retired
            emit RecordTerminalOutcome
        }

        // 34. Recycle: from Idle/Retired → Idle, from Attached → Attached
        transition RecycleFromIdleOrRetired {
            on input Recycle
            guard {
                self.lifecycle_phase == Phase::Idle || self.lifecycle_phase == Phase::Retired
            }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Idle
            emit InitiateRecycle
        }
        transition RecycleFromAttached {
            on input Recycle
            guard { self.lifecycle_phase == Phase::Attached }
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.active_fence_token = None;
                self.current_run_id = None;
            }
            to Attached
            emit InitiateRecycle
        }

        // =====================================================================
        // Absorbed substate transitions — Input Lifecycle
        // =====================================================================

        // QueueAccepted: admit a new input into the queue lane
        transition QueueAccepted {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input QueueAccepted { input_id }
            guard "not_already_tracked" { !self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Queued");
                self.queue_lane.insert(input_id);
                self.input_admission_seq.insert(input_id, self.next_admission_seq);
                self.next_admission_seq += 1;
            }
            to Idle
            emit IngressAccepted
        }

        // StageForRun: stage a queued input for a run
        transition StageForRun {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageForRun { input_id, run_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Staged");
                self.input_run_associations.insert(input_id, run_id);
            }
            to Idle
            emit RecordRunAssociation
        }

        // IncrementAttemptCount: count this stage attempt for an input.
        transition IncrementAttemptCount {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input IncrementAttemptCount { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_attempt_counts.increment(input_id, 1);
            }
            to Idle
        }

        // RollbackStaged: return a staged input to queued
        transition RollbackStaged {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RollbackStaged { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Queued");
                self.input_run_associations.remove(input_id);
            }
            to Idle
            emit InputLifecycleNotice
        }

        // MarkApplied: mark an input as applied
        transition MarkApplied {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkApplied { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Applied");
            }
            to Idle
            emit InputLifecycleNotice
        }

        // MarkAppliedPendingConsumption: Applied → AppliedPendingConsumption
        transition MarkAppliedPendingConsumption {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input MarkAppliedPendingConsumption { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "AppliedPendingConsumption");
            }
            to Idle
            emit InputLifecycleNotice
        }

        // ConsumeOnAccept: direct Accepted → Consumed (skip queue)
        transition ConsumeOnAccept {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ConsumeOnAccept { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Consumed");
                self.queue_lane.remove(input_id);
                self.steer_lane.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // RecordBoundarySeq: record boundary sequence for crash recovery
        transition RecordBoundarySeq {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RecordBoundarySeq { input_id, seq }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_boundary_sequences.insert(input_id, seq);
            }
            to Idle
            emit RecordBoundarySequence
        }

        // ConsumeInput: terminal — mark input consumed, remove from lanes
        transition ConsumeInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ConsumeInput { input_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Consumed");
                self.queue_lane.remove(input_id);
                self.steer_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, "Consumed");
                self.input_superseded_by.remove(input_id);
                self.input_aggregate_id.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // SupersedeInput: terminal — mark input superseded, remove from queue
        transition SupersedeInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SupersedeInput { input_id, superseded_by }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Superseded");
                self.queue_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, "Superseded");
                self.input_superseded_by.insert(input_id, superseded_by);
                self.input_aggregate_id.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // CoalesceInput: terminal — mark input coalesced, remove from queue
        transition CoalesceInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CoalesceInput { input_id, aggregate_id }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Coalesced");
                self.queue_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, "Coalesced");
                self.input_aggregate_id.insert(input_id, aggregate_id);
                self.input_superseded_by.remove(input_id);
                self.input_abandon_reason.remove(input_id);
                self.input_abandon_attempt_count.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // AbandonInput: terminal — mark input abandoned, remove from lanes
        transition AbandonInput {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbandonInput { input_id, reason, attempt_count }
            guard "input_tracked" { self.input_phases.contains_key(input_id) }
            update {
                self.input_phases.insert(input_id, "Abandoned");
                self.queue_lane.remove(input_id);
                self.steer_lane.remove(input_id);
                self.input_terminal_kind.insert(input_id, "Abandoned");
                self.input_abandon_reason.insert(input_id, reason);
                self.input_abandon_attempt_count.insert(input_id, attempt_count);
                self.input_superseded_by.remove(input_id);
                self.input_aggregate_id.remove(input_id);
            }
            to Idle
            emit RecordTerminalOutcome
        }

        // =====================================================================
        // Absorbed substate transitions — Ops Lifecycle
        // =====================================================================

        // RegisterOp: register a new operation as Provisioning
        transition RegisterOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RegisterOp { operation_id, kind }
            guard "not_already_registered" { !self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Provisioning");
                self.op_kinds.insert(operation_id, kind);
                self.op_peer_ready.insert(operation_id, false);
                self.op_progress_counts.insert(operation_id, 0);
                self.active_op_count += 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // StartOp: advance to Running
        transition StartOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StartOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Running");
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // CompleteOp: terminal success — record completion sequence
        transition CompleteOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CompleteOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Completed");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
                self.op_completion_seq.insert(operation_id, self.next_completion_seq);
                self.next_completion_seq += 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // FailOp: terminal failure
        transition FailOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input FailOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Failed");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // CancelOp: terminal cancellation
        transition CancelOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CancelOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Cancelled");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // AbortOp: terminal abort
        transition AbortOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input AbortOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Aborted");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // PeerReadyOp: mark operation's peer as ready
        transition PeerReadyOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input PeerReadyOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_peer_ready.insert(operation_id, true);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // ProgressReportedOp: progress tick (increments per-op counter)
        transition ProgressReportedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input ProgressReportedOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_progress_counts.increment(operation_id, 1);
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // RetireRequestedOp: advance Running -> Retiring (non-terminal).
        // Shell enforces Running pre-state; DSL records the status transition.
        transition RetireRequestedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RetireRequestedOp { operation_id }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Retiring");
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
        }

        // RetireCompletedOp: terminal retirement (Running|Retiring -> Retired).
        transition RetireCompletedOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RetireCompletedOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Retired");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // TerminateOp: bulk-terminate variant used by owner-terminated cascade.
        // Shell loops across non-terminal ops and issues one TerminateOp each;
        // the session lock provides cascade atomicity.
        transition TerminateOp {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input TerminateOp { operation_id, outcome }
            guard "op_registered" { self.op_statuses.contains_key(operation_id) }
            update {
                self.op_statuses.insert(operation_id, "Terminated");
                self.op_terminal_outcomes.insert(operation_id, outcome);
                self.active_op_count -= 1;
            }
            to Idle
            emit SubmitOpEvent { operation_id: operation_id }
            emit NotifyOpWatcher { operation_id: operation_id }
        }

        // RequestWaitAll: activate wait-all barrier with explicit membership.
        // `wait_operation_ids` is DSL-owned; shell must not mirror it.
        transition RequestWaitAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input RequestWaitAll { operation_ids }
            update {
                self.wait_active = true;
                self.wait_operation_ids = operation_ids;
            }
            to Idle
        }

        // SatisfyWaitAll: deactivate wait-all barrier
        transition SatisfyWaitAll {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SatisfyWaitAll
            guard "wait_is_active" { self.wait_active == true }
            update {
                self.wait_active = false;
                self.wait_operation_ids = EmptySet;
            }
            to Idle
            emit WaitAllSatisfied
        }

        // =====================================================================
        // Absorbed substate transitions — Comms Drain
        // =====================================================================

        // SpawnDrain: start a drain task
        transition SpawnDrain {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input SpawnDrain { mode }
            guard "drain_can_spawn" { self.drain_phase == "Inactive" || self.drain_phase == "Stopped" || self.drain_phase == "ExitedRespawnable" }
            update {
                self.drain_phase = "Running";
                self.drain_mode = Some(mode);
            }
            to Idle
            emit SpawnDrainTask
        }

        // StopDrain: stop the running drain
        transition StopDrain {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StopDrain
            guard "drain_is_running" { self.drain_phase == "Running" }
            update {
                self.drain_phase = "Stopped";
            }
            to Idle
        }

        // DrainExitedClean: drain exited cleanly, reset to inactive
        transition DrainExitedClean {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DrainExitedClean
            update {
                self.drain_phase = "Inactive";
                self.drain_mode = None;
            }
            to Idle
        }

        // DrainExitedRespawnable: drain exited but can be respawned
        transition DrainExitedRespawnable {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input DrainExitedRespawnable
            update {
                self.drain_phase = "ExitedRespawnable";
            }
            to Idle
        }

        // =====================================================================
        // Absorbed substate transitions — Visibility
        // =====================================================================

        // StageVisibilityFilter: stage a new tool filter + revision
        transition StageVisibilityFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageVisibilityFilter { filter, revision }
            update {
                self.staged_filter = filter;
                self.staged_visibility_revision = revision;
            }
            to Idle
            emit RefreshVisibleSurfaceSet
        }

        // CommitVisibilityFilter: promote staged filter to active
        transition CommitVisibilityFilter {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CommitVisibilityFilter { filter, revision }
            update {
                self.active_filter = filter;
                self.active_visibility_revision = revision;
            }
            to Idle
            emit RefreshVisibleSurfaceSet
        }

        // StageDeferredNames: stage a set of deferred tool names
        transition StageDeferredNames {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input StageDeferredNames { names }
            update {
                self.staged_deferred_names = names;
            }
            to Idle
            emit RefreshVisibleSurfaceSet
        }

        // CommitDeferredNames: promote staged deferred names to active
        transition CommitDeferredNames {
            per_phase [Idle, Attached, Running, Retired, Stopped]
            on input CommitDeferredNames { names }
            update {
                self.active_deferred_names = names;
            }
            to Idle
            emit RefreshVisibleSurfaceSet
        }
    }
}

// =====================================================================
