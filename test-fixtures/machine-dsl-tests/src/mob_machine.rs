use meerkat_machine_dsl::machine;

machine! {
    machine MobMachine {
        version: 1,
        rust: "meerkat-mob" / "generated::mob_machine",

        state {
            lifecycle_phase: MobPhase,
            live_runtime_ids: Set<AgentRuntimeId>,
            externally_addressable_runtime_ids: Set<AgentRuntimeId>,
            runtime_fence_tokens: Map<AgentRuntimeId, FenceToken>,
            active_run_count: u64,
            pending_spawn_count: u64,
            coordinator_bound: bool,
        }

        init(Running) {
            live_runtime_ids = EmptySet,
            externally_addressable_runtime_ids = EmptySet,
            runtime_fence_tokens = EmptyMap,
            active_run_count = 0,
            pending_spawn_count = 0,
            coordinator_bound = true,
        }

        terminal [Destroyed]

        phase MobPhase {
            Running,
            Stopped,
            Completed,
            Destroyed,
        }

        input MobMachineInput {
            RunFlow,
            CancelFlow,
            FlowStatus,
            Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            Retire { agent_runtime_id: AgentRuntimeId },
            Respawn { agent_runtime_id: AgentRuntimeId },
            RetireAll,
            Wire,
            Unwire,
            ExternalTurn,
            InternalTurn,
            SubmitWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: String },
            CancelWork { work_id: WorkId },
            CancelAllWork { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            Stop,
            Resume,
            Complete,
            Reset,
            Destroy,
            TaskCreate,
            TaskUpdate,
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            SubscribeAgentEvents,
            SubscribeAllAgentEvents,
            SubscribeMobEvents,
            PollEvents,
            ReplayAllEvents,
            RecordOperatorActionProvenance,
            GetMember,
            SetSpawnPolicy,
            Shutdown,
            ForceCancel,
        }

        surface_only [
            FlowStatus,
            TaskList,
            TaskGet,
            McpServerStates,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            CancelWork,
            PollEvents,
            ReplayAllEvents,
            GetMember
        ]

        signal MobMachineSignal {
            ObserveRuntimeReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RetireMember { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            DestroyMob,
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            MarkCompleted,
            StartRun,
            FinishRun,
            BeginCleanup,
            FinishCleanup,
            InitializeOrchestrator,
            BindCoordinator,
            UnbindCoordinator,
            StageSpawn,
            CompleteSpawn,
            StartFlow,
            CompleteFlow,
            StopOrchestrator,
            ResumeOrchestrator,
            DestroyOrchestrator,
            ForceCancelMember,
            MemberPeerExposed,
            MemberTerminalized,
            OperationPeerTrusted,
            PeerInputAdmitted,
            CreateRun,
        }

        effect MobMachineEffect {
            RequestRuntimeBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: String },
            RequestRuntimeRetire,
            RequestRuntimeDestroy,
            EmitMemberLifecycleNotice { kind: String },
            EmitRunLifecycleNotice,
            EmitFlowRunNotice,
            AppendFailureLedger,
            FlowTerminalized,
            EscalateSupervisor,
            NotifyCoordinator,
            ExposePendingSpawn,
            EmitMemberTerminalNotice,
            AdmitPeerInput,
            EmitProgressNote,
            EmitTaskNotice,
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine],
        disposition RequestRuntimeIngress => routed [MeerkatMachine],
        disposition RequestRuntimeRetire => routed [MeerkatMachine],
        disposition RequestRuntimeDestroy => routed [MeerkatMachine],
        disposition EmitMemberLifecycleNotice => external,
        disposition EmitRunLifecycleNotice => external,
        disposition EmitFlowRunNotice => external,
        disposition AppendFailureLedger => local,
        disposition FlowTerminalized => external,
        disposition EscalateSupervisor => external,
        disposition NotifyCoordinator => external,
        disposition ExposePendingSpawn => external,
        disposition EmitMemberTerminalNotice => external,
        disposition AdmitPeerInput => external,
        disposition EmitProgressNote => external,
        disposition EmitTaskNotice => external,

        // =====================================================================
        // Direct transitions
        // =====================================================================

        // Spawn is a Running self-loop: the real runtime starts in Running
        // and does not expose a durable pre-start top-level phase.
        transition SpawnRunning {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "spawned" }
        }

        transition ObserveRuntimeReady {
            on signal ObserveRuntimeReady { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }

        // SubmitWork formally requests runtime ingress on the bound member runtime.
        transition SubmitWorkRunningExternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "external_origin" { origin == "External" }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition SubmitWorkRunningInternal {
            on input SubmitWork { agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "internal_origin" { origin == "Internal" }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition RetireMember {
            on signal RetireMember { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeRetire
        }

        transition ObserveRuntimeRetired {
            on signal ObserveRuntimeRetired { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: "retired" }
        }

        transition ResetMember {
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "reset" }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation }
            emit EmitMemberLifecycleNotice { kind: "respawned" }
        }

        transition MarkCompleted {
            on signal MarkCompleted
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Completed
            emit EmitMemberLifecycleNotice { kind: "completed" }
        }

        transition DestroyMob {
            on signal DestroyMob
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit RequestRuntimeDestroy
        }

        transition ObserveRuntimeDestroyed {
            on signal ObserveRuntimeDestroyed { agent_runtime_id, fence_token }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
                || self.lifecycle_phase == Phase::Destroyed
            }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit EmitMemberLifecycleNotice { kind: "destroyed" }
        }

        // =====================================================================
        // Absorbed transitions: per-phase self-loops
        // =====================================================================

        // RecordOperatorActionProvenance: all 4 phases, no guard, no effect
        transition RecordOperatorActionProvenanceRunning {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition RecordOperatorActionProvenanceStopped {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition RecordOperatorActionProvenanceCompleted {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition RecordOperatorActionProvenanceDestroyed {
            on input RecordOperatorActionProvenance
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // SetSpawnPolicy: all 4 phases, no guard, no effect
        transition SetSpawnPolicyRunning {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SetSpawnPolicyStopped {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SetSpawnPolicyCompleted {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SetSpawnPolicyDestroyed {
            on input SetSpawnPolicy
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Phase-changing transitions
        // =====================================================================

        // Stop: Running -> Stopped
        transition StopRunning {
            on input Stop
            guard { self.lifecycle_phase == Phase::Running }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        // Resume: Stopped -> Running
        transition ResumeStopped {
            on input Resume
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // Complete: Running -> Completed
        transition CompleteRunning {
            on input Complete
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        // Reset: Running|Stopped|Completed -> Running
        transition ResetToRunning {
            on input Reset
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Running self-loops (inputs)
        // =====================================================================

        transition WireRunning {
            on input Wire
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit NotifyCoordinator
        }

        transition ExternalTurnRunning {
            on input ExternalTurn
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitProgressNote
        }

        transition InternalTurnRunning {
            on input InternalTurn
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitProgressNote
        }

        transition TaskCreateRunning {
            on input TaskCreate
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitTaskNotice
        }

        transition TaskUpdateRunning {
            on input TaskUpdate
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitTaskNotice
        }

        transition ForceCancelRunning {
            on input ForceCancel
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        // =====================================================================
        // Subscribe commands
        // =====================================================================

        // SubscribeAgentEvents: all 4 phases, guard: active_members_present
        transition SubscribeAgentEventsRunning {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Running
        }
        transition SubscribeAgentEventsStopped {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Stopped
        }
        transition SubscribeAgentEventsCompleted {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Completed
        }
        transition SubscribeAgentEventsDestroyed {
            on input SubscribeAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Destroyed
        }

        // SubscribeAllAgentEvents: all 4 phases, no guard
        transition SubscribeAllAgentEventsRunning {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeAllAgentEventsStopped {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeAllAgentEventsCompleted {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeAllAgentEventsDestroyed {
            on input SubscribeAllAgentEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // SubscribeMobEvents: all 4 phases, no guard
        transition SubscribeMobEventsRunning {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }
        transition SubscribeMobEventsStopped {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }
        transition SubscribeMobEventsCompleted {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }
        transition SubscribeMobEventsDestroyed {
            on input SubscribeMobEvents
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Shutdown: from any non-Destroyed state
        // =====================================================================

        transition ShutdownRunning {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownStopped {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition ShutdownCompleted {
            on input Shutdown
            guard { self.lifecycle_phase == Phase::Completed }
            update {
                self.coordinator_bound = false;
                self.active_run_count = 0;
            }
            to Completed
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Signal-driven Running self-loops
        // =====================================================================

        transition CancelFlowRunning {
            on input CancelFlow
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        transition InitializeOrchestratorRunning {
            on signal InitializeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition BindCoordinatorRunning {
            on signal BindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit NotifyCoordinator
        }

        transition UnbindCoordinatorRunning {
            on signal UnbindCoordinator
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.coordinator_bound = false;
            }
            to Running
            emit NotifyCoordinator
        }

        transition StageSpawnRunning {
            on signal StageSpawn
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.pending_spawn_count += 1;
            }
            to Running
            emit ExposePendingSpawn
        }

        transition StopOrchestratorRunning {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition StopOrchestratorStopped {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition StopOrchestratorCompleted {
            on signal StopOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ResumeOrchestratorRunning {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = true; }
            to Running
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorStopped {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = true; }
            to Stopped
            emit NotifyCoordinator
        }
        transition ResumeOrchestratorCompleted {
            on signal ResumeOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = true; }
            to Completed
            emit NotifyCoordinator
        }

        transition DestroyOrchestratorRunning {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Running }
            update { self.coordinator_bound = false; }
            to Running
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorStopped {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Stopped }
            update { self.coordinator_bound = false; }
            to Stopped
            emit NotifyCoordinator
        }
        transition DestroyOrchestratorCompleted {
            on signal DestroyOrchestrator
            guard { self.lifecycle_phase == Phase::Completed }
            update { self.coordinator_bound = false; }
            to Completed
            emit NotifyCoordinator
        }

        transition ForceCancelMemberRunning {
            on signal ForceCancelMember
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition MemberPeerExposedRunning {
            on signal MemberPeerExposed
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition MemberTerminalizedRunning {
            on signal MemberTerminalized
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberTerminalNotice
        }

        transition OperationPeerTrustedRunning {
            on signal OperationPeerTrusted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        transition PeerInputAdmittedRunning {
            on signal PeerInputAdmitted
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AdmitPeerInput
        }

        // =====================================================================
        // BeginCleanup / FinishCleanup
        // =====================================================================

        transition BeginCleanupStopped {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition BeginCleanupCompleted {
            on signal BeginCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupStopped {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        transition FinishCleanupCompleted {
            on signal FinishCleanup
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Stopped
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // RunFlow / StartFlow / CreateRun / StartRun
        // =====================================================================

        transition RunFlowRunning {
            on input RunFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition StartFlowRunning {
            on signal StartFlow
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition CreateRunRunning {
            on signal CreateRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition StartRunRunning {
            on signal StartRun
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count += 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Unwire
        // =====================================================================

        transition UnwireRunning {
            on input Unwire
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit NotifyCoordinator
        }

        // =====================================================================
        // CompleteFlow / FinishRun
        // =====================================================================

        transition CompleteFlowRunning {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        transition FinishRunRunning {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Retire / RetireAll
        // =====================================================================

        transition RetireRunning {
            on input Retire { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
            }
            to Running
            emit RequestRuntimeRetire
        }

        transition RetireStopped {
            on input Retire { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
            }
            to Stopped
            emit RequestRuntimeRetire
        }

        transition RetireAllRunning {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: "retiring" }
        }

        transition RetireAllStopped {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: "retiring" }
        }

        // =====================================================================
        // CompleteSpawn
        // =====================================================================

        transition CompleteSpawnRunning {
            on signal CompleteSpawn
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "pending_spawns_present" { self.pending_spawn_count > 0 }
            update {
                self.pending_spawn_count -= 1;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: "spawned" }
        }

        // =====================================================================
        // Destroy (input)
        // =====================================================================

        transition DestroyFromAny {
            on input Destroy
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.coordinator_bound = false;
            }
            to Destroyed
        }

        // =====================================================================
        // Respawn (input, Running self-loop)
        // =====================================================================

        transition RespawnRunning {
            on input Respawn { agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {}
            to Running
            emit ExposePendingSpawn
        }

        // =====================================================================
        // CancelAllWork
        // =====================================================================

        transition CancelAllWorkRunning {
            on input CancelAllWork { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }
    }
}

// ---------------------------------------------------------------------------
// Stub types for compilation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentIdentity(pub String);
impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentRuntimeId(pub String);
impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FenceToken(pub u64);
impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);
impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkId(pub String);
impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Runtime dispatch tests ----

    #[test]
    fn initial_state_is_running() {
        let auth = MobMachineAuthority::new();
        assert_eq!(auth.state.phase(), MobPhase::Running);
        assert!(auth.state.live_runtime_ids.is_empty());
        assert_eq!(auth.state.active_run_count, 0);
        assert!(auth.state.coordinator_bound);
    }

    #[test]
    fn spawn_and_retire_lifecycle() {
        let mut auth = MobMachineAuthority::new();

        // Spawn a member
        let r = MobMachineMutator::apply(
            &mut auth,
            MobMachineInput::Spawn {
                agent_identity: AgentIdentity::from("agent-1"),
                agent_runtime_id: AgentRuntimeId::from("rt-1"),
                fence_token: FenceToken(1),
                generation: Generation(0),
                external_addressable: true,
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, MobPhase::Running);
        assert!(
            auth.state
                .live_runtime_ids
                .contains(&AgentRuntimeId::from("rt-1"))
        );
        assert!(
            auth.state
                .externally_addressable_runtime_ids
                .contains(&AgentRuntimeId::from("rt-1"))
        );

        // Retire the member (signal)
        let r = auth
            .apply_signal(MobMachineSignal::RetireMember {
                agent_runtime_id: AgentRuntimeId::from("rt-1"),
                fence_token: FenceToken(1),
            })
            .unwrap();
        assert_eq!(r.to_phase, MobPhase::Running);

        // Observe runtime retired (signal)
        let r = auth
            .apply_signal(MobMachineSignal::ObserveRuntimeRetired {
                agent_runtime_id: AgentRuntimeId::from("rt-1"),
                fence_token: FenceToken(1),
            })
            .unwrap();
        assert_eq!(r.to_phase, MobPhase::Stopped);
        assert!(auth.state.live_runtime_ids.is_empty());
    }

    #[test]
    fn stop_and_resume() {
        let mut auth = MobMachineAuthority::new();

        // Stop
        let r = MobMachineMutator::apply(&mut auth, MobMachineInput::Stop).unwrap();
        assert_eq!(r.to_phase, MobPhase::Stopped);
        assert!(!auth.state.coordinator_bound);

        // Resume
        let r = MobMachineMutator::apply(&mut auth, MobMachineInput::Resume).unwrap();
        assert_eq!(r.to_phase, MobPhase::Running);
        assert!(auth.state.coordinator_bound);
    }

    #[test]
    fn destroy_is_terminal() {
        let mut auth = MobMachineAuthority::new();
        let r = MobMachineMutator::apply(&mut auth, MobMachineInput::Destroy).unwrap();
        assert_eq!(r.to_phase, MobPhase::Destroyed);

        // Cannot act after destroy
        let r = MobMachineMutator::apply(&mut auth, MobMachineInput::Stop);
        assert!(r.is_err());
    }

    #[test]
    fn reset_returns_to_running() {
        let mut auth = MobMachineAuthority::new();
        MobMachineMutator::apply(&mut auth, MobMachineInput::Stop).unwrap();
        assert_eq!(auth.state.phase(), MobPhase::Stopped);

        let r = MobMachineMutator::apply(&mut auth, MobMachineInput::Reset).unwrap();
        assert_eq!(r.to_phase, MobPhase::Running);
        assert!(auth.state.coordinator_bound);
    }

    // ---- Schema tests ----

    #[test]
    fn schema_validates() {
        let schema = MobMachineState::schema();
        schema
            .validate()
            .expect("mob machine schema should validate");
    }

    // ---- TLA+ rendering ----

    #[test]
    fn schema_renders_tla() {
        let schema = MobMachineState::schema();
        let tla = meerkat_machine_codegen::render_machine_module(&schema);
        assert!(tla.contains("MobMachine"));
        assert!(tla.contains("SpawnRunning"));
        assert!(tla.contains("DestroyMob"));
        assert!(tla.contains("ObserveRuntimeRetired"));
    }

    // ---- Kernel round-trip ----

    #[test]
    fn dsl_dispatch_matches_kernel() {
        let schema = MobMachineState::schema();
        let kernel = meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(schema);

        let mut auth = MobMachineAuthority::new();
        let mut kernel_state = kernel.initial_state().unwrap();

        // Run Spawn through both
        let dsl_r = MobMachineMutator::apply(
            &mut auth,
            MobMachineInput::Spawn {
                agent_identity: AgentIdentity::from("agent-1"),
                agent_runtime_id: AgentRuntimeId::from("rt-1"),
                fence_token: FenceToken(1),
                generation: Generation(0),
                external_addressable: true,
            },
        )
        .unwrap();

        let kernel_input = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: "Spawn".into(),
            fields: std::collections::BTreeMap::from([
                (
                    "agent_identity".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::String("agent-1".into()),
                ),
                (
                    "agent_runtime_id".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::String("rt-1".into()),
                ),
                (
                    "fence_token".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::U64(1),
                ),
                (
                    "generation".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::U64(0),
                ),
                (
                    "external_addressable".into(),
                    meerkat_machine_kernels::test_oracle::KernelValue::Bool(true),
                ),
            ]),
        };
        let kernel_r = kernel.transition(&kernel_state, &kernel_input).unwrap();

        assert_eq!(format!("{:?}", dsl_r.to_phase), kernel_r.next_state.phase);
        assert_eq!(dsl_r.effects.len(), kernel_r.effects.len());

        kernel_state = kernel_r.next_state;

        // Run Stop through both
        let dsl_r = MobMachineMutator::apply(&mut auth, MobMachineInput::Stop);
        let kernel_input = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: "Stop".into(),
            fields: std::collections::BTreeMap::new(),
        };
        let kernel_r = kernel.transition(&kernel_state, &kernel_input);

        assert_eq!(dsl_r.is_ok(), kernel_r.is_ok());
    }
}
