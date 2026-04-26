use meerkat_machine_dsl::machine;

machine! {
    machine MeerkatMachine {
        version: 1,
        rust: "meerkat-runtime" / "generated::meerkat_machine",

        state {
            lifecycle_phase: MeerkatPhase,
            session_id: Option<SessionId>,
            active_runtime_id: Option<AgentRuntimeId>,
            active_fence_token: Option<FenceToken>,
            current_run_id: Option<RunId>,
            pre_run_phase: Option<String>,
            silent_intent_overrides: Set<String>,
        }

        init(Initializing) {
            session_id = None,
            active_runtime_id = None,
            active_fence_token = None,
            current_run_id = None,
            pre_run_phase = None,
            silent_intent_overrides = EmptySet,
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
            PrepareBindings { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId },
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
            StartConversationRun,
            StartImmediateAppend,
            StartImmediateContext,
            ClassifyExternalEnvelope,
            ClassifyPlainEvent,
            EnsureDrainRunning,
            StageAdd,
            StageRemove,
            StageReload,
            ApplySurfaceBoundary,
            PendingSucceeded,
            PendingFailed,
            CallStarted,
            CallFinished,
            FinalizeRemovalClean,
            FinalizeRemovalForced,
            SnapshotAligned,
            ShutdownSurface,
        }

        effect MeerkatMachineEffect {
            RuntimeBound { agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken> },
            RuntimeRetired { agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken> },
            RuntimeDestroyed { agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken> },
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
            SubmitOpEvent,
            NotifyOpWatcher,
            ExposeOperationPeer,
            RetainTerminalRecord,
            EvictCompletedRecord,
            CompletionProduced { seq: u64, operation_id: OperationId, kind: OperationKind },
            WaitAllSatisfied,
            CollectCompletedResult,
            EnqueueClassifiedEntry,
            SpawnDrainTask,
            ScheduleSurfaceCompletion,
            RefreshVisibleSurfaceSet,
            EmitExternalToolDelta,
            CloseSurfaceConnection,
            RejectSurfaceCall,
        }

        // =====================================================================
        // Effect dispositions
        // =====================================================================

        disposition RuntimeBound => routed [MobMachine],
        disposition RuntimeRetired => routed [MobMachine],
        disposition RuntimeDestroyed => routed [MobMachine],
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
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Initializing }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Initializing
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }
        // Idle → Attached
        transition PrepareBindingsIdle {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }
        // Attached → Attached
        transition PrepareBindingsAttached {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Attached
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }
        // Running → Running
        transition PrepareBindingsRunning {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Running
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }
        // Retired → Retired
        transition PrepareBindingsRetired {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Retired }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Retired
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }
        // Stopped → Stopped (inline in hand-written catalog)
        transition PrepareBindingsStopped {
            on input PrepareBindings { agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.active_runtime_id = Some(agent_runtime_id);
                self.active_fence_token = Some(fence_token);
            }
            to Stopped
            emit RuntimeBound { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
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
            emit RuntimeRetired { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
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
            guard "runtime_is_bound" { self.active_runtime_id != None }
            update {
                self.current_run_id = None;
                self.pre_run_phase = None;
                self.silent_intent_overrides = EmptySet;
            }
            to Destroyed
            emit RuntimeDestroyed { agent_runtime_id: self.active_runtime_id, fence_token: self.active_fence_token }
        }

        // =====================================================================
        // Absorbed transitions
        // =====================================================================

        // 18. EnsureSessionWithExecutor
        // Idle → Attached (phase change)
        transition EnsureSessionWithExecutorIdle {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Idle }
            update {}
            to Attached
        }
        // Attached, Running: self-loop
        transition EnsureSessionWithExecutorAttached {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Attached }
            update {}
            to Attached
        }
        transition EnsureSessionWithExecutorRunning {
            on input EnsureSessionWithExecutor { session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
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

        // 30. StartConversationRun/StartImmediateAppend/StartImmediateContext:
        //     Attached self-loops (signals), emit SubmitRunPrimitive
        transition StartConversationRunAttached {
            on signal StartConversationRun
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
        }
        transition StartImmediateAppendAttached {
            on signal StartImmediateAppend
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
        }
        transition StartImmediateContextAttached {
            on signal StartImmediateContext
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit SubmitRunPrimitive
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

        // 33. MCP surface signals: Attached/Running per-phase, emit EmitExternalToolDelta (or other)
        transition StageAddAttached {
            on signal StageAdd
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageAddRunning {
            on signal StageAdd
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition StageRemoveAttached {
            on signal StageRemove
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageRemoveRunning {
            on signal StageRemove
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition StageReloadAttached {
            on signal StageReload
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition StageReloadRunning {
            on signal StageReload
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition ApplySurfaceBoundaryAttached {
            on signal ApplySurfaceBoundary
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit ScheduleSurfaceCompletion
        }
        transition ApplySurfaceBoundaryRunning {
            on signal ApplySurfaceBoundary
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit ScheduleSurfaceCompletion
        }
        transition PendingSucceededAttached {
            on signal PendingSucceeded
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition PendingSucceededRunning {
            on signal PendingSucceeded
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition PendingFailedAttached {
            on signal PendingFailed
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition PendingFailedRunning {
            on signal PendingFailed
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition CallStartedAttached {
            on signal CallStarted
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
        }
        transition CallStartedRunning {
            on signal CallStarted
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
        }
        transition CallFinishedAttached {
            on signal CallFinished
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
        }
        transition CallFinishedRunning {
            on signal CallFinished
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
        }
        transition FinalizeRemovalCleanAttached {
            on signal FinalizeRemovalClean
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalCleanRunning {
            on signal FinalizeRemovalClean
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalForcedAttached {
            on signal FinalizeRemovalForced
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition FinalizeRemovalForcedRunning {
            on signal FinalizeRemovalForced
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition SnapshotAlignedAttached {
            on signal SnapshotAligned
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition SnapshotAlignedRunning {
            on signal SnapshotAligned
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
        }
        transition ShutdownSurfaceAttached {
            on signal ShutdownSurface
            guard { self.lifecycle_phase == Phase::Attached }
            guard "session_registered" { self.session_id != None }
            update {}
            to Attached
            emit EmitExternalToolDelta
        }
        transition ShutdownSurfaceRunning {
            on signal ShutdownSurface
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_registered" { self.session_id != None }
            update {}
            to Running
            emit EmitExternalToolDelta
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
    }
}

// =====================================================================
// Stub types for compilation
// =====================================================================

macro_rules! stub_newtype {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub String);
        impl<T: Into<String>> From<T> for $name {
            fn from(s: T) -> Self {
                Self(s.into())
            }
        }
    };
}

stub_newtype!(SessionId);
stub_newtype!(AgentRuntimeId);
stub_newtype!(FenceToken);
stub_newtype!(RunId);
stub_newtype!(InputId);
stub_newtype!(WorkId);
stub_newtype!(OperationId);
stub_newtype!(SessionLlmIdentity);
stub_newtype!(SessionToolVisibilityState);
stub_newtype!(SessionLlmCapabilitySurface);
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionLlmCapabilitySurfaceStatus {
    Resolved,
    #[default]
    Unresolved,
}
stub_newtype!(SessionToolVisibilityDelta);
stub_newtype!(ToolFilter);
stub_newtype!(ToolVisibilityWitness);
stub_newtype!(Generation);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OperationKind {
    ToolCall,
    Completion,
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_machine_schema::identity::{FieldId, InputVariantId, PhaseId, SignalVariantId};

    fn fid(s: &str) -> FieldId {
        FieldId::parse(s).expect("valid field slug")
    }

    fn ivid(s: &str) -> InputVariantId {
        InputVariantId::parse(s).expect("valid input variant slug")
    }

    fn svid(s: &str) -> SignalVariantId {
        SignalVariantId::parse(s).expect("valid signal variant slug")
    }

    fn assert_phase_eq(dsl_phase: impl std::fmt::Debug, kernel_phase: &PhaseId) {
        assert_eq!(format!("{:?}", dsl_phase), kernel_phase.as_str());
    }

    // ---- Direction 1: runtime dispatch works ----

    #[test]
    fn initial_state_is_initializing() {
        let auth = MeerkatMachineAuthority::new();
        assert_eq!(auth.state.phase(), MeerkatPhase::Initializing);
        assert!(auth.state.session_id.is_none());
        assert!(auth.state.active_runtime_id.is_none());
        assert!(auth.state.active_fence_token.is_none());
        assert!(auth.state.current_run_id.is_none());
        assert!(auth.state.pre_run_phase.is_none());
        assert!(auth.state.silent_intent_overrides.is_empty());
    }

    #[test]
    fn initialize_to_idle() {
        let mut auth = MeerkatMachineAuthority::new();
        let r = auth
            .apply_signal(MeerkatMachineSignal::Initialize)
            .expect("Initialize should succeed");
        assert_eq!(r.to_phase, MeerkatPhase::Idle);
    }

    #[test]
    fn register_and_prepare_bindings_lifecycle() {
        let mut auth = MeerkatMachineAuthority::new();
        // Initialize
        auth.apply_signal(MeerkatMachineSignal::Initialize).unwrap();
        assert_eq!(auth.state.phase(), MeerkatPhase::Idle);

        // RegisterSession
        let r = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::RegisterSession {
                session_id: "sess-1".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Idle);
        assert_eq!(auth.state.session_id, Some("sess-1".into()));

        // PrepareBindings: Idle → Attached
        let r = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: "rt-1".into(),
                fence_token: "fence-1".into(),
                generation: "gen-1".into(),
                session_id: "sess-1".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Attached);
        assert_eq!(auth.state.active_runtime_id, Some("rt-1".into()));
        assert_eq!(auth.state.active_fence_token, Some("fence-1".into()));
        // Should emit RuntimeBound
        assert_eq!(r.effects.len(), 1);
    }

    #[test]
    fn prepare_then_commit_run_lifecycle() {
        let mut auth = MeerkatMachineAuthority::new();
        auth.apply_signal(MeerkatMachineSignal::Initialize).unwrap();
        MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::RegisterSession {
                session_id: "sess-1".into(),
            },
        )
        .unwrap();
        MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: "rt-1".into(),
                fence_token: "fence-1".into(),
                generation: "gen-1".into(),
                session_id: "sess-1".into(),
            },
        )
        .unwrap();
        assert_eq!(auth.state.phase(), MeerkatPhase::Attached);

        // Prepare: Attached → Running
        let r = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::Prepare {
                session_id: "sess-1".into(),
                run_id: "run-1".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Running);
        assert_eq!(auth.state.current_run_id, Some("run-1".into()));
        assert_eq!(auth.state.pre_run_phase, Some("attached".into()));

        // Commit: Running → Attached
        let r = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::Commit {
                input_id: "input-1".into(),
                run_id: "run-1".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Attached);
        assert!(auth.state.current_run_id.is_none());
        assert!(auth.state.pre_run_phase.is_none());
    }

    #[test]
    fn retire_and_destroy() {
        let mut auth = MeerkatMachineAuthority::new();
        auth.apply_signal(MeerkatMachineSignal::Initialize).unwrap();
        MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: "rt-1".into(),
                fence_token: "fence-1".into(),
                generation: "gen-1".into(),
                session_id: "sess-1".into(),
            },
        )
        .unwrap();
        assert_eq!(auth.state.phase(), MeerkatPhase::Attached);

        // Retire: Attached → Retired
        let r = MeerkatMachineMutator::apply(&mut auth, MeerkatMachineInput::Retire).unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Retired);

        // Destroy: Retired → Destroyed
        let r = MeerkatMachineMutator::apply(&mut auth, MeerkatMachineInput::Destroy).unwrap();
        assert_eq!(r.to_phase, MeerkatPhase::Destroyed);
    }

    // ---- Direction 2: Schema validates ----

    #[test]
    fn schema_validates() {
        // Mirror the meerkat-machine catalog binding set from
        // `meerkat-machine-schema/src/catalog/dsl/mod.rs::dsl_meerkat_machine`.
        // B-4 (`c0cb12071`) made `MachineSchema.named_types` validation-
        // gated; catalogs populate via `with_named_types`, DSL macro
        // emits `vec![]`, so the test fixture must populate inline.
        use meerkat_machine_schema::identity::NamedTypeBinding;
        let mut schema = MeerkatMachineState::schema();
        schema.named_types = vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string("MeerkatPhase"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string("OperationKind"),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("SessionLlmCapabilitySurface"),
            NamedTypeBinding::string("SessionLlmCapabilitySurfaceStatus"),
            NamedTypeBinding::string("SessionLlmIdentity"),
            NamedTypeBinding::string("SessionToolVisibilityDelta"),
            NamedTypeBinding::string("SessionToolVisibilityState"),
            NamedTypeBinding::string("ToolFilter"),
            NamedTypeBinding::string("ToolVisibilityWitness"),
            NamedTypeBinding::string("WorkId"),
            // PeerEndpoint etc. are type_path bindings in the catalog;
            // for this test-fixture lib-scope validation, string is
            // sufficient (validation checks binding existence, not the
            // Rust-atom shape).
            NamedTypeBinding::string("PeerEndpoint"),
            NamedTypeBinding::string("PeerName"),
            NamedTypeBinding::string("PeerId"),
            NamedTypeBinding::string("PeerAddress"),
        ];
        schema
            .validate()
            .expect("meerkat machine schema should validate");
    }

    // ---- Direction 3: TLA+ rendering ----

    #[test]
    fn schema_renders_tla() {
        let schema = MeerkatMachineState::schema();
        let tla = meerkat_machine_codegen::render_machine_module(&schema);

        assert!(
            tla.contains("---- MODULE Machine_MeerkatMachine ----"),
            "TLA+ should contain module header"
        );
        assert!(
            tla.contains("TRANSITIONS"),
            "TLA+ should contain TRANSITIONS section"
        );
        assert!(
            tla.contains("Initialize"),
            "TLA+ should contain Initialize transition"
        );
        assert!(
            tla.contains("===="),
            "TLA+ should end with module terminator"
        );
    }

    // ---- Direction 5: DSL dispatch matches kernel ----

    #[test]
    fn dsl_dispatch_matches_kernel() {
        fn named_string(
            type_name: &str,
            value: &str,
        ) -> meerkat_machine_kernels::test_oracle::KernelValue {
            meerkat_machine_kernels::test_oracle::KernelValue::Named {
                type_name: meerkat_machine_schema::identity::NamedTypeId::parse(type_name).unwrap(),
                value: Box::new(meerkat_machine_kernels::test_oracle::KernelValue::String(
                    value.into(),
                )),
            }
        }

        fn named_u64(
            type_name: &str,
            value: u64,
        ) -> meerkat_machine_kernels::test_oracle::KernelValue {
            meerkat_machine_kernels::test_oracle::KernelValue::Named {
                type_name: meerkat_machine_schema::identity::NamedTypeId::parse(type_name).unwrap(),
                value: Box::new(meerkat_machine_kernels::test_oracle::KernelValue::U64(
                    value,
                )),
            }
        }

        let mut schema = MeerkatMachineState::schema();
        schema.named_types = vec![
            meerkat_machine_schema::identity::NamedTypeBinding::u64("BoundarySequence"),
            meerkat_machine_schema::identity::NamedTypeBinding::u64("FenceToken"),
            meerkat_machine_schema::identity::NamedTypeBinding::u64("Generation"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("AgentRuntimeId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("CommsRuntimeId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("InputId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("McpServerId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("MeerkatPhase"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("MobId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("OperationId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("OperationKind"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("PeerCorrelationId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("RunId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("SessionId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string(
                "SessionLlmCapabilitySurface",
            ),
            meerkat_machine_schema::identity::NamedTypeBinding::string(
                "SessionLlmCapabilitySurfaceStatus",
            ),
            meerkat_machine_schema::identity::NamedTypeBinding::string("SessionLlmIdentity"),
            meerkat_machine_schema::identity::NamedTypeBinding::string(
                "SessionToolVisibilityDelta",
            ),
            meerkat_machine_schema::identity::NamedTypeBinding::string(
                "SessionToolVisibilityState",
            ),
            meerkat_machine_schema::identity::NamedTypeBinding::string("ToolFilter"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("ToolVisibilityWitness"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("WorkId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("PeerEndpoint"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("PeerName"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("PeerId"),
            meerkat_machine_schema::identity::NamedTypeBinding::string("PeerAddress"),
        ];
        let kernel = meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(schema);

        // Run the same sequence through both dispatchers:
        // Initialize (signal) → RegisterSession → PrepareBindings →
        // StopRuntimeExecutor (stays Attached) → Retire → Destroy
        let mut auth = MeerkatMachineAuthority::new();
        let mut kernel_state = kernel.initial_state().unwrap();

        // Initialize (signal)
        auth.apply_signal(MeerkatMachineSignal::Initialize).unwrap();
        // Advance kernel with Initialize signal
        let init_signal = meerkat_machine_kernels::test_oracle::KernelSignal {
            variant: svid("Initialize"),
            fields: std::collections::BTreeMap::new(),
        };
        let ko = kernel
            .transition_signal(&kernel_state, &init_signal)
            .unwrap();
        kernel_state = ko.next_state;

        // RegisterSession
        let dsl_result = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::RegisterSession {
                session_id: "s1".into(),
            },
        );
        let ki = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: ivid("RegisterSession"),
            fields: std::collections::BTreeMap::from([(
                fid("session_id"),
                named_string("SessionId", "s1"),
            )]),
        };
        let kr = kernel.transition(&kernel_state, &ki);
        assert_eq!(
            dsl_result.is_ok(),
            kr.is_ok(),
            "RegisterSession: DSL={}, kernel={}",
            dsl_result.is_ok(),
            kr.is_ok()
        );
        if let (Ok(d), Ok(k)) = (&dsl_result, &kr) {
            assert_phase_eq(&d.to_phase, &k.next_state.phase);
            kernel_state = k.next_state.clone();
        }

        // PrepareBindings
        let dsl_result = MeerkatMachineMutator::apply(
            &mut auth,
            MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: "rt-1".into(),
                fence_token: "ft-1".into(),
                generation: "gen-1".into(),
                session_id: "s1".into(),
            },
        );
        let ki = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: ivid("PrepareBindings"),
            fields: std::collections::BTreeMap::from([
                (
                    fid("agent_runtime_id"),
                    named_string("AgentRuntimeId", "rt-1"),
                ),
                (fid("fence_token"), named_u64("FenceToken", 1)),
                (fid("generation"), named_u64("Generation", 1)),
                (fid("session_id"), named_string("SessionId", "s1")),
            ]),
        };
        let kr = kernel.transition(&kernel_state, &ki);
        assert_eq!(
            dsl_result.is_ok(),
            kr.is_ok(),
            "PrepareBindings: DSL={}, kernel={}",
            dsl_result.is_ok(),
            kr.is_ok()
        );
        if let (Ok(d), Ok(k)) = (&dsl_result, &kr) {
            assert_phase_eq(&d.to_phase, &k.next_state.phase);
            assert_eq!(d.effects.len(), k.effects.len());
            kernel_state = k.next_state.clone();
        }

        // StopRuntimeExecutor (from Attached → stays Attached)
        let dsl_result =
            MeerkatMachineMutator::apply(&mut auth, MeerkatMachineInput::StopRuntimeExecutor);
        let ki = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: ivid("StopRuntimeExecutor"),
            fields: std::collections::BTreeMap::new(),
        };
        let kr = kernel.transition(&kernel_state, &ki);
        assert_eq!(
            dsl_result.is_ok(),
            kr.is_ok(),
            "StopRuntimeExecutor: DSL={}, kernel={}",
            dsl_result.is_ok(),
            kr.is_ok()
        );
        if let (Ok(d), Ok(k)) = (&dsl_result, &kr) {
            assert_phase_eq(&d.to_phase, &k.next_state.phase);
            kernel_state = k.next_state.clone();
        }
    }
}
