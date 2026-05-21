use super::OptionValueExt;

#[macro_export]
macro_rules! mob_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        meerkat_machine_dsl::machine! {
    machine MobMachine {
        version: 2,
        rust: $rust_crate / $rust_module,

        state {
            lifecycle_phase: MobPhase,
            destroy_admitted: bool,
            live_runtime_ids: Set<AgentRuntimeId>,
            externally_addressable_runtime_ids: Set<AgentRuntimeId>,
            runtime_fence_tokens: Map<AgentRuntimeId, FenceToken>,
            identity_runtime_generations: Map<AgentIdentity, Generation>,
            identity_runtime_fence_tokens: Map<AgentIdentity, FenceToken>,
            active_run_count: u64,
            run_status: Map<RunId, Enum<FlowRunStatus>>,
            run_ordered_steps: Map<RunId, Seq<StepId>>,
            run_tracked_steps: Map<RunId, Set<StepId>>,
            run_step_status: Map<RunId, Map<StepId, Option<Enum<StepRunStatus>>>>,
            run_step_status_flat: Map<RunStepKey, Enum<StepRunStatus>>,
            run_output_recorded: Map<RunId, Map<StepId, bool>>,
            run_step_condition_results_flat: Map<RunStepKey, Option<bool>>,
            run_step_condition_results: Map<RunId, Map<StepId, Option<bool>>>,
            run_step_has_conditions: Map<RunId, Map<StepId, bool>>,
            run_step_dependencies: Map<RunId, Map<StepId, Seq<StepId>>>,
            run_step_dependency_modes: Map<RunId, Map<StepId, Enum<DependencyMode>>>,
            run_step_branches: Map<RunId, Map<StepId, Option<BranchId>>>,
            run_step_collection_policies: Map<RunId, Map<StepId, Enum<CollectionPolicyKind>>>,
            run_step_quorum_thresholds: Map<RunId, Map<StepId, u32>>,
            run_step_target_counts: Map<RunId, Map<StepId, u64>>,
            run_step_target_success_counts: Map<RunId, Map<StepId, u64>>,
            run_step_target_terminal_failure_counts: Map<RunId, Map<StepId, u64>>,
            run_output_recorded_flat: Map<RunStepKey, bool>,
            run_step_target_counts_flat: Map<RunStepKey, u64>,
            run_step_target_success_counts_flat: Map<RunStepKey, u64>,
            run_step_target_terminal_failure_counts_flat: Map<RunStepKey, u64>,
            run_target_retry_counts: Map<RunId, Map<String, u64>>,
            run_target_retry_counts_flat: Map<RunStepKey, u64>,
            run_failure_count: Map<RunId, u64>,
            run_consecutive_failure_count: Map<RunId, u64>,
            run_escalation_threshold: Map<RunId, u64>,
            run_max_step_retries: Map<RunId, u32>,
            run_ready_frames: Map<RunId, Seq<FrameId>>,
            run_ready_frame_membership: Map<RunId, Set<FrameId>>,
            run_ready_frame_membership_flat: Set<FrameId>,
            run_pending_body_frame_loops: Map<RunId, Seq<LoopInstanceId>>,
            run_pending_body_frame_loop_membership: Map<RunId, Set<LoopInstanceId>>,
            run_pending_body_frame_loop_membership_flat: Set<LoopInstanceId>,
            run_active_node_count: Map<RunId, u64>,
            run_active_frame_count: Map<RunId, u64>,
            run_last_granted_frame: Map<RunId, FrameId>,
            run_last_granted_loop: Map<RunId, LoopInstanceId>,
            run_max_active_nodes: Map<RunId, u64>,
            run_max_active_frames: Map<RunId, u64>,
            run_max_frame_depth: Map<RunId, u64>,
            frame_scope: Map<FrameId, Enum<FrameScope>>,
            frame_phase: Map<FrameId, Enum<FrameStatus>>,
            frame_run: Map<FrameId, RunId>,
            frame_parent_loop: Map<FrameId, Option<LoopInstanceId>>,
            frame_iteration: Map<FrameId, u32>,
            frame_tracked_nodes: Map<FrameId, Set<FlowNodeId>>,
            frame_ordered_nodes: Map<FrameId, Seq<FlowNodeId>>,
            frame_node_kind: Map<FrameId, Map<FlowNodeId, Enum<FlowNodeKind>>>,
            frame_node_dependencies: Map<FrameId, Map<FlowNodeId, Seq<FlowNodeId>>>,
            frame_node_dependency_modes: Map<FrameId, Map<FlowNodeId, Enum<DependencyMode>>>,
            frame_node_step_ids: Map<FrameId, Map<FlowNodeId, StepId>>,
            frame_node_loop_ids: Map<FrameId, Map<FlowNodeId, LoopId>>,
            frame_node_status: Map<FrameId, Map<FlowNodeId, Enum<NodeRunStatus>>>,
            frame_ready_queue: Map<FrameId, Seq<FlowNodeId>>,
            frame_output_recorded: Map<FrameId, Map<FlowNodeId, bool>>,
            frame_output_recorded_flat: Map<FrameNodeKey, bool>,
            frame_last_admitted_node: Map<FrameId, FlowNodeId>,
            frame_node_condition_results: Map<FrameId, Map<FlowNodeId, Option<bool>>>,
            frame_node_branches: Map<FrameId, Map<FlowNodeId, Option<BranchId>>>,
            loop_phase: Map<LoopInstanceId, Enum<LoopStatus>>,
            loop_parent_frame: Map<LoopInstanceId, FrameId>,
            loop_parent_node: Map<LoopInstanceId, FlowNodeId>,
            loop_definition: Map<LoopInstanceId, LoopId>,
            loop_depth: Map<LoopInstanceId, u32>,
            loop_stage: Map<LoopInstanceId, Enum<LoopIterationStage>>,
            loop_current_iteration: Map<LoopInstanceId, u64>,
            loop_last_completed_iteration: Map<LoopInstanceId, u64>,
            loop_max_iterations: Map<LoopInstanceId, u64>,
            loop_active_body_frame: Map<LoopInstanceId, Option<FrameId>>,
            pending_spawn_count: u64,
            pending_spawn_sessions: Map<AgentIdentity, SessionId>,
            coordinator_bound: bool,
            member_startup_binding_requested: Set<AgentRuntimeId>,
            member_startup_runtime_ready: Set<AgentRuntimeId>,
            member_startup_ready: Set<AgentRuntimeId>,
            member_kickoff_pending: Set<String>,
            member_kickoff_starting: Set<String>,
            member_kickoff_callback_pending: Set<String>,
            member_kickoff_started: Set<String>,
            member_kickoff_failed: Set<String>,
            member_kickoff_cancelled: Set<String>,
            member_kickoff_error: Map<String, String>,
            // Identity-level restore failures are lifecycle facts owned by
            // MobMachine. Projection code may surface the reason, but the
            // Broken/terminal classification comes from this map.
            member_restore_failures: Map<AgentIdentity, String>,
            // Per-runtime lifecycle marker (Active vs Retiring). Tracks the
            // draining/retiring sub-state independently of the mob-level
            // lifecycle phase so generated SubmitWork authority can decide
            // whether fresh work may route while retire-drain is in flight.
            member_state_markers: Map<AgentRuntimeId, MobMemberState>,
            // Undirected wiring edges between agent identities. Stored as
            // ordered pairs (smaller identity first) wrapped in WiringEdge
            // so the DSL sees a single opaque key type.
            wiring_edges: Set<WiringEdge>,
            // Descriptor-bearing external peer trust edges. These are not
            // member-to-member wiring edges: the endpoint carries the external
            // peer's routing id, address, and signing key, so it has its own
            // machine-owned fact instead of being projected into WiringEdge by
            // peer name.
            external_peer_edges: Set<ExternalPeerEdge>,
            external_peer_edges_by_key: Map<ExternalPeerKey, ExternalPeerEdge>,
            // Identity → current runtime binding. Survives within a
            // generation; respawn replaces the runtime id for the same
            // identity.
            identity_to_runtime: Map<AgentIdentity, AgentRuntimeId>,
            member_session_bindings: Map<AgentIdentity, SessionId>,
            member_peer_ids: Map<AgentIdentity, PeerId>,
            member_peer_endpoints: Map<AgentIdentity, MemberPeerEndpoint>,
            pending_session_ingress_detach_runtime_ids: Set<AgentRuntimeId>,
            // Dynamic auto-spawn policy facts. The opaque callback remains a
            // shell observation source, but MobMachine owns whether policy is
            // enabled, the active revision, and every typed resolution that
            // can admit unknown-member external work.
            spawn_policy_enabled: bool,
            spawn_policy_revision: u64,
            spawn_policy_resolution_revision: Map<AgentIdentity, u64>,
            spawn_policy_resolution_profiles: Map<AgentIdentity, String>,
            spawn_policy_resolution_runtime_modes: Map<AgentIdentity, Option<Enum<SpawnPolicyRuntimeMode>>>,
            spawn_policy_resolution_absent: Set<AgentIdentity>,
            topology_epoch: u64,
        }

        init(Running) {
            destroy_admitted = false,
            live_runtime_ids = EmptySet,
            externally_addressable_runtime_ids = EmptySet,
            runtime_fence_tokens = EmptyMap,
            identity_runtime_generations = EmptyMap,
            identity_runtime_fence_tokens = EmptyMap,
            active_run_count = 0,
            run_status = EmptyMap,
            run_ordered_steps = EmptyMap,
            run_tracked_steps = EmptyMap,
            run_step_status = EmptyMap,
            run_step_status_flat = EmptyMap,
            run_output_recorded = EmptyMap,
            run_step_condition_results_flat = EmptyMap,
            run_step_condition_results = EmptyMap,
            run_step_has_conditions = EmptyMap,
            run_step_dependencies = EmptyMap,
            run_step_dependency_modes = EmptyMap,
            run_step_branches = EmptyMap,
            run_step_collection_policies = EmptyMap,
            run_step_quorum_thresholds = EmptyMap,
            run_step_target_counts = EmptyMap,
            run_step_target_success_counts = EmptyMap,
            run_step_target_terminal_failure_counts = EmptyMap,
            run_output_recorded_flat = EmptyMap,
            run_step_target_counts_flat = EmptyMap,
            run_step_target_success_counts_flat = EmptyMap,
            run_step_target_terminal_failure_counts_flat = EmptyMap,
            run_target_retry_counts = EmptyMap,
            run_target_retry_counts_flat = EmptyMap,
            run_failure_count = EmptyMap,
            run_consecutive_failure_count = EmptyMap,
            run_escalation_threshold = EmptyMap,
            run_max_step_retries = EmptyMap,
            run_ready_frames = EmptyMap,
            run_ready_frame_membership = EmptyMap,
            run_ready_frame_membership_flat = EmptySet,
            run_pending_body_frame_loops = EmptyMap,
            run_pending_body_frame_loop_membership = EmptyMap,
            run_pending_body_frame_loop_membership_flat = EmptySet,
            run_active_node_count = EmptyMap,
            run_active_frame_count = EmptyMap,
            run_last_granted_frame = EmptyMap,
            run_last_granted_loop = EmptyMap,
            run_max_active_nodes = EmptyMap,
            run_max_active_frames = EmptyMap,
            run_max_frame_depth = EmptyMap,
            frame_scope = EmptyMap,
            frame_phase = EmptyMap,
            frame_run = EmptyMap,
            frame_parent_loop = EmptyMap,
            frame_iteration = EmptyMap,
            frame_tracked_nodes = EmptyMap,
            frame_ordered_nodes = EmptyMap,
            frame_node_kind = EmptyMap,
            frame_node_dependencies = EmptyMap,
            frame_node_dependency_modes = EmptyMap,
            frame_node_step_ids = EmptyMap,
            frame_node_loop_ids = EmptyMap,
            frame_node_status = EmptyMap,
            frame_ready_queue = EmptyMap,
            frame_output_recorded = EmptyMap,
            frame_output_recorded_flat = EmptyMap,
            frame_last_admitted_node = EmptyMap,
            frame_node_condition_results = EmptyMap,
            frame_node_branches = EmptyMap,
            loop_phase = EmptyMap,
            loop_parent_frame = EmptyMap,
            loop_parent_node = EmptyMap,
            loop_definition = EmptyMap,
            loop_depth = EmptyMap,
            loop_stage = EmptyMap,
            loop_current_iteration = EmptyMap,
            loop_last_completed_iteration = EmptyMap,
            loop_max_iterations = EmptyMap,
            loop_active_body_frame = EmptyMap,
            pending_spawn_count = 0,
            pending_spawn_sessions = EmptyMap,
            coordinator_bound = true,
            member_startup_binding_requested = EmptySet,
            member_startup_runtime_ready = EmptySet,
            member_startup_ready = EmptySet,
            member_kickoff_pending = EmptySet,
            member_kickoff_starting = EmptySet,
            member_kickoff_callback_pending = EmptySet,
            member_kickoff_started = EmptySet,
            member_kickoff_failed = EmptySet,
            member_kickoff_cancelled = EmptySet,
            member_kickoff_error = EmptyMap,
            member_restore_failures = EmptyMap,
            member_state_markers = EmptyMap,
            wiring_edges = EmptySet,
            external_peer_edges = EmptySet,
            external_peer_edges_by_key = EmptyMap,
            identity_to_runtime = EmptyMap,
            member_session_bindings = EmptyMap,
            member_peer_ids = EmptyMap,
            member_peer_endpoints = EmptyMap,
            pending_session_ingress_detach_runtime_ids = EmptySet,
            spawn_policy_enabled = false,
            spawn_policy_revision = 0,
            spawn_policy_resolution_revision = EmptyMap,
            spawn_policy_resolution_profiles = EmptyMap,
            spawn_policy_resolution_runtime_modes = EmptyMap,
            spawn_policy_resolution_absent = EmptySet,
            topology_epoch = 0,
        }

        terminal [Destroyed]

        phase MobPhase {
            Running,
            Stopped,
            Completed,
            Destroyed,
        }

        input MobMachineInput {
            RunFlow {
                run_id: RunId,
                step_ids: Set<StepId>,
                ordered_steps: Seq<StepId>,
                step_has_conditions: Map<StepId, bool>,
                step_dependencies: Map<StepId, Seq<StepId>>,
                step_dependency_modes: Map<StepId, Enum<DependencyMode>>,
                step_branches: Map<StepId, Option<BranchId>>,
                step_collection_policies: Map<StepId, Enum<CollectionPolicyKind>>,
                step_quorum_thresholds: Map<StepId, u32>,
                escalation_threshold: u64,
                max_step_retries: u32,
                max_active_nodes: u64,
                max_active_frames: u64,
                max_frame_depth: u64,
            },
            CreateRunSeed {
                run_id: RunId,
                step_ids: Set<StepId>,
                ordered_steps: Seq<StepId>,
                step_has_conditions: Map<StepId, bool>,
                step_dependencies: Map<StepId, Seq<StepId>>,
                step_dependency_modes: Map<StepId, Enum<DependencyMode>>,
                step_branches: Map<StepId, Option<BranchId>>,
                step_collection_policies: Map<StepId, Enum<CollectionPolicyKind>>,
                step_quorum_thresholds: Map<StepId, u32>,
                escalation_threshold: u64,
                max_step_retries: u32,
                max_active_nodes: u64,
                max_active_frames: u64,
                max_frame_depth: u64,
            },
            CreateFrameSeed {
                run_id: RunId,
                frame_id: FrameId,
                frame_scope: Enum<FrameScope>,
                loop_instance_id: Option<LoopInstanceId>,
                iteration: u32,
                tracked_nodes: Set<FlowNodeId>,
                ordered_nodes: Seq<FlowNodeId>,
                node_kind: Map<FlowNodeId, Enum<FlowNodeKind>>,
                node_dependencies: Map<FlowNodeId, Seq<FlowNodeId>>,
                node_dependency_modes: Map<FlowNodeId, Enum<DependencyMode>>,
                node_branches: Map<FlowNodeId, Option<BranchId>>,
                node_step_ids: Map<FlowNodeId, StepId>,
                node_loop_ids: Map<FlowNodeId, LoopId>,
                node_status: Map<FlowNodeId, Enum<NodeRunStatus>>,
                ready_queue: Seq<FlowNodeId>,
            },
            CreateLoopSeed {
                loop_instance_id: LoopInstanceId,
                parent_frame_id: FrameId,
                parent_node_id: FlowNodeId,
                loop_id: LoopId,
                depth: u32,
                max_iterations: u64,
            },
            RecordLoopBodyFrameCompleted {
                loop_instance_id: LoopInstanceId,
                iteration: u64,
            },
            RecordLoopUntilConditionMet {
                loop_instance_id: LoopInstanceId,
                iteration: u64,
            },
            RecordLoopUntilConditionFailed {
                loop_instance_id: LoopInstanceId,
                iteration: u64,
            },
            AuthorizeFlowRunReducerCommand {
                run_id: RunId,
                command: Enum<FlowRunReducerCommandKind>,
                step_id: Option<StepId>,
                run_step_key: Option<RunStepKey>,
                step_status: Option<Enum<StepRunStatus>>,
                target_count: Option<u64>,
                frame_id: Option<FrameId>,
                node_id: Option<FlowNodeId>,
                loop_instance_id: Option<LoopInstanceId>,
                retry_key: Option<String>,
            },
            AuthorizeFlowFrameReducerCommand {
                frame_id: FrameId,
                command: Enum<FlowFrameReducerCommandKind>,
                node_id: Option<FlowNodeId>,
                frame_node_key: Option<FrameNodeKey>,
                node_status: Option<Enum<NodeRunStatus>>,
                terminal_status: Option<Enum<FrameStatus>>,
            },
            AuthorizeLoopIterationReducerCommand {
                loop_instance_id: LoopInstanceId,
                command: Enum<LoopIterationReducerCommandKind>,
                body_frame_id: Option<FrameId>,
                body_frame_iteration: Option<u64>,
            },
            CancelFlow { run_id: RunId },
            FlowStatus,
            Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, bridge_session_id: SessionId, replacing: Option<SessionId> },
            AuthorizeSpawnProfile { agent_identity: AgentIdentity, profile_name: String, model: String, provider_params_digest: Option<String>, external_addressable: bool },
            EnsureMember { agent_identity: AgentIdentity },
            Reconcile { desired: Set<AgentIdentity>, retire_stale: bool },
            Retire { mob_id: MobId, agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, generation: Generation, releasing: Option<SessionId>, session_id: SessionId },
            RetireAbsent { agent_identity: AgentIdentity },
            RequestPendingSessionIngressDetachForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            Respawn { agent_runtime_id: AgentRuntimeId },
            RetireAll,
            // Track-B (R5): explicit identity-level wiring and session-binding
            // mutation inputs. These drive `wiring_edges` and
            // `member_session_bindings` directly at DSL authority,
            // independent of the Spawn/Retire lifecycle.
            //
            // The `edge` field on `WireMembers`/`UnwireMembers` carries a
            // pre-normalized `WiringEdge` (a <= b). Callers construct the
            // edge via `WiringEdge::new(a, b)` before submitting.
            WireMembers { edge: WiringEdge },
            UnwireMembers { edge: WiringEdge },
            WireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RegisterMemberPeer { agent_identity: AgentIdentity, peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberTrustWiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustUnwiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanup { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanupObserved { edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId },
            AuthorizeExternalPeerReciprocalTrust { key: ExternalPeerKey, agent_identity: AgentIdentity },
            UnwireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            SessionIngressDetachedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            SessionIngressDetachFailedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId, reason: String },
            SubmitWork { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
            ResolveSubmitWorkRejection { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, origin: Enum<WorkOrigin> },
            CancelWork { work_id: WorkId },
            CancelAllWork { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ResolveCancelAllWorkRejection { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            Stop,
            Resume,
            Complete,
            Reset,
            Destroy,
            RosterSnapshot,
            ListMembers,
            ListMembersIncludingRetiring,
            ListAllMembers,
            MemberStatus,
            SubscribeAgentEvents { agent_identity: AgentIdentity },
            SubscribeAllAgentEvents { session_bound_runtimes: Set<AgentRuntimeId> },
            SubscribeMobEvents { initial_cursor: u64, channel_capacity: u64, poll_interval_ms: u64, session_bound_runtimes: Set<AgentRuntimeId> },
            SubscribeStructuralEvents { after_cursor: u64, latest_cursor: u64, explicit_after_cursor: bool, batch_limit: u64, channel_capacity: u64 },
            AuthorizeMobEventRouterMemberSubscription { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            AuthorizeMobEventRouterMemberRemoval { agent_identity: AgentIdentity },
            PollEvents,
            PollEventsStrict { after_cursor: u64, latest_cursor: u64, limit: u64 },
            ReplayAllEvents,
            RecordOperatorActionProvenance { tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String> },
            GetMember,
            SetSpawnPolicy { enabled: bool },
            ResolveSpawnPolicy { agent_identity: AgentIdentity, revision: u64, profile_name: Option<String>, runtime_mode: Option<Enum<SpawnPolicyRuntimeMode>> },
            Shutdown,
            ForceCancel { agent_identity: AgentIdentity },
            KickoffMarkPending { member_id: String },
            KickoffMarkStarting { member_id: String },
            StartupMarkReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            KickoffResolveStarted { member_id: String },
            KickoffResolveCallbackPending { member_id: String },
            KickoffResolveFailed { member_id: String, error: String },
            KickoffCancelRequested { member_id: String },
            KickoffClear { member_id: String },
        }

        surface_only [
            FlowStatus,
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
            RetireMember { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId },
            AdmitDestroyMemberRetire { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveDestroyMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId> },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, session_id: SessionId },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool, session_id: SessionId },
            ResolveRespawnTopologyRestore { agent_identity: AgentIdentity, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            DestroyMob { session_id: SessionId },
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RecoverRosterMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, external_addressable: bool },
            RecoverMemberSessionBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, replacing: Option<SessionId> },
            RecoverRosterMemberReset { agent_identity: AgentIdentity, previous_agent_runtime_id: AgentRuntimeId, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecoverRosterMemberRetired { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId },
            RecoverMemberKickoff { member_id: String, phase: KickoffPhase, error: Option<String> },
            RecoverRosterWiring { edge: WiringEdge },
            RecoverRosterUnwire { edge: WiringEdge },
            RecoverExternalPeerWiring { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RecoverExternalPeerUnwire { key: ExternalPeerKey },
            RecoverMemberRestoreFailure { agent_identity: AgentIdentity, reason: String },
            AdmitDestroyCleanup,
            AdmitDestroyStorageFinalizing,
            MarkCompleted,
            StartRun,
            FinishRun,
            BeginCleanup,
            FinishCleanup,
            InitializeOrchestrator,
            BindCoordinator,
            UnbindCoordinator,
            StageSpawn { agent_identity: AgentIdentity, session_id: SessionId },
            CompleteSpawn { agent_identity: AgentIdentity },
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
            RequestRuntimeBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: SessionId },
            SpawnProfileAuthorized { agent_identity: AgentIdentity, profile_name: String, model: String, provider_params_digest: Option<String>, external_addressable: bool },
            RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
            SubmitWorkRejected { agent_runtime_id: AgentRuntimeId, origin: Enum<WorkOrigin>, reason: Enum<SubmitWorkRejectReasonKind>, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken> },
            CancelAllWorkRejected { agent_runtime_id: AgentRuntimeId, reason: Enum<CancelAllWorkRejectReasonKind>, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken> },
            RequestRuntimeRetire { session_id: SessionId },
            RequestRuntimeDestroy { session_id: SessionId },
            RequestSessionIngressDetachForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            AppendLifecycleJournal { kind: Enum<MobLifecycleJournalKind>, agent_identity: Option<AgentIdentity>, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, session_id: Option<SessionId> },
            AppendOperatorActionProvenance { tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String> },
            EmitMemberLifecycleNotice { kind: Enum<MemberLifecycleKind> },
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
            PersistKickoffUpdate { member_id: String, phase: KickoffPhase },
            PersistKickoffFailureUpdate { member_id: String, phase: KickoffPhase, error: String },
            EmitKickoffLifecycleNotice { member_id: String, intent: Enum<KickoffIntent> },
            SpawnPolicyResolutionRecorded { agent_identity: AgentIdentity, revision: u64, profile_name: Option<String>, runtime_mode: Option<Enum<SpawnPolicyRuntimeMode>> },
            RespawnTopologyRestoreResolved { agent_identity: AgentIdentity, result: Enum<RespawnTopologyRestoreResultKind>, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            // Track-B (R5): canonical topology-change signals consumed by
            // the `RecomputeMobPeerOverlay` composition driver.
            //
            // - `WiringGraphChanged` fires when `wiring_edges` mutates.
            // - `MemberSessionBindingChanged` fires on every binding
            //   mutation (set, rotated, or released), carrying the
            //   before/after session ids as `Option<SessionId>`. Absence
            //   of a value means "no binding on this side of the
            //   transition": `old=None, new=Some` → set, `Some, Some` →
            //   rotate, `Some, None` → release.
            //
            // Topology-change signals carry the post-transition
            // `topology_epoch` so the driver can linearize recomputes
            // against the newest topology snapshot. Trust handoffs carry the
            // topology epoch they authorize; member unwiring is a two-step
            // authorize-then-remove path, so its handoff carries the next
            // epoch that the graph-removal transition must produce.
            WiringGraphChanged { epoch: u64 },
            MemberSessionBindingChanged { epoch: u64, agent_identity: AgentIdentity, old_session_id: Option<SessionId>, new_session_id: Option<SessionId> },
            MemberTrustWiringRequested { edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, a_endpoint: MemberPeerEndpoint, b_endpoint: MemberPeerEndpoint, epoch: u64 },
            MemberTrustUnwiringRequested { edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, epoch: u64 },
            WiringTrustRepairRequested { edge: WiringEdge },
            ExternalPeerTrustWiringRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            ExternalPeerTrustUnwiringRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            ExternalPeerTrustRepairRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            MemberPeerRegistered { agent_identity: AgentIdentity, peer_id: PeerId },
            ExternalPeerReciprocalTrustRequested { key: ExternalPeerKey, edge: ExternalPeerEdge, peer_id: PeerId, peer_endpoint: MemberPeerEndpoint, epoch: u64 },
            // D-wiring-observability (#27): pair-valued notice emitted from
            // `WireMembers`/`UnwireMembers` alongside `WiringGraphChanged`.
            // Unlike `WiringGraphChanged` (opaque epoch bump), this carries
            // the `WiringEdge` so external observers (event store,
            // telemetry) can reconstruct which identity pair was wired or
            // unwired. Separate from `EmitMemberLifecycleNotice` because
            // wiring is pair-valued, not per-member.
            EmitWiringLifecycleNotice { kind: Enum<WiringLifecycleKind>, edge: WiringEdge },
            // Descriptor-bearing external peer trust notice. Carries the
            // endpoint fields (`peer_id`, `address`, `signing_key`) that
            // cannot be represented by a member `WiringEdge`.
            EmitExternalPeerWiringLifecycleNotice { kind: Enum<WiringLifecycleKind>, edge: ExternalPeerEdge },
            AuthorizeAgentEventSubscription { agent_identity: AgentIdentity, session_id: SessionId },
            RejectAgentEventSubscription { agent_identity: AgentIdentity, reason: Enum<EventSubscriptionRejectReasonKind> },
            AuthorizeAllAgentEventSubscription { session_bound_runtimes: Set<AgentRuntimeId> },
            RejectAllAgentEventSubscription { reason: Enum<EventSubscriptionRejectReasonKind> },
            AuthorizeMobEventRouter { initial_cursor: u64, channel_capacity: u64, poll_interval_ms: u64, session_bound_runtimes: Set<AgentRuntimeId> },
            AuthorizeMobEventRouterMemberSubscription { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId },
            AuthorizeMobEventRouterMemberRemoval { agent_identity: AgentIdentity },
            AuthorizeStructuralEventSubscription { after_cursor: u64, explicit_after_cursor: bool, batch_limit: u64, channel_capacity: u64 },
            RejectStructuralEventSubscription { after_cursor: u64, latest_cursor: u64 },
            AuthorizeStrictEventPoll { after_cursor: u64, limit: u64 },
            RejectStrictEventPoll { after_cursor: u64, latest_cursor: u64 },
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine],
        disposition SpawnProfileAuthorized => local,
        disposition RequestRuntimeIngress => routed [MeerkatMachine],
        disposition SubmitWorkRejected => local,
        disposition CancelAllWorkRejected => local,
        disposition RequestRuntimeRetire => routed [MeerkatMachine],
        disposition RequestRuntimeDestroy => routed [MeerkatMachine],
        disposition RequestSessionIngressDetachForMobDestroy => external handoff mob_destroying_session_ingress,
        disposition AppendLifecycleJournal => local,
        disposition AppendOperatorActionProvenance => local,
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
        disposition PersistKickoffUpdate => local,
        disposition PersistKickoffFailureUpdate => local,
        disposition EmitKickoffLifecycleNotice => external,
        disposition SpawnPolicyResolutionRecorded => local,
        disposition RespawnTopologyRestoreResolved => local,
        disposition WiringGraphChanged => external,
        disposition MemberSessionBindingChanged => external,
        disposition MemberTrustWiringRequested => external handoff mob_member_trust_wiring,
        disposition MemberTrustUnwiringRequested => external handoff mob_member_trust_unwiring,
        disposition WiringTrustRepairRequested => local,
        disposition ExternalPeerTrustWiringRequested => external handoff mob_external_peer_trust_wiring,
        disposition ExternalPeerTrustUnwiringRequested => external handoff mob_external_peer_trust_unwiring,
        disposition ExternalPeerTrustRepairRequested => external handoff mob_external_peer_trust_repair,
        disposition MemberPeerRegistered => local,
        disposition ExternalPeerReciprocalTrustRequested => external handoff mob_external_peer_reciprocal_trust,
        disposition EmitWiringLifecycleNotice => external,
        disposition EmitExternalPeerWiringLifecycleNotice => external,
        disposition AuthorizeAgentEventSubscription => local,
        disposition RejectAgentEventSubscription => local,
        disposition AuthorizeAllAgentEventSubscription => local,
        disposition RejectAllAgentEventSubscription => local,
        disposition AuthorizeMobEventRouter => local,
        disposition AuthorizeMobEventRouterMemberSubscription => local,
        disposition AuthorizeMobEventRouterMemberRemoval => local,
        disposition AuthorizeStructuralEventSubscription => local,
        disposition RejectStructuralEventSubscription => local,
        disposition AuthorizeStrictEventPoll => local,
        disposition RejectStrictEventPoll => local,

        // =====================================================================
        // Invariants
        // =====================================================================

        // W3-H / dogma #4: "no zombie realtime binding" — every identity that
        // has a bound session must also appear in `identity_to_runtime` (i.e.
        // must be an identity MobMachine has spawned). Ensures the binding map
        // cannot reference identities the machine has never admitted. Paired
        // with the Retire transition's `member_session_bindings.remove` and
        // Spawn's guard/state consistency: keys(bindings) ⊆ keys(identity_to_runtime).
        invariant bindings_require_known_identity {
            for_all(id in self.member_session_bindings.keys(), self.identity_to_runtime.contains_key(id))
        }

        invariant identity_runtime_material_matches_runtime_binding {
            for_all(id in self.identity_to_runtime.keys(), self.identity_runtime_generations.contains_key(id))
            && for_all(id in self.identity_to_runtime.keys(), self.identity_runtime_fence_tokens.contains_key(id))
            && for_all(id in self.identity_runtime_generations.keys(), self.identity_to_runtime.contains_key(id))
            && for_all(id in self.identity_runtime_fence_tokens.keys(), self.identity_to_runtime.contains_key(id))
        }

        invariant external_peer_edges_are_keyed_coherently {
            for_all(key in self.external_peer_edges_by_key.keys(),
                mob_machine_external_peer_key_matches_edge(key, self.external_peer_edges_by_key.get_cloned(key).get("value"))
                && self.external_peer_edges.contains(self.external_peer_edges_by_key.get_cloned(key).get("value")))
            && for_all(edge in self.external_peer_edges,
                mob_machine_external_peer_edge_has_matching_key(self.external_peer_edges_by_key, edge))
        }

        // =====================================================================
        // Direct transitions
        // =====================================================================

        // W3-H: Spawn splits into two guarded variants — Fresh (no prior
        // realtime binding for the identity) and Replacing (identity already
        // has a `BoundToSession`, i.e. this Spawn is the second half of a
        // shell-orchestrated respawn). Guards check BOTH the input's
        // `replacing` witness AND the state's `member_session_bindings` key
        // presence so a caller cannot invoke the wrong branch — the DSL
        // enforces caller/state consistency, and a mismatched caller fails
        // loudly with "no transition matched" rather than silently picking a
        // wrong branch.
        transition SpawnRunningFresh {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            update {
                // Spawn is the "member joined live_runtime_ids" fact. The
                // pending_spawn_count lifecycle is owned by StageSpawn (+1)
                // and CompleteSpawn (-1) signals; Spawn itself leaves the
                // counter untouched. Concurrent spawn batches rely on that
                // separation — zeroing or double-decrementing here breaks
                // alignment between DSL pending_spawn_count and the actor's
                // pending_spawns map
                // (test_concurrent_spawns_parallelize_provisioning).
                // active_run_count is unrelated to spawn and stays untouched
                // (previously zeroed here by mistake).
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: bridge_session_id }
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberSpawned,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: Some(bridge_session_id)
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: Some(bridge_session_id) }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition SpawnRunningReplacing {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            guard "replacing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(replacing.get("value")) }
            update {
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: bridge_session_id }
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberSpawned,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: Some(bridge_session_id)
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(replacing.get("value")), new_session_id: Some(bridge_session_id) }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition AuthorizeSpawnProfileRunning {
            on input AuthorizeSpawnProfile { agent_identity, profile_name, model, provider_params_digest, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {}
            to Running
            emit SpawnProfileAuthorized {
                agent_identity: agent_identity,
                profile_name: profile_name,
                model: model,
                provider_params_digest: provider_params_digest,
                external_addressable: external_addressable
            }
        }

        transition EnsureMemberRunningExisting {
            on input EnsureMember { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            update {}
            to Running
        }

        transition EnsureMemberRunningMissing {
            on input EnsureMember { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Running
        }

        transition RecoverRosterMemberRunning {
            on signal RecoverRosterMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_not_recovered" { self.identity_to_runtime.contains_key(agent_identity) == false }
            guard "runtime_not_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            update {
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterMemberAddressabilityRunning {
            on signal RecoverRosterMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            update {
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
        }

        transition RecoverMemberSessionBindingFreshRunning {
            on signal RecoverMemberSessionBinding { agent_identity, agent_runtime_id, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            update {
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: Some(bridge_session_id) }
        }

        transition RecoverMemberSessionBindingReplacingRunning {
            on signal RecoverMemberSessionBinding { agent_identity, agent_runtime_id, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            guard "replacing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(replacing.get("value")) }
            guard "replacement_changes_binding" { bridge_session_id != replacing.get("value") }
            update {
                self.member_session_bindings.insert(agent_identity, bridge_session_id);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(replacing.get("value")), new_session_id: Some(bridge_session_id) }
        }

        transition RecoverMemberSessionBindingAlreadyCurrentRunning {
            on signal RecoverMemberSessionBinding { agent_identity, agent_runtime_id, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "binding_already_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(bridge_session_id) }
            guard "replacing_absent_or_current" { replacing == None || replacing == Some(bridge_session_id) }
            update {}
            to Running
        }

        transition RecoverRosterMemberResetRunning {
            on signal RecoverRosterMemberReset { agent_identity, previous_agent_runtime_id, agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Running }
            guard "previous_runtime_recovered" { self.live_runtime_ids.contains(previous_agent_runtime_id) == true }
            guard "identity_recovered" { self.identity_to_runtime.get_cloned(agent_identity) == Some(previous_agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(previous_agent_runtime_id);
                self.runtime_fence_tokens.remove(previous_agent_runtime_id);
                self.member_startup_binding_requested.remove(previous_agent_runtime_id);
                self.member_startup_runtime_ready.remove(previous_agent_runtime_id);
                self.member_startup_ready.remove(previous_agent_runtime_id);
                self.member_state_markers.remove(previous_agent_runtime_id);
                self.live_runtime_ids.insert(agent_runtime_id);
                if self.externally_addressable_runtime_ids.contains(previous_agent_runtime_id) {
                    self.externally_addressable_runtime_ids.remove(previous_agent_runtime_id);
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterMemberRetiredRunning {
            on signal RecoverRosterMemberRetired { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.identity_to_runtime.remove(agent_identity);
                self.identity_runtime_generations.remove(agent_identity);
                self.identity_runtime_fence_tokens.remove(agent_identity);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterMemberRetiredAlreadyAbsent {
            on signal RecoverRosterMemberRetired { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_not_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            update {
                self.identity_to_runtime.remove(agent_identity);
                self.identity_runtime_generations.remove(agent_identity);
                self.identity_runtime_fence_tokens.remove(agent_identity);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
        }

        transition RecoverMemberKickoffPending {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_pending_phase" { phase == KickoffPhase::Pending }
            guard "recover_pending_without_error" { error == None }
            update {
                self.member_kickoff_pending.insert(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition RecoverMemberKickoffStarting {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_starting_phase" { phase == KickoffPhase::Starting }
            guard "recover_starting_without_error" { error == None }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.insert(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition RecoverMemberKickoffCallbackPending {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_callback_pending_phase" { phase == KickoffPhase::CallbackPending }
            guard "recover_callback_pending_without_error" { error == None }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.insert(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition RecoverMemberKickoffStarted {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_started_phase" { phase == KickoffPhase::Started }
            guard "recover_started_without_error" { error == None }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.insert(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition RecoverMemberKickoffFailed {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_failed_phase" { phase == KickoffPhase::Failed }
            guard "recover_failed_has_error" { error != None }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.insert(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.insert(member_id, error.get("value"));
            }
            to Running
        }

        transition RecoverMemberKickoffCancelled {
            on signal RecoverMemberKickoff { member_id, phase, error }
            guard { self.lifecycle_phase == Phase::Running }
            guard "recover_cancelled_phase" { phase == KickoffPhase::Cancelled }
            guard "recover_cancelled_without_error" { error == None }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.insert(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition ReconcileRunning {
            on input Reconcile { desired, retire_stale }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
        }

        transition ReconcileStopped {
            on input Reconcile { desired, retire_stale }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
        }

        transition ReconcileCompleted {
            on input Reconcile { desired, retire_stale }
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
        }

        transition ObserveRuntimeReady {
            on signal ObserveRuntimeReady { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.insert(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
            }
            to Running
        }

        transition StartupMarkReady {
            per_phase [Running, Stopped, Completed]
            on input StartupMarkReady { agent_runtime_id, fence_token }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.insert(agent_runtime_id);
            }
            to Running
        }

        transition KickoffMarkPending {
            per_phase [Running, Stopped, Completed]
            on input KickoffMarkPending { member_id }
            guard "kickoff_not_started" {
                !self.member_kickoff_pending.contains(member_id)
                && !self.member_kickoff_starting.contains(member_id)
                && !self.member_kickoff_callback_pending.contains(member_id)
                && !self.member_kickoff_started.contains(member_id)
                && !self.member_kickoff_failed.contains(member_id)
                && !self.member_kickoff_cancelled.contains(member_id)
            }
            update {
                self.member_kickoff_pending.insert(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Pending }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Pending }
        }

        transition KickoffMarkStarting {
            per_phase [Running, Stopped, Completed]
            on input KickoffMarkStarting { member_id }
            guard "kickoff_pending" { self.member_kickoff_pending.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.insert(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Starting }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Starting }
        }

        transition KickoffResolveStarted {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveStarted { member_id }
            guard "kickoff_starting" { self.member_kickoff_starting.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.insert(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Started }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Started }
        }

        transition KickoffResolveCallbackPending {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveCallbackPending { member_id }
            guard "kickoff_starting" { self.member_kickoff_starting.contains(member_id) }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.insert(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::CallbackPending }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::CallbackPending }
        }

        transition KickoffResolveFailedFromStarting {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveFailed { member_id, error }
            guard "kickoff_active_failed" {
                (self.member_kickoff_pending.contains(member_id)
                    || self.member_kickoff_starting.contains(member_id)
                    || self.member_kickoff_callback_pending.contains(member_id))
            }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.insert(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.insert(member_id, error);
            }
            to Running
            emit PersistKickoffFailureUpdate { member_id: member_id, phase: KickoffPhase::Failed, error: error }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Failed }
        }

        transition KickoffCancelRequested {
            per_phase [Running, Stopped, Completed]
            on input KickoffCancelRequested { member_id }
            guard "kickoff_cancellable" {
                (self.member_kickoff_pending.contains(member_id)
                    || self.member_kickoff_starting.contains(member_id)
                    || self.member_kickoff_callback_pending.contains(member_id))
            }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.insert(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Cancelled }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Cancelled }
        }

        transition KickoffClear {
            per_phase [Running, Stopped, Completed]
            on input KickoffClear { member_id }
            update {
                self.member_kickoff_pending.remove(member_id);
                self.member_kickoff_starting.remove(member_id);
                self.member_kickoff_callback_pending.remove(member_id);
                self.member_kickoff_started.remove(member_id);
                self.member_kickoff_failed.remove(member_id);
                self.member_kickoff_cancelled.remove(member_id);
                self.member_kickoff_error.remove(member_id);
            }
            to Running
        }

        transition SubmitWorkRunningExternal {
            on input SubmitWork { agent_identity, agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "external_origin" { origin == WorkOrigin::External }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition SubmitWorkRunningInternal {
            on input SubmitWork { agent_identity, agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "internal_origin" { origin == WorkOrigin::Internal }
            update {}
            to Running
            emit RequestRuntimeIngress { agent_runtime_id: agent_runtime_id, fence_token: fence_token, work_id: work_id, origin: origin }
        }

        transition ResolveSubmitWorkRejectionStopped {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionCompleted {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionDestroyed {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionMemberNotFound {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { !self.identity_to_runtime.contains_key(agent_identity) }
            update {}
            to Running
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MemberNotFound,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionCurrentRuntimeNotLive {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) }
            guard "current_runtime_not_live" { !self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            update {}
            to Running
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MemberNotFound,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionStaleFenceToken {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) }
            guard "current_runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "runtime_or_fence_stale" {
                self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id)
                || self.runtime_fence_tokens.get_copied(self.identity_to_runtime.get_cloned(agent_identity).get("value")) != Some(fence_token)
            }
            update {}
            to Running
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::StaleFenceToken,
                expected_fence_token: self.runtime_fence_tokens.get_copied(self.identity_to_runtime.get_cloned(agent_identity).get("value")),
                actual_fence_token: Some(fence_token)
            }
        }

        transition ResolveSubmitWorkRejectionRetiringAsMemberNotFound {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {}
            to Running
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::MemberNotFound,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveSubmitWorkRejectionNotExternallyAddressable {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "external_origin" { origin == WorkOrigin::External }
            guard "runtime_not_externally_addressable" { !self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit SubmitWorkRejected {
                agent_runtime_id: agent_runtime_id,
                origin: origin,
                reason: SubmitWorkRejectReasonKind::NotExternallyAddressable,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition RetireMember {
            on signal RetireMember { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition AdmitDestroyMemberRetireLiveRunning {
            on signal AdmitDestroyMemberRetire { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition AdmitDestroyMemberRetireLiveStopped {
            on signal AdmitDestroyMemberRetire { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringRunning {
            on signal AdmitDestroyMemberRetire { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {}
            to Running
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringStopped {
            on signal AdmitDestroyMemberRetire { agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {}
            to Stopped
        }

        transition ObserveRuntimeRetired {
            on signal ObserveRuntimeRetired { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedLive {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedLiveStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.active_run_count = 0;
            }
            to Stopped
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedRetired {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
            }
            to Running
        }

        transition ObserveMemberRetirementArchivedRetiredStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
            }
            to Stopped
        }

        transition ObserveMemberRetirementArchivedStaleRuntime {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_remapped" { self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
            }
            to Running
        }

        transition ObserveMemberRetirementArchivedStaleRuntimeStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_remapped" { self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
            }
            to Stopped
        }

        transition ObserveMemberRetirementArchivedAlreadyCleared {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Running
        }

        transition ObserveMemberRetirementArchivedAlreadyClearedStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Stopped
        }

        transition ObserveDestroyMemberRetirementArchivedLiveRunning {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.active_run_count = 0;
                self.topology_epoch += 1;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: session_id, new_session_id: None }
        }

        transition ObserveDestroyMemberRetirementArchivedLiveStopped {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.active_run_count = 0;
                self.topology_epoch += 1;
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: session_id, new_session_id: None }
        }

        transition ObserveDestroyMemberRetirementArchivedRetiredRunning {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: session_id, new_session_id: None }
        }

        transition ObserveDestroyMemberRetirementArchivedRetiredStopped {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: session_id, new_session_id: None }
        }

        transition ResetMember {
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Reset }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, external_addressable, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.live_runtime_ids.insert(agent_runtime_id);
                if external_addressable {
                    self.externally_addressable_runtime_ids.insert(agent_runtime_id);
                } else {
                    self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                }
                self.runtime_fence_tokens.insert(agent_runtime_id, fence_token);
                self.identity_to_runtime.insert(agent_identity, agent_runtime_id);
                self.identity_runtime_generations.insert(agent_identity, generation);
                self.identity_runtime_fence_tokens.insert(agent_identity, fence_token);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: generation, session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Respawned }
        }

        transition ResolveRespawnTopologyRestoreCompleted {
            on signal ResolveRespawnTopologyRestore { agent_identity, failed_peer_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "no_failed_peers" { failed_peer_ids == EmptySeq }
            update {}
            to Running
            emit RespawnTopologyRestoreResolved {
                agent_identity: agent_identity,
                result: RespawnTopologyRestoreResultKind::Completed,
                failed_peer_ids: failed_peer_ids
            }
        }

        transition ResolveRespawnTopologyRestoreFailed {
            on signal ResolveRespawnTopologyRestore { agent_identity, failed_peer_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "failed_peers_present" { failed_peer_ids != EmptySeq }
            update {}
            to Running
            emit RespawnTopologyRestoreResolved {
                agent_identity: agent_identity,
                result: RespawnTopologyRestoreResultKind::TopologyRestoreFailed,
                failed_peer_ids: failed_peer_ids
            }
        }

        transition RecoverMemberRestoreFailureRunning {
            on signal RecoverMemberRestoreFailure { agent_identity, reason }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.member_restore_failures.insert(agent_identity, reason);
            }
            to Running
        }

        transition RecoverMemberRestoreFailureStopped {
            on signal RecoverMemberRestoreFailure { agent_identity, reason }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.member_restore_failures.insert(agent_identity, reason);
            }
            to Stopped
        }

        transition RecoverMemberRestoreFailureCompleted {
            on signal RecoverMemberRestoreFailure { agent_identity, reason }
            guard { self.lifecycle_phase == Phase::Completed }
            update {
                self.member_restore_failures.insert(agent_identity, reason);
            }
            to Completed
        }

        transition AdmitDestroyCleanup {
            on signal AdmitDestroyCleanup
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            update {
                self.destroy_admitted = true;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::Destroying,
                agent_identity: None,
                agent_runtime_id: None,
                fence_token: None,
                generation: None,
                session_id: None
            }
        }

        transition AdmitDestroyStorageFinalizing {
            on signal AdmitDestroyStorageFinalizing
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "destroy_admitted" { self.destroy_admitted == true }
            update {}
            to Destroyed
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::DestroyStorageFinalizing,
                agent_identity: None,
                agent_runtime_id: None,
                fence_token: None,
                generation: None,
                session_id: None
            }
        }

        transition MarkCompleted {
            on signal MarkCompleted
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Completed
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Completed }
        }

        transition DestroyMob {
            on signal DestroyMob { session_id }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
                || self.lifecycle_phase == Phase::Completed
            }
            guard "session_ingress_detaches_closed" { self.pending_session_ingress_detach_runtime_ids == EmptySet }
            update {
                self.destroy_admitted = true;
                self.live_runtime_ids = EmptySet;
                self.externally_addressable_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.wiring_edges = EmptySet;
                self.external_peer_edges = EmptySet;
                self.external_peer_edges_by_key = EmptyMap;
                self.identity_to_runtime = EmptyMap;
                self.identity_runtime_generations = EmptyMap;
                self.identity_runtime_fence_tokens = EmptyMap;
                self.member_session_bindings = EmptyMap;
                self.member_peer_ids = EmptyMap;
                self.member_peer_endpoints = EmptyMap;
                self.member_startup_binding_requested = EmptySet;
                self.member_startup_runtime_ready = EmptySet;
                self.member_startup_ready = EmptySet;
                self.member_state_markers = EmptyMap;
                self.member_restore_failures = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit RequestRuntimeDestroy { session_id: session_id }
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
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_state_markers = EmptyMap;
                self.member_restore_failures = EmptyMap;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.coordinator_bound = false;
            }
            to Destroyed
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Destroyed }
        }

        // =====================================================================
        // Absorbed transitions: per-phase self-loops
        // =====================================================================

        transition RecordOperatorActionProvenanceRunning {
            on input RecordOperatorActionProvenance { tool_name, principal_token, caller_provenance, audit_invocation_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit AppendOperatorActionProvenance {
                tool_name: tool_name,
                principal_token: principal_token,
                caller_provenance: caller_provenance,
                audit_invocation_id: audit_invocation_id
            }
        }
        transition RecordOperatorActionProvenanceStopped {
            on input RecordOperatorActionProvenance { tool_name, principal_token, caller_provenance, audit_invocation_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit AppendOperatorActionProvenance {
                tool_name: tool_name,
                principal_token: principal_token,
                caller_provenance: caller_provenance,
                audit_invocation_id: audit_invocation_id
            }
        }
        transition RecordOperatorActionProvenanceCompleted {
            on input RecordOperatorActionProvenance { tool_name, principal_token, caller_provenance, audit_invocation_id }
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
            emit AppendOperatorActionProvenance {
                tool_name: tool_name,
                principal_token: principal_token,
                caller_provenance: caller_provenance,
                audit_invocation_id: audit_invocation_id
            }
        }
        transition RecordOperatorActionProvenanceDestroyed {
            on input RecordOperatorActionProvenance { tool_name, principal_token, caller_provenance, audit_invocation_id }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
            emit AppendOperatorActionProvenance {
                tool_name: tool_name,
                principal_token: principal_token,
                caller_provenance: caller_provenance,
                audit_invocation_id: audit_invocation_id
            }
        }

        transition SetSpawnPolicyRunning {
            on input SetSpawnPolicy { enabled }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.spawn_policy_enabled = enabled;
                self.spawn_policy_revision += 1;
                self.spawn_policy_resolution_revision = EmptyMap;
                self.spawn_policy_resolution_profiles = EmptyMap;
                self.spawn_policy_resolution_runtime_modes = EmptyMap;
                self.spawn_policy_resolution_absent = EmptySet;
            }
            to Running
        }
        transition SetSpawnPolicyStopped {
            on input SetSpawnPolicy { enabled }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.spawn_policy_enabled = enabled;
                self.spawn_policy_revision += 1;
                self.spawn_policy_resolution_revision = EmptyMap;
                self.spawn_policy_resolution_profiles = EmptyMap;
                self.spawn_policy_resolution_runtime_modes = EmptyMap;
                self.spawn_policy_resolution_absent = EmptySet;
            }
            to Stopped
        }
        transition SetSpawnPolicyCompleted {
            on input SetSpawnPolicy { enabled }
            guard { self.lifecycle_phase == Phase::Completed }
            update {
                self.spawn_policy_enabled = enabled;
                self.spawn_policy_revision += 1;
                self.spawn_policy_resolution_revision = EmptyMap;
                self.spawn_policy_resolution_profiles = EmptyMap;
                self.spawn_policy_resolution_runtime_modes = EmptyMap;
                self.spawn_policy_resolution_absent = EmptySet;
            }
            to Completed
        }
        transition SetSpawnPolicyDestroyed {
            on input SetSpawnPolicy { enabled }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {
                self.spawn_policy_enabled = enabled;
                self.spawn_policy_revision += 1;
                self.spawn_policy_resolution_revision = EmptyMap;
                self.spawn_policy_resolution_profiles = EmptyMap;
                self.spawn_policy_resolution_runtime_modes = EmptyMap;
                self.spawn_policy_resolution_absent = EmptySet;
            }
            to Destroyed
        }

        transition ResolveSpawnPolicyAdmitted {
            on input ResolveSpawnPolicy { agent_identity, revision, profile_name, runtime_mode }
            guard { self.lifecycle_phase == Phase::Running }
            guard "policy_enabled" { self.spawn_policy_enabled }
            guard "revision_matches" { revision == self.spawn_policy_revision }
            guard "identity_absent" { !self.identity_to_runtime.contains_key(agent_identity) }
            guard "profile_present" { profile_name != None }
            update {
                self.spawn_policy_resolution_revision.insert(agent_identity, revision);
                self.spawn_policy_resolution_profiles.insert(agent_identity, profile_name.get("value"));
                self.spawn_policy_resolution_runtime_modes.insert(agent_identity, runtime_mode);
                self.spawn_policy_resolution_absent.remove(agent_identity);
            }
            to Running
            emit SpawnPolicyResolutionRecorded {
                agent_identity: agent_identity,
                revision: revision,
                profile_name: profile_name,
                runtime_mode: runtime_mode
            }
        }

        transition ResolveSpawnPolicyNoMatch {
            on input ResolveSpawnPolicy { agent_identity, revision, profile_name, runtime_mode }
            guard { self.lifecycle_phase == Phase::Running }
            guard "policy_enabled" { self.spawn_policy_enabled }
            guard "revision_matches" { revision == self.spawn_policy_revision }
            guard "identity_absent" { !self.identity_to_runtime.contains_key(agent_identity) }
            guard "profile_absent" { profile_name == None }
            guard "runtime_mode_absent" { runtime_mode == None }
            update {
                self.spawn_policy_resolution_revision.insert(agent_identity, revision);
                self.spawn_policy_resolution_profiles.remove(agent_identity);
                self.spawn_policy_resolution_runtime_modes.remove(agent_identity);
                self.spawn_policy_resolution_absent.insert(agent_identity);
            }
            to Running
            emit SpawnPolicyResolutionRecorded {
                agent_identity: agent_identity,
                revision: revision,
                profile_name: None,
                runtime_mode: None
            }
        }

        // =====================================================================
        // Phase-changing transitions
        // =====================================================================

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

        transition ResumeStopped {
            on input Resume
            guard { self.lifecycle_phase == Phase::Stopped }
            update {
                self.coordinator_bound = true;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition CompleteRunning {
            on input Complete
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.active_run_count = 0;
            }
            to Completed
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::Completed,
                agent_identity: None,
                agent_runtime_id: None,
                fence_token: None,
                generation: None,
                session_id: None
            }
            emit EmitRunLifecycleNotice
        }

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
                self.pending_spawn_sessions = EmptyMap;
                self.coordinator_bound = true;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::Reset,
                agent_identity: None,
                agent_runtime_id: None,
                fence_token: None,
                generation: None,
                session_id: None
            }
            emit EmitRunLifecycleNotice
        }

        // =====================================================================
        // Running self-loops (inputs)
        // =====================================================================

        // =====================================================================
        // Track-B (R5): identity-level wiring mutations.
        //
        // `WireMembers`/`UnwireMembers` mutate `wiring_edges` at DSL
        // authority and bump `topology_epoch`. The `WiringGraphChanged`
        // effect lets the `RecomputeMobPeerOverlay` composition driver
        // linearize peer-overlay recomputation against graph changes.
        // =====================================================================

        transition WireMembersRunning {
            on input WireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_not_already_wired" { self.wiring_edges.contains(edge) == false }
            update {
                self.wiring_edges.insert(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Wired, edge: edge }
        }

        transition WireMembersAlreadyWired {
            on input WireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_already_wired" { self.wiring_edges.contains(edge) == true }
            update {}
            to Running
            emit WiringTrustRepairRequested { edge: edge }
        }

        transition RecoverRosterWiringRunning {
            on signal RecoverRosterWiring { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_not_already_recovered" { self.wiring_edges.contains(edge) == false }
            update {
                self.wiring_edges.insert(edge);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterWiringAlreadyRecovered {
            on signal RecoverRosterWiring { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_already_recovered" { self.wiring_edges.contains(edge) == true }
            update {}
            to Running
        }

        transition RecoverRosterUnwireRunning {
            on signal RecoverRosterUnwire { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_recovered" { self.wiring_edges.contains(edge) == true }
            update {
                self.wiring_edges.remove(edge);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterUnwireAlreadyAbsent {
            on signal RecoverRosterUnwire { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_not_recovered" { self.wiring_edges.contains(edge) == false }
            update {}
            to Running
        }

        transition UnwireMembersRunning {
            on input UnwireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            update {
                self.wiring_edges.remove(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Unwired, edge: edge }
        }

        transition UnwireMembersAlreadyAbsent {
            on input UnwireMembers { edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_already_absent" { self.wiring_edges.contains(edge) == false }
            update {}
            to Running
        }

        transition WireExternalPeerRunning {
            on input WireExternalPeer { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_not_already_wired" { self.external_peer_edges_by_key.contains_key(key) == false }
            guard "external_peer_edge_not_already_wired" { self.external_peer_edges.contains(edge) == false }
            guard "local_member_peer_registered" { self.member_peer_ids.contains_key(mob_machine_external_peer_edge_local(edge)) == true }
            update {
                self.external_peer_edges.insert(edge);
                self.external_peer_edges_by_key.insert(key, edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit ExternalPeerTrustWiringRequested {
                edge: edge,
                local_peer_id: self.member_peer_ids.get_cloned(mob_machine_external_peer_edge_local(edge)).get("value"),
                peer_id: mob_machine_external_peer_edge_peer_id(edge),
                epoch: self.topology_epoch
            }
            emit EmitExternalPeerWiringLifecycleNotice { kind: WiringLifecycleKind::Wired, edge: edge }
        }

        transition WireExternalPeerAlreadyWired {
            on input WireExternalPeer { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_already_wired" { self.external_peer_edges_by_key.get_cloned(key) == Some(edge) }
            guard "external_peer_edge_already_wired" { self.external_peer_edges.contains(edge) == true }
            guard "local_member_peer_registered" { self.member_peer_ids.contains_key(mob_machine_external_peer_edge_local(edge)) == true }
            update {}
            to Running
            emit ExternalPeerTrustRepairRequested {
                edge: edge,
                local_peer_id: self.member_peer_ids.get_cloned(mob_machine_external_peer_edge_local(edge)).get("value"),
                peer_id: mob_machine_external_peer_edge_peer_id(edge),
                epoch: self.topology_epoch
            }
        }

        transition RegisterMemberPeerRunning {
            on input RegisterMemberPeer { agent_identity, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            update {
                self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(peer_endpoint));
                self.member_peer_endpoints.insert(agent_identity, peer_endpoint);
            }
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition AuthorizeMemberTrustWiringRunning {
            on input AuthorizeMemberTrustWiring { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            guard "a_member_endpoint_registered" { self.member_peer_endpoints.contains_key(a_identity) == true }
            guard "b_member_endpoint_registered" { self.member_peer_endpoints.contains_key(b_identity) == true }
            update {}
            to Running
            emit MemberTrustWiringRequested {
                edge: edge,
                a_peer_id: self.member_peer_ids.get_cloned(a_identity).get("value"),
                b_peer_id: self.member_peer_ids.get_cloned(b_identity).get("value"),
                a_endpoint: self.member_peer_endpoints.get_cloned(a_identity).get("value"),
                b_endpoint: self.member_peer_endpoints.get_cloned(b_identity).get("value"),
                epoch: self.topology_epoch
            }
        }

        transition AuthorizeMemberTrustUnwiringRunning {
            on input AuthorizeMemberTrustUnwiring { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            update {}
            to Running
            emit MemberTrustUnwiringRequested {
                edge: edge,
                a_peer_id: self.member_peer_ids.get_cloned(a_identity).get("value"),
                b_peer_id: self.member_peer_ids.get_cloned(b_identity).get("value"),
                epoch: self.topology_epoch + 1
            }
        }

        transition AuthorizeMemberTrustCleanupRunning {
            on input AuthorizeMemberTrustCleanup { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            update {}
            to Running
            emit MemberTrustUnwiringRequested {
                edge: edge,
                a_peer_id: self.member_peer_ids.get_cloned(a_identity).get("value"),
                b_peer_id: self.member_peer_ids.get_cloned(b_identity).get("value"),
                epoch: self.topology_epoch
            }
        }

        transition AuthorizeMemberTrustCleanupObservedRunning {
            on input AuthorizeMemberTrustCleanupObserved { edge, a_identity, a_peer_id, b_identity, b_peer_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "cleanup_has_restore_failure" { self.member_restore_failures.contains_key(a_identity) == true || self.member_restore_failures.contains_key(b_identity) == true }
            update {}
            to Running
            emit MemberTrustUnwiringRequested {
                edge: edge,
                a_peer_id: a_peer_id,
                b_peer_id: b_peer_id,
                epoch: self.topology_epoch
            }
        }

        transition AuthorizeExternalPeerReciprocalTrustRunning {
            on input AuthorizeExternalPeerReciprocalTrust { key, agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_already_wired" { self.external_peer_edges_by_key.contains_key(key) == true }
            guard "external_peer_key_matches_member" { mob_machine_external_peer_key_matches_local(key, agent_identity) }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            update {}
            to Running
            emit ExternalPeerReciprocalTrustRequested {
                key: key,
                edge: self.external_peer_edges_by_key.get_cloned(key).get("value"),
                peer_id: self.member_peer_ids.get_cloned(agent_identity).get("value"),
                peer_endpoint: self.member_peer_endpoints.get_cloned(agent_identity).get("value"),
                epoch: self.topology_epoch
            }
        }

        transition RecoverExternalPeerWiringRunning {
            on signal RecoverExternalPeerWiring { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_not_already_recovered" { self.external_peer_edges_by_key.contains_key(key) == false }
            guard "external_peer_edge_not_already_recovered" { self.external_peer_edges.contains(edge) == false }
            update {
                self.external_peer_edges.insert(edge);
                self.external_peer_edges_by_key.insert(key, edge);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverExternalPeerWiringAlreadyRecovered {
            on signal RecoverExternalPeerWiring { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_already_recovered" { self.external_peer_edges_by_key.get_cloned(key) == Some(edge) }
            guard "external_peer_edge_already_recovered" { self.external_peer_edges.contains(edge) == true }
            update {}
            to Running
        }

        transition RecoverExternalPeerUnwireRunning {
            on signal RecoverExternalPeerUnwire { key }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_recovered" { self.external_peer_edges_by_key.contains_key(key) == true }
            update {
                self.external_peer_edges.remove(self.external_peer_edges_by_key.get_cloned(key).get("value"));
                self.external_peer_edges_by_key.remove(key);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverExternalPeerUnwireAlreadyAbsent {
            on signal RecoverExternalPeerUnwire { key }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_not_recovered" { self.external_peer_edges_by_key.contains_key(key) == false }
            update {}
            to Running
        }

        transition UnwireExternalPeerRunning {
            on input UnwireExternalPeer { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_currently_wired" { self.external_peer_edges_by_key.get_cloned(key) == Some(edge) }
            guard "external_peer_edge_currently_wired" { self.external_peer_edges.contains(edge) == true }
            guard "local_member_peer_registered" { self.member_peer_ids.contains_key(mob_machine_external_peer_edge_local(edge)) == true }
            update {
                self.external_peer_edges.remove(edge);
                self.external_peer_edges_by_key.remove(key);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit ExternalPeerTrustUnwiringRequested {
                edge: edge,
                local_peer_id: self.member_peer_ids.get_cloned(mob_machine_external_peer_edge_local(edge)).get("value"),
                peer_id: mob_machine_external_peer_edge_peer_id(edge),
                epoch: self.topology_epoch
            }
            emit EmitExternalPeerWiringLifecycleNotice { kind: WiringLifecycleKind::Unwired, edge: edge }
        }

        transition UnwireExternalPeerAlreadyAbsent {
            on input UnwireExternalPeer { key, edge }
            guard { self.lifecycle_phase == Phase::Running }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_already_absent" { self.external_peer_edges_by_key.contains_key(key) == false }
            guard "external_peer_edge_already_absent" { self.external_peer_edges.contains(edge) == false }
            update {}
            to Running
        }

        transition ForceCancelRunning {
            on input ForceCancel { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_known" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(self.identity_to_runtime.get_cloned(agent_identity).get("value")) != Some(MobMemberState::Retiring) }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        // =====================================================================
        // Subscribe commands
        // =====================================================================

        transition SubscribeAgentEventsRunning {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Running
            emit AuthorizeAgentEventSubscription {
                agent_identity: agent_identity,
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value")
            }
        }
        transition SubscribeAgentEventsStopped {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Stopped
            emit AuthorizeAgentEventSubscription {
                agent_identity: agent_identity,
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value")
            }
        }
        transition SubscribeAgentEventsCompleted {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Completed
            emit AuthorizeAgentEventSubscription {
                agent_identity: agent_identity,
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value")
            }
        }
        transition SubscribeAgentEventsDestroyed {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Destroyed
            emit AuthorizeAgentEventSubscription {
                agent_identity: agent_identity,
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value")
            }
        }
        transition SubscribeAgentEventsMissingMemberRunning {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Running
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsMissingMemberStopped {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Stopped
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsMissingMemberCompleted {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Completed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsMissingMemberDestroyed {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Destroyed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsMissingSessionRunning {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Running
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionStopped {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Stopped
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionCompleted {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Completed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionDestroyed {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Destroyed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsRuntimeNotLiveRunning {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Running
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsRuntimeNotLiveStopped {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Stopped
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsRuntimeNotLiveCompleted {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Completed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }
        transition SubscribeAgentEventsRuntimeNotLiveDestroyed {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Destroyed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::MemberNotFound }
        }

        transition SubscribeAllAgentEventsRunning {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "session_bound_or_no_live_members" { session_bound_runtimes != EmptySet || self.live_runtime_ids == EmptySet }
            update {}
            to Running
            emit AuthorizeAllAgentEventSubscription { session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeAllAgentEventsStopped {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "session_bound_or_no_live_members" { session_bound_runtimes != EmptySet || self.live_runtime_ids == EmptySet }
            update {}
            to Stopped
            emit AuthorizeAllAgentEventSubscription { session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeAllAgentEventsCompleted {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "session_bound_or_no_live_members" { session_bound_runtimes != EmptySet || self.live_runtime_ids == EmptySet }
            update {}
            to Completed
            emit AuthorizeAllAgentEventSubscription { session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeAllAgentEventsDestroyed {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "session_bound_or_no_live_members" { session_bound_runtimes != EmptySet || self.live_runtime_ids == EmptySet }
            update {}
            to Destroyed
            emit AuthorizeAllAgentEventSubscription { session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeAllAgentEventsNoSessionBindingsRunning {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "no_session_bound_runtime" { session_bound_runtimes == EmptySet }
            guard "live_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Running
            emit RejectAllAgentEventSubscription { reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAllAgentEventsNoSessionBindingsStopped {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "no_session_bound_runtime" { session_bound_runtimes == EmptySet }
            guard "live_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Stopped
            emit RejectAllAgentEventSubscription { reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAllAgentEventsNoSessionBindingsCompleted {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "no_session_bound_runtime" { session_bound_runtimes == EmptySet }
            guard "live_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Completed
            emit RejectAllAgentEventSubscription { reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAllAgentEventsNoSessionBindingsDestroyed {
            on input SubscribeAllAgentEvents { session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            guard "no_session_bound_runtime" { session_bound_runtimes == EmptySet }
            guard "live_members_present" { self.live_runtime_ids != EmptySet }
            update {}
            to Destroyed
            emit RejectAllAgentEventSubscription { reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }

        transition SubscribeMobEventsRunning {
            on input SubscribeMobEvents { initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Running }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            guard "poll_interval_positive" { poll_interval_ms > 0 }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            update {}
            to Running
            emit AuthorizeMobEventRouter { initial_cursor: initial_cursor, channel_capacity: channel_capacity, poll_interval_ms: poll_interval_ms, session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeMobEventsStopped {
            on input SubscribeMobEvents { initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            guard "poll_interval_positive" { poll_interval_ms > 0 }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            update {}
            to Stopped
            emit AuthorizeMobEventRouter { initial_cursor: initial_cursor, channel_capacity: channel_capacity, poll_interval_ms: poll_interval_ms, session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeMobEventsCompleted {
            on input SubscribeMobEvents { initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            guard "poll_interval_positive" { poll_interval_ms > 0 }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            update {}
            to Completed
            emit AuthorizeMobEventRouter { initial_cursor: initial_cursor, channel_capacity: channel_capacity, poll_interval_ms: poll_interval_ms, session_bound_runtimes: session_bound_runtimes }
        }
        transition SubscribeMobEventsDestroyed {
            on input SubscribeMobEvents { initial_cursor, channel_capacity, poll_interval_ms, session_bound_runtimes }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            guard "poll_interval_positive" { poll_interval_ms > 0 }
            guard "session_bound_runtimes_match" { mob_machine_session_bound_live_runtime_ids_match(self.identity_to_runtime, self.member_session_bindings, self.live_runtime_ids, session_bound_runtimes) }
            update {}
            to Destroyed
            emit AuthorizeMobEventRouter { initial_cursor: initial_cursor, channel_capacity: channel_capacity, poll_interval_ms: poll_interval_ms, session_bound_runtimes: session_bound_runtimes }
        }

        transition SubscribeStructuralEventsRunning {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            guard "batch_limit_positive" { batch_limit > 0 }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            update {}
            to Running
            emit AuthorizeStructuralEventSubscription { after_cursor: after_cursor, explicit_after_cursor: explicit_after_cursor, batch_limit: batch_limit, channel_capacity: channel_capacity }
        }
        transition SubscribeStructuralEventsStopped {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            guard "batch_limit_positive" { batch_limit > 0 }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            update {}
            to Stopped
            emit AuthorizeStructuralEventSubscription { after_cursor: after_cursor, explicit_after_cursor: explicit_after_cursor, batch_limit: batch_limit, channel_capacity: channel_capacity }
        }
        transition SubscribeStructuralEventsCompleted {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            guard "batch_limit_positive" { batch_limit > 0 }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            update {}
            to Completed
            emit AuthorizeStructuralEventSubscription { after_cursor: after_cursor, explicit_after_cursor: explicit_after_cursor, batch_limit: batch_limit, channel_capacity: channel_capacity }
        }
        transition SubscribeStructuralEventsDestroyed {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            guard "batch_limit_positive" { batch_limit > 0 }
            guard "channel_capacity_positive" { channel_capacity > 0 }
            update {}
            to Destroyed
            emit AuthorizeStructuralEventSubscription { after_cursor: after_cursor, explicit_after_cursor: explicit_after_cursor, batch_limit: batch_limit, channel_capacity: channel_capacity }
        }
        transition SubscribeStructuralEventsStaleRunning {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Running
            emit RejectStructuralEventSubscription { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition SubscribeStructuralEventsStaleStopped {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Stopped
            emit RejectStructuralEventSubscription { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition SubscribeStructuralEventsStaleCompleted {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Completed
            emit RejectStructuralEventSubscription { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition SubscribeStructuralEventsStaleDestroyed {
            on input SubscribeStructuralEvents { after_cursor, latest_cursor, explicit_after_cursor, batch_limit, channel_capacity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Destroyed
            emit RejectStructuralEventSubscription { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }

        transition PollEventsStrictRunning {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Running }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            update {}
            to Running
            emit AuthorizeStrictEventPoll { after_cursor: after_cursor, limit: limit }
        }
        transition PollEventsStrictStopped {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            update {}
            to Stopped
            emit AuthorizeStrictEventPoll { after_cursor: after_cursor, limit: limit }
        }
        transition PollEventsStrictCompleted {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            update {}
            to Completed
            emit AuthorizeStrictEventPoll { after_cursor: after_cursor, limit: limit }
        }
        transition PollEventsStrictDestroyed {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "cursor_not_stale" { after_cursor <= latest_cursor }
            update {}
            to Destroyed
            emit AuthorizeStrictEventPoll { after_cursor: after_cursor, limit: limit }
        }
        transition PollEventsStrictStaleRunning {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Running }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Running
            emit RejectStrictEventPoll { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition PollEventsStrictStaleStopped {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Stopped
            emit RejectStrictEventPoll { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition PollEventsStrictStaleCompleted {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Completed
            emit RejectStrictEventPoll { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }
        transition PollEventsStrictStaleDestroyed {
            on input PollEventsStrict { after_cursor, latest_cursor, limit }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "cursor_stale" { after_cursor > latest_cursor }
            update {}
            to Destroyed
            emit RejectStrictEventPoll { after_cursor: after_cursor, latest_cursor: latest_cursor }
        }

        transition AuthorizeMobEventRouterMemberSubscriptionRunning {
            on input AuthorizeMobEventRouterMemberSubscription { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Running
            emit AuthorizeMobEventRouterMemberSubscription { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, session_id: self.member_session_bindings.get_cloned(agent_identity).get("value") }
        }
        transition AuthorizeMobEventRouterMemberSubscriptionStopped {
            on input AuthorizeMobEventRouterMemberSubscription { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Stopped
            emit AuthorizeMobEventRouterMemberSubscription { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, session_id: self.member_session_bindings.get_cloned(agent_identity).get("value") }
        }
        transition AuthorizeMobEventRouterMemberSubscriptionCompleted {
            on input AuthorizeMobEventRouterMemberSubscription { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Completed
            emit AuthorizeMobEventRouterMemberSubscription { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, session_id: self.member_session_bindings.get_cloned(agent_identity).get("value") }
        }
        transition AuthorizeMobEventRouterMemberSubscriptionDestroyed {
            on input AuthorizeMobEventRouterMemberSubscription { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_bound" { self.member_session_bindings.contains_key(agent_identity) == true }
            update {}
            to Destroyed
            emit AuthorizeMobEventRouterMemberSubscription { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, session_id: self.member_session_bindings.get_cloned(agent_identity).get("value") }
        }

        transition AuthorizeMobEventRouterMemberRemovalMissingRunning {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Running
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalMissingStopped {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Stopped
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalMissingCompleted {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Completed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalMissingDestroyed {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Destroyed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundRunning {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Running
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundStopped {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Stopped
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundCompleted {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Completed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundDestroyed {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { self.member_session_bindings.contains_key(agent_identity) == false }
            update {}
            to Destroyed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveRunning {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Running
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveStopped {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Stopped
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveCompleted {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Completed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveDestroyed {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_not_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false }
            update {}
            to Destroyed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
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
            on input CancelFlow { run_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_known" { self.run_status.contains_key(run_id) == true }
            update {}
            to Running
            emit NotifyCoordinator
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
            on signal StageSpawn { agent_identity, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_identity_unused" { self.pending_spawn_sessions.contains_key(agent_identity) == false }
            update {
                self.pending_spawn_count += 1;
                self.pending_spawn_sessions.insert(agent_identity, session_id);
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
            on input RunFlow { run_id, step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "run_seed_is_new" { self.run_status.contains_key(run_id) == false }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Pending);
                self.run_tracked_steps.insert(run_id, step_ids);
                self.run_ordered_steps.insert(run_id, ordered_steps);
                self.run_step_status.insert(run_id, EmptyMap);
                self.run_output_recorded.insert(run_id, EmptyMap);
                self.run_step_condition_results.insert(run_id, EmptyMap);
                self.run_step_has_conditions.insert(run_id, step_has_conditions);
                self.run_step_dependencies.insert(run_id, step_dependencies);
                self.run_step_dependency_modes.insert(run_id, step_dependency_modes);
                self.run_step_branches.insert(run_id, step_branches);
                self.run_step_collection_policies.insert(run_id, step_collection_policies);
                self.run_step_quorum_thresholds.insert(run_id, step_quorum_thresholds);
                self.run_step_target_counts.insert(run_id, EmptyMap);
                self.run_step_target_success_counts.insert(run_id, EmptyMap);
                self.run_step_target_terminal_failure_counts.insert(run_id, EmptyMap);
                self.run_target_retry_counts.insert(run_id, EmptyMap);
                self.run_failure_count.insert(run_id, 0);
                self.run_consecutive_failure_count.insert(run_id, 0);
                self.run_escalation_threshold.insert(run_id, escalation_threshold);
                self.run_max_step_retries.insert(run_id, max_step_retries);
                self.run_ready_frames.insert(run_id, EmptySeq);
                self.run_ready_frame_membership.insert(run_id, EmptySet);
                self.run_pending_body_frame_loops.insert(run_id, EmptySeq);
                self.run_pending_body_frame_loop_membership.insert(run_id, EmptySet);
                self.run_active_node_count.insert(run_id, 0);
                self.run_active_frame_count.insert(run_id, 0);
                self.run_max_active_nodes.insert(run_id, max_active_nodes);
                self.run_max_active_frames.insert(run_id, max_active_frames);
                self.run_max_frame_depth.insert(run_id, max_frame_depth);
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition CreateRunSeedRunning {
            on input CreateRunSeed { run_id, step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_seed_is_new" { self.run_status.contains_key(run_id) == false }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Pending);
                self.run_tracked_steps.insert(run_id, step_ids);
                self.run_ordered_steps.insert(run_id, ordered_steps);
                self.run_step_status.insert(run_id, EmptyMap);
                self.run_output_recorded.insert(run_id, EmptyMap);
                self.run_step_condition_results.insert(run_id, EmptyMap);
                self.run_step_has_conditions.insert(run_id, step_has_conditions);
                self.run_step_dependencies.insert(run_id, step_dependencies);
                self.run_step_dependency_modes.insert(run_id, step_dependency_modes);
                self.run_step_branches.insert(run_id, step_branches);
                self.run_step_collection_policies.insert(run_id, step_collection_policies);
                self.run_step_quorum_thresholds.insert(run_id, step_quorum_thresholds);
                self.run_step_target_counts.insert(run_id, EmptyMap);
                self.run_step_target_success_counts.insert(run_id, EmptyMap);
                self.run_step_target_terminal_failure_counts.insert(run_id, EmptyMap);
                self.run_target_retry_counts.insert(run_id, EmptyMap);
                self.run_failure_count.insert(run_id, 0);
                self.run_consecutive_failure_count.insert(run_id, 0);
                self.run_escalation_threshold.insert(run_id, escalation_threshold);
                self.run_max_step_retries.insert(run_id, max_step_retries);
                self.run_ready_frames.insert(run_id, EmptySeq);
                self.run_ready_frame_membership.insert(run_id, EmptySet);
                self.run_pending_body_frame_loops.insert(run_id, EmptySeq);
                self.run_pending_body_frame_loop_membership.insert(run_id, EmptySet);
                self.run_active_node_count.insert(run_id, 0);
                self.run_active_frame_count.insert(run_id, 0);
                self.run_max_active_nodes.insert(run_id, max_active_nodes);
                self.run_max_active_frames.insert(run_id, max_active_frames);
                self.run_max_frame_depth.insert(run_id, max_frame_depth);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition CreateFrameSeedRunning {
            on input CreateFrameSeed { run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue }
            guard { self.lifecycle_phase == Phase::Running }
            guard "frame_seed_is_new" { self.frame_phase.contains_key(frame_id) == false }
            guard "run_known" { self.run_status.contains_key(run_id) == true }
            guard "body_frame_has_parent_loop" { frame_scope != FrameScope::Body || loop_instance_id != None }
            guard "frame_seed_status_covers_tracked_nodes" {
                for_all(candidate in tracked_nodes,
                    node_status.contains_key(candidate))
            }
            guard "frame_seed_node_kind_covers_tracked_nodes" {
                for_all(candidate in tracked_nodes,
                    node_kind.contains_key(candidate))
            }
            guard "frame_seed_node_kind_keys_are_tracked" {
                for_all(candidate in node_kind.keys(),
                    tracked_nodes.contains(candidate))
            }
            guard "frame_seed_ready_queue_matches_dependency_roots" {
                for_all(candidate in tracked_nodes,
                    (node_dependencies.get_cloned(candidate).get("value") == EmptySeq
                        && node_status.get_cloned(candidate) == Some(NodeRunStatus::Ready)
                        && ready_queue.contains(candidate))
                    || (node_dependencies.get_cloned(candidate).get("value") != EmptySeq
                        && node_status.get_cloned(candidate) == Some(NodeRunStatus::Pending)
                        && ready_queue.contains(candidate) == false))
            }
            guard "frame_seed_step_nodes_have_exact_step_ids" {
                for_all(candidate in tracked_nodes,
                    (node_kind.get_cloned(candidate) == Some(FlowNodeKind::Step)
                        && node_step_ids.contains_key(candidate)
                        && node_loop_ids.contains_key(candidate) == false)
                    || node_kind.get_cloned(candidate) != Some(FlowNodeKind::Step))
            }
            guard "frame_seed_loop_nodes_have_exact_loop_ids" {
                for_all(candidate in tracked_nodes,
                    (node_kind.get_cloned(candidate) == Some(FlowNodeKind::Loop)
                        && node_loop_ids.contains_key(candidate)
                        && node_step_ids.contains_key(candidate) == false)
                    || node_kind.get_cloned(candidate) != Some(FlowNodeKind::Loop))
            }
            guard "frame_seed_step_id_keys_are_tracked_steps" {
                for_all(candidate in node_step_ids.keys(),
                    tracked_nodes.contains(candidate)
                    && node_kind.get_cloned(candidate) == Some(FlowNodeKind::Step))
            }
            guard "frame_seed_loop_id_keys_are_tracked_loops" {
                for_all(candidate in node_loop_ids.keys(),
                    tracked_nodes.contains(candidate)
                    && node_kind.get_cloned(candidate) == Some(FlowNodeKind::Loop))
            }
            update {
                self.frame_scope.insert(frame_id, frame_scope);
                self.frame_phase.insert(frame_id, FrameStatus::Running);
                self.frame_run.insert(frame_id, run_id);
                self.frame_parent_loop.insert(frame_id, loop_instance_id);
                self.frame_iteration.insert(frame_id, iteration);
                self.frame_tracked_nodes.insert(frame_id, tracked_nodes);
                self.frame_ordered_nodes.insert(frame_id, ordered_nodes);
                self.frame_node_kind.insert(frame_id, node_kind);
                self.frame_node_dependencies.insert(frame_id, node_dependencies);
                self.frame_node_dependency_modes.insert(frame_id, node_dependency_modes);
                self.frame_node_branches.insert(frame_id, node_branches);
                self.frame_node_step_ids.insert(frame_id, node_step_ids);
                self.frame_node_loop_ids.insert(frame_id, node_loop_ids);
                self.frame_node_status.insert(frame_id, node_status);
                self.frame_ready_queue.insert(frame_id, ready_queue);
                self.frame_output_recorded.insert(frame_id, EmptyMap);
                self.frame_node_condition_results.insert(frame_id, EmptyMap);
                if frame_scope == FrameScope::Body {
                    self.loop_stage.insert(loop_instance_id.get("value"), LoopIterationStage::BodyFrameActive);
                    self.loop_active_body_frame.insert(loop_instance_id.get("value"), Some(frame_id));
                }
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition CreateLoopSeedRunning {
            on input CreateLoopSeed { loop_instance_id, parent_frame_id, parent_node_id, loop_id, depth, max_iterations }
            guard { self.lifecycle_phase == Phase::Running }
            guard "loop_seed_is_new" { self.loop_phase.contains_key(loop_instance_id) == false }
            guard "parent_frame_known" { self.frame_run.contains_key(parent_frame_id) == true }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Running);
                self.loop_parent_frame.insert(loop_instance_id, parent_frame_id);
                self.loop_parent_node.insert(loop_instance_id, parent_node_id);
                self.loop_definition.insert(loop_instance_id, loop_id);
                self.loop_depth.insert(loop_instance_id, depth);
                self.loop_stage.insert(loop_instance_id, LoopIterationStage::AwaitingBodyFrame);
                self.loop_current_iteration.insert(loop_instance_id, 0u64);
                self.loop_last_completed_iteration.insert(loop_instance_id, 0u64);
                self.loop_max_iterations.insert(loop_instance_id, max_iterations);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition RecordLoopBodyFrameCompletedRunning {
            on input RecordLoopBodyFrameCompleted { loop_instance_id, iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "body_frame_active" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::BodyFrameActive) }
            guard "iteration_matches_current" { self.loop_current_iteration.get_cloned(loop_instance_id) == Some(iteration) }
            update {
                self.loop_stage.insert(loop_instance_id, LoopIterationStage::AwaitingUntilEvaluation);
                self.loop_last_completed_iteration.insert(loop_instance_id, iteration);
                self.loop_current_iteration.insert(loop_instance_id, iteration + 1u64);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition RecordLoopUntilConditionMetRunning {
            on input RecordLoopUntilConditionMet { loop_instance_id, iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "awaiting_until_evaluation" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::AwaitingUntilEvaluation) }
            guard "iteration_matches_last_completed" { self.loop_last_completed_iteration.get_cloned(loop_instance_id) == Some(iteration) }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Completed);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition RecordLoopUntilConditionFailedRunning {
            on input RecordLoopUntilConditionFailed { loop_instance_id, iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "awaiting_until_evaluation" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::AwaitingUntilEvaluation) }
            guard "iteration_matches_last_completed" { self.loop_last_completed_iteration.get_cloned(loop_instance_id) == Some(iteration) }
            guard "iterations_remaining" {
                self.loop_current_iteration.get_cloned(loop_instance_id).get("value")
                    < self.loop_max_iterations.get_cloned(loop_instance_id).get("value")
            }
            update {
                self.loop_stage.insert(loop_instance_id, LoopIterationStage::AwaitingBodyFrame);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition RecordLoopUntilConditionFailedExhausted {
            on input RecordLoopUntilConditionFailed { loop_instance_id, iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "awaiting_until_evaluation" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::AwaitingUntilEvaluation) }
            guard "iteration_matches_last_completed" { self.loop_last_completed_iteration.get_cloned(loop_instance_id) == Some(iteration) }
            guard "iterations_exhausted" {
                self.loop_current_iteration.get_cloned(loop_instance_id).get("value")
                    >= self.loop_max_iterations.get_cloned(loop_instance_id).get("value")
            }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Exhausted);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandStartRun {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "start_run_command" { command == FlowRunReducerCommandKind::StartRun }
            guard "run_pending" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Pending) }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Running);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandDispatchStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "dispatch_step_command" { command == FlowRunReducerCommandKind::DispatchStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "dispatched_step_status" { step_status == Some(StepRunStatus::Dispatched) }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Dispatched);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandCompleteStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "complete_step_command" { command == FlowRunReducerCommandKind::CompleteStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "completed_step_status" { step_status == Some(StepRunStatus::Completed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Completed);
                self.run_consecutive_failure_count.insert(run_id, 0);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordStepOutput {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_step_output_command" { command == FlowRunReducerCommandKind::RecordStepOutput }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_output_recorded_flat.insert(run_step_key.get("value"), true);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandConditionPassed {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "condition_passed_command" { command == FlowRunReducerCommandKind::ConditionPassed }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_condition_results_flat.insert(run_step_key.get("value"), Some(true));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandConditionRejected {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "condition_rejected_command" { command == FlowRunReducerCommandKind::ConditionRejected }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_condition_results_flat.insert(run_step_key.get("value"), Some(false));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandFailStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "fail_step_command" { command == FlowRunReducerCommandKind::FailStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "failed_step_status" { step_status == Some(StepRunStatus::Failed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "supervisor_escalation_not_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") == 0
                || self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    < self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
        }

        transition AuthorizeFlowRunReducerCommandFailStepEscalating {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "fail_step_command" { command == FlowRunReducerCommandKind::FailStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "failed_step_status" { step_status == Some(StepRunStatus::Failed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "supervisor_escalation_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") > 0
                && self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    >= self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
            emit EscalateSupervisor
        }

        transition AuthorizeFlowRunReducerCommandSkipStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "skip_step_command" { command == FlowRunReducerCommandKind::SkipStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "skipped_step_status" { step_status == Some(StepRunStatus::Skipped) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Skipped);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandProjectFrameStepStatus {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status_flat.contains_key(run_step_key.get("value")) == false
                || self.run_step_status_flat.get_cloned(run_step_key.get("value")) == Some(StepRunStatus::Dispatched)
            }
            guard "frame_node_completed_or_skipped" {
                self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Completed)
                || self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Skipped)
            }
            update {
                self.run_step_status_flat.insert(
                    run_step_key.get("value"),
                    mob_machine_step_status_from_frame_node_status(
                        self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")).get("value")
                    )
                );
                if self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Completed) {
                    self.run_consecutive_failure_count.insert(run_id, 0);
                }
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailed {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status_flat.contains_key(run_step_key.get("value")) == false
                || self.run_step_status_flat.get_cloned(run_step_key.get("value")) == Some(StepRunStatus::Dispatched)
            }
            guard "frame_node_failed" {
                self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Failed)
            }
            guard "supervisor_escalation_not_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") == 0
                || self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    < self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
        }

        transition AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalating {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status_flat.contains_key(run_step_key.get("value")) == false
                || self.run_step_status_flat.get_cloned(run_step_key.get("value")) == Some(StepRunStatus::Dispatched)
            }
            guard "frame_node_failed" {
                self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Failed)
            }
            guard "supervisor_escalation_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") > 0
                && self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    >= self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
            emit EscalateSupervisor
        }

        transition AuthorizeFlowRunReducerCommandCancelStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "cancel_step_command" { command == FlowRunReducerCommandKind::CancelStep }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "canceled_step_status" { step_status == Some(StepRunStatus::Canceled) }
            update {
                self.run_step_status_flat.insert(run_step_key.get("value"), StepRunStatus::Canceled);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterTargets {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "register_targets_command" { command == FlowRunReducerCommandKind::RegisterTargets }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "has_target_count" { target_count != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_counts_flat.insert(run_step_key.get("value"), target_count.get("value"));
                self.run_step_target_success_counts_flat.insert(run_step_key.get("value"), 0);
                self.run_step_target_terminal_failure_counts_flat.insert(run_step_key.get("value"), 0);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetSuccess {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_success_command" { command == FlowRunReducerCommandKind::RecordTargetSuccess }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_success_counts_flat.increment(run_step_key.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetTerminalFailure {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_terminal_failure_command" { command == FlowRunReducerCommandKind::RecordTargetTerminalFailure }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_terminal_failure_counts_flat.increment(run_step_key.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetCanceled {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_canceled_command" { command == FlowRunReducerCommandKind::RecordTargetCanceled }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetFailure {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_failure_command" { command == FlowRunReducerCommandKind::RecordTargetFailure }
            guard "has_step_id" { step_id != None }
            guard "has_run_step_key" { run_step_key != None }
            guard "has_retry_key" { retry_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_target_retry_counts_flat.increment(run_step_key.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterReadyFrame {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "register_ready_frame_command" { command == FlowRunReducerCommandKind::RegisterReadyFrame }
            guard "has_frame_id" { frame_id != None }
            guard "known_frame" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_not_already_ready" { self.run_ready_frame_membership_flat.contains(frame_id.get("value")) == false }
            update {
                self.run_ready_frame_membership_flat.insert(frame_id.get("value"));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterReadyFrameAlreadyReady {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "register_ready_frame_command" { command == FlowRunReducerCommandKind::RegisterReadyFrame }
            guard "has_frame_id" { frame_id != None }
            guard "known_frame" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_already_ready" { self.run_ready_frame_membership_flat.contains(frame_id.get("value")) == true }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandPumpNodeScheduler {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "pump_node_scheduler_command" { command == FlowRunReducerCommandKind::PumpNodeScheduler }
            guard "has_frame_id" { frame_id != None }
            guard "ready_frame_registered" { self.run_ready_frame_membership_flat.contains(frame_id.get("value")) == true }
            guard "machine_selected_ready_frame" {
                for_all(candidate in self.run_ready_frame_membership_flat,
                    self.frame_run.get_cloned(candidate) != Some(run_id)
                    || frame_id.get("value") <= candidate)
            }
            guard "node_capacity_available" {
                self.run_max_active_nodes.get_cloned(run_id).get("value") == 0
                || self.run_active_node_count.get_cloned(run_id).get("value") < self.run_max_active_nodes.get_cloned(run_id).get("value")
            }
            update {
                self.run_ready_frame_membership_flat.remove(frame_id.get("value"));
                self.run_active_node_count.increment(run_id, 1);
                self.run_last_granted_frame.insert(run_id, frame_id.get("value"));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterPendingBodyFrame {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "register_pending_body_frame_command" { command == FlowRunReducerCommandKind::RegisterPendingBodyFrame }
            guard "has_loop_instance_id" { loop_instance_id != None }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id.get("value")) == true }
            guard "loop_not_already_pending_body_frame" { self.run_pending_body_frame_loop_membership_flat.contains(loop_instance_id.get("value")) == false }
            update {
                self.run_pending_body_frame_loop_membership_flat.insert(loop_instance_id.get("value"));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandPumpFrameScheduler {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "pump_frame_scheduler_command" { command == FlowRunReducerCommandKind::PumpFrameScheduler }
            guard "has_loop_instance_id" { loop_instance_id != None }
            guard "pending_body_frame_registered" { self.run_pending_body_frame_loop_membership_flat.contains(loop_instance_id.get("value")) == true }
            guard "machine_selected_pending_body_frame_loop" {
                for_all(candidate in self.run_pending_body_frame_loop_membership_flat,
                    self.loop_parent_frame.contains_key(candidate) == false
                    || self.frame_run.get_cloned(self.loop_parent_frame.get_cloned(candidate).get("value")) != Some(run_id)
                    || loop_instance_id.get("value") <= candidate)
            }
            guard "frame_capacity_available" {
                self.run_max_active_frames.get_cloned(run_id).get("value") == 0
                || self.run_active_frame_count.get_cloned(run_id).get("value") < self.run_max_active_frames.get_cloned(run_id).get("value")
            }
            update {
                self.run_pending_body_frame_loop_membership_flat.remove(loop_instance_id.get("value"));
                self.run_active_frame_count.increment(run_id, 1);
                self.run_last_granted_loop.insert(run_id, loop_instance_id.get("value"));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandNodeExecutionReleased {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "node_execution_released_command" { command == FlowRunReducerCommandKind::NodeExecutionReleased }
            guard "active_node_count_present" { self.run_active_node_count.contains_key(run_id) == true }
            guard "active_node_count_positive" { self.run_active_node_count.get_cloned(run_id).get("value") > 0 }
            update {
                self.run_active_node_count.decrement(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandFrameTerminated {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "frame_terminated_command" { command == FlowRunReducerCommandKind::FrameTerminated }
            guard "active_frame_count_present" { self.run_active_frame_count.contains_key(run_id) == true }
            guard "active_frame_count_positive" { self.run_active_frame_count.get_cloned(run_id).get("value") > 0 }
            update {
                self.run_active_frame_count.decrement(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandFrameTerminatedNoActiveFrame {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "frame_terminated_command" { command == FlowRunReducerCommandKind::FrameTerminated }
            guard "active_frame_count_present" { self.run_active_frame_count.contains_key(run_id) == true }
            guard "active_frame_count_zero" { self.run_active_frame_count.get_cloned(run_id).get("value") == 0 }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandTerminalCompleted {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "terminal_completed_command" { command == FlowRunReducerCommandKind::TerminalizeCompleted }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Completed);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandTerminalFailed {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "terminal_failed_command" { command == FlowRunReducerCommandKind::TerminalizeFailed }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Failed);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandTerminalCanceled {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, run_step_key, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "terminal_canceled_command" { command == FlowRunReducerCommandKind::TerminalizeCanceled }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Canceled);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandAdmitNextReadyNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "admit_next_ready_node_command" { command == FlowFrameReducerCommandKind::AdmitNextReadyNode }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "running_node_status" { node_status == Some(NodeRunStatus::Running) }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            guard "node_currently_ready" { self.frame_ready_queue.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            update {
                self.frame_node_status = mob_machine_frame_node_status_after_admit(self.frame_node_status, self.frame_node_branches, self.frame_ordered_nodes, frame_id, node_id.get("value"));
                self.frame_last_admitted_node.insert(frame_id, node_id.get("value"));
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_admit(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandCompleteNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "complete_node_command" { command == FlowFrameReducerCommandKind::CompleteNode }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "completed_node_status" { node_status == Some(NodeRunStatus::Completed) }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            guard "node_currently_running" { self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Running) }
            update {
                self.frame_node_status = mob_machine_frame_node_status_after_terminal(self.frame_node_status, self.frame_node_branches, self.frame_ordered_nodes, self.frame_node_dependencies, self.frame_node_dependency_modes, frame_id, node_id.get("value"), NodeRunStatus::Completed);
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_terminal(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandRecordNodeOutput {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "record_node_output_command" { command == FlowFrameReducerCommandKind::RecordNodeOutput }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "has_frame_node_key" { frame_node_key != None }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            update {
                self.frame_output_recorded_flat.insert(frame_node_key.get("value"), true);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandFailNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "fail_node_command" { command == FlowFrameReducerCommandKind::FailNode }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "failed_node_status" { node_status == Some(NodeRunStatus::Failed) }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            guard "node_currently_running" { self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Running) }
            update {
                self.frame_node_status = mob_machine_frame_node_status_after_terminal(self.frame_node_status, self.frame_node_branches, self.frame_ordered_nodes, self.frame_node_dependencies, self.frame_node_dependency_modes, frame_id, node_id.get("value"), NodeRunStatus::Failed);
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_terminal(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandSkipNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "skip_node_command" { command == FlowFrameReducerCommandKind::SkipNode }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "skipped_node_status" { node_status == Some(NodeRunStatus::Skipped) }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            guard "node_currently_running" { self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Running) }
            update {
                self.frame_node_status = mob_machine_frame_node_status_after_terminal(self.frame_node_status, self.frame_node_branches, self.frame_ordered_nodes, self.frame_node_dependencies, self.frame_node_dependency_modes, frame_id, node_id.get("value"), NodeRunStatus::Skipped);
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_terminal(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandCancelNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "cancel_node_command" { command == FlowFrameReducerCommandKind::CancelNode }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "canceled_node_status" { node_status == Some(NodeRunStatus::Canceled) }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            guard "node_currently_running" { self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Running) }
            update {
                self.frame_node_status = mob_machine_frame_node_status_after_terminal(self.frame_node_status, self.frame_node_branches, self.frame_ordered_nodes, self.frame_node_dependencies, self.frame_node_dependency_modes, frame_id, node_id.get("value"), NodeRunStatus::Canceled);
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_terminal(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandSealFrame {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, frame_node_key, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "seal_frame_command" { command == FlowFrameReducerCommandKind::SealFrame }
            guard "terminal_frame_status" {
                terminal_status == Some(FrameStatus::Completed)
                || terminal_status == Some(FrameStatus::Failed)
                || terminal_status == Some(FrameStatus::Canceled)
            }
            guard "all_nodes_terminal" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Completed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Failed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Skipped)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Canceled))
            }
            guard "terminal_class_matches_nodes" {
                (terminal_status == Some(FrameStatus::Failed)
                    && for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                        self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed)) == false)
                || (terminal_status == Some(FrameStatus::Canceled)
                    && for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                        self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed))
                    && for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                        self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Canceled)) == false)
                || (terminal_status == Some(FrameStatus::Completed)
                    && for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                        self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed))
                    && for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                        self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Canceled)))
            }
            update {
                self.frame_phase.insert(frame_id, terminal_status.get("value"));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandBodyFrameStarted {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "body_frame_started_command" { command == LoopIterationReducerCommandKind::BodyFrameStarted }
            guard "blocked_use_CreateFrameSeed_body_side_effect" { false }
            guard "no_body_frame_iteration" { body_frame_iteration == None }
            guard "body_frame_already_active" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::BodyFrameActive) }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandBodyFrameCompleted {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "body_frame_active" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::BodyFrameActive) }
            guard "body_frame_completed_command" { command == LoopIterationReducerCommandKind::BodyFrameCompleted }
            guard "blocked_use_RecordLoopBodyFrameCompleted" { false }
            guard "body_frame_iteration_present" { body_frame_iteration != None }
            guard "iteration_matches_current" {
                self.loop_current_iteration.get_cloned(loop_instance_id) == body_frame_iteration
            }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandBodyFrameFailed {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "body_frame_active" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::BodyFrameActive) }
            guard "body_frame_failed_command" { command == LoopIterationReducerCommandKind::BodyFrameFailed }
            guard "body_frame_iteration_present" { body_frame_iteration != None }
            guard "iteration_matches_current" {
                self.loop_current_iteration.get_cloned(loop_instance_id) == body_frame_iteration
            }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Failed);
                self.loop_last_completed_iteration.insert(loop_instance_id, body_frame_iteration.get("value"));
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandBodyFrameCanceled {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "body_frame_active" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::BodyFrameActive) }
            guard "body_frame_canceled_command" { command == LoopIterationReducerCommandKind::BodyFrameCanceled }
            guard "body_frame_iteration_present" { body_frame_iteration != None }
            guard "iteration_matches_current" {
                self.loop_current_iteration.get_cloned(loop_instance_id) == body_frame_iteration
            }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Canceled);
                self.loop_last_completed_iteration.insert(loop_instance_id, body_frame_iteration.get("value"));
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandUntilFeedback {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "awaiting_until_evaluation" { self.loop_stage.get_cloned(loop_instance_id) == Some(LoopIterationStage::AwaitingUntilEvaluation) }
            guard "blocked_use_RecordLoopUntilConditionFeedback" { false }
            guard "no_body_frame_iteration" { body_frame_iteration == None }
            guard "until_feedback_command" {
                command == LoopIterationReducerCommandKind::UntilConditionMet
                || command == LoopIterationReducerCommandKind::UntilConditionFailed
            }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeLoopIterationReducerCommandCancelLoop {
            on input AuthorizeLoopIterationReducerCommand { loop_instance_id, command, body_frame_id, body_frame_iteration }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_loop" { self.loop_phase.contains_key(loop_instance_id) == true }
            guard "loop_running" { self.loop_phase.get_cloned(loop_instance_id) == Some(LoopStatus::Running) }
            guard "cancel_loop_command" { command == LoopIterationReducerCommandKind::CancelLoop }
            guard "no_body_frame_iteration" { body_frame_iteration == None }
            update {
                self.loop_phase.insert(loop_instance_id, LoopStatus::Canceled);
                self.loop_active_body_frame.insert(loop_instance_id, None);
            }
            to Running
            emit EmitRunLifecycleNotice
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
        // CompleteFlow / FinishRun
        // =====================================================================
        //
        // Two independent flow-terminalization paths drive the authority to
        // the same state:
        //   1. Natural completion: a run's task finishes and
        //      `handle_flow_cleanup` fires `CompleteFlow` + `FinishRun`
        //      (decrementing `active_run_count`, clearing the run-tracker).
        //   2. Destroy-driven cancel: `cancel_all_flow_tasks` iterates the
        //      run-tracker and fires the same signals for any run that has
        //      not already been cleaned up.
        // Because actor-command ordering is unordered between these two
        // paths, they race. Whichever lands first drives
        // `active_run_count` from 1 → 0; the other arrives with the counter
        // already at 0. The *Zero transitions below model "CompleteFlow /
        // FinishRun at count 0" as a legitimate terminal convergence
        // (no-op update, same target phase) rather than as an error the
        // caller must paper over — dogma requires that convergence
        // semantics live in the machine authority.

        transition CompleteFlowRunning {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit FlowTerminalized
        }

        transition CompleteFlowRunningZero {
            on signal CompleteFlow
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Completed }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        transition FinishRunRunning {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "active_runs_present" { self.active_run_count > 0 }
            update {
                self.active_run_count -= 1;
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition FinishRunRunningZero {
            on signal FinishRun
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "no_active_runs" { self.active_run_count == 0 }
            update {}
            to Running
            emit NotifyCoordinator
        }

        // =====================================================================
        // Retire / RetireAll
        // =====================================================================

        transition RetireRunningReleasing {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            guard "releasing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(releasing.get("value")) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
        }

        transition RetireRunningPreservingBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireRunningNoBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireStoppedReleasing {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            guard "releasing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(releasing.get("value")) }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
        }

        // MobMachine does not own a mob-id field, so feedback `mob_id` and
        // failure `reason` remain protocol payload. The machine-owned check is
        // the pending-detach runtime id opened by the Retire transition.
        transition RequestPendingSessionIngressDetachForMobDestroyRunning {
            on input RequestPendingSessionIngressDetachForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
        }

        transition RequestPendingSessionIngressDetachForMobDestroyStopped {
            on input RequestPendingSessionIngressDetachForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Stopped
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
        }

        transition SessionIngressDetachedForMobDestroyRunning {
            on input SessionIngressDetachedForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
            }
            to Running
        }

        transition SessionIngressDetachedForMobDestroyStopped {
            on input SessionIngressDetachedForMobDestroy { mob_id, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
            }
            to Stopped
        }

        transition SessionIngressDetachFailedForMobDestroyRunning {
            on input SessionIngressDetachFailedForMobDestroy { mob_id, agent_runtime_id, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
        }

        transition SessionIngressDetachFailedForMobDestroyStopped {
            on input SessionIngressDetachFailedForMobDestroy { mob_id, agent_runtime_id, reason }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "pending_detach_present" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            update {}
            to Stopped
        }

        transition RetireStoppedPreservingBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireStoppedNoBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: Some(session_id)
            }
            emit RequestRuntimeRetire { session_id: session_id }
        }

        transition RetireAbsentRunning {
            on input RetireAbsent { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Running
        }

        transition RetireAbsentStopped {
            on input RetireAbsent { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {}
            to Stopped
        }

        transition RetireAllRunning {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Running }
            update {}
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
        }

        transition RetireAllStopped {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
        }

        transition RetireAllCompleted {
            on input RetireAll
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retiring }
        }

        // =====================================================================
        // CompleteSpawn
        // =====================================================================

        transition CompleteSpawnRunning {
            on signal CompleteSpawn { agent_identity }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped }
            guard "pending_spawns_present" { self.pending_spawn_count > 0 }
            guard "pending_identity_present" { self.pending_spawn_sessions.contains_key(agent_identity) == true }
            update {
                self.pending_spawn_count -= 1;
                self.pending_spawn_sessions.remove(agent_identity);
            }
            to Running
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
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
            guard "session_ingress_detaches_closed" { self.pending_session_ingress_detach_runtime_ids == EmptySet }
            update {
                self.destroy_admitted = true;
                self.live_runtime_ids = EmptySet;
                self.externally_addressable_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.wiring_edges = EmptySet;
                self.external_peer_edges = EmptySet;
                self.external_peer_edges_by_key = EmptyMap;
                self.identity_to_runtime = EmptyMap;
                self.identity_runtime_generations = EmptyMap;
                self.identity_runtime_fence_tokens = EmptyMap;
                self.member_session_bindings = EmptyMap;
                self.member_peer_ids = EmptyMap;
                self.member_peer_endpoints = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
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
            on input CancelAllWork { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.active_run_count = 0;
            }
            to Running
            emit FlowTerminalized
        }

        transition ResolveCancelAllWorkRejectionStopped {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
            update {}
            to Stopped
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveCancelAllWorkRejectionCompleted {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Completed }
            update {}
            to Completed
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveCancelAllWorkRejectionDestroyed {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::MobNotRunning,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveCancelAllWorkRejectionMemberNotFound {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { !self.identity_to_runtime.contains_key(agent_identity) }
            update {}
            to Running
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::MemberNotFound,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveCancelAllWorkRejectionCurrentRuntimeNotLive {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) }
            guard "current_runtime_not_live" { !self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            update {}
            to Running
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::MemberNotFound,
                expected_fence_token: None,
                actual_fence_token: None
            }
        }

        transition ResolveCancelAllWorkRejectionStaleFenceToken {
            on input ResolveCancelAllWorkRejection { agent_identity, agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) }
            guard "current_runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "runtime_or_fence_stale" {
                self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id)
                || self.runtime_fence_tokens.get_copied(self.identity_to_runtime.get_cloned(agent_identity).get("value")) != Some(fence_token)
            }
            update {}
            to Running
            emit CancelAllWorkRejected {
                agent_runtime_id: agent_runtime_id,
                reason: CancelAllWorkRejectReasonKind::StaleFenceToken,
                expected_fence_token: self.runtime_fence_tokens.get_copied(self.identity_to_runtime.get_cloned(agent_identity).get("value")),
                actual_fence_token: Some(fence_token)
            }
        }

    }
        }

        impl MobMachineAuthority {
        fn mob_machine_session_bound_live_runtime_ids_match(
            identity_to_runtime: &std::collections::BTreeMap<AgentIdentity, AgentRuntimeId>,
            member_session_bindings: &std::collections::BTreeMap<AgentIdentity, SessionId>,
            live_runtime_ids: &std::collections::BTreeSet<AgentRuntimeId>,
            expected_runtime_ids: &std::collections::BTreeSet<AgentRuntimeId>,
        ) -> bool {
            let actual_runtime_ids = member_session_bindings
                .keys()
                .filter_map(|identity| identity_to_runtime.get(identity))
                .filter(|runtime_id| live_runtime_ids.contains(*runtime_id))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            &actual_runtime_ids == expected_runtime_ids
        }

        fn mob_machine_external_peer_edge_has_matching_key(
            edges_by_key: &std::collections::BTreeMap<ExternalPeerKey, ExternalPeerEdge>,
            edge: &ExternalPeerEdge,
        ) -> bool {
            let key = ExternalPeerKey::new(edge.local.clone(), edge.endpoint.name.clone());
            edges_by_key.get(&key) == Some(edge)
        }

        fn mob_machine_external_peer_key_matches_edge(
            key: &ExternalPeerKey,
            edge: &ExternalPeerEdge,
        ) -> bool {
            key.local == edge.local && key.name == edge.endpoint.name
        }

        fn mob_machine_external_peer_key_matches_local(
            key: &ExternalPeerKey,
            agent_identity: &AgentIdentity,
        ) -> bool {
            key.local == *agent_identity
        }

        fn mob_machine_external_peer_edge_peer_id(edge: &ExternalPeerEdge) -> PeerId {
            edge.endpoint.peer_id.clone()
        }

        fn mob_machine_external_peer_edge_local(edge: &ExternalPeerEdge) -> AgentIdentity {
            edge.local.clone()
        }

        fn mob_machine_member_peer_endpoint_peer_id(endpoint: &MemberPeerEndpoint) -> PeerId {
            endpoint.peer_id.clone()
        }

        fn mob_machine_wiring_edge_matches_members(
            edge: &WiringEdge,
            a_identity: &AgentIdentity,
            b_identity: &AgentIdentity,
        ) -> bool {
            edge.a == *a_identity && edge.b == *b_identity
        }

        fn mob_machine_frame_node_status_after_admit(
            all_statuses: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>>,
            frame_branches: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, Option<BranchId>>>,
            frame_ordered_nodes: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            frame_id: &FrameId,
            node_id: &FlowNodeId,
        ) -> std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>> {
            let mut all_statuses = all_statuses.clone();
            let statuses = all_statuses.entry(frame_id.clone()).or_default();
            let ordered_nodes = frame_ordered_nodes.get(&frame_id).cloned().unwrap_or_default();
            let branches = frame_branches.get(&frame_id).cloned().unwrap_or_default();
            let admitted_branch = branches.get(node_id).cloned().unwrap_or(None);
            if let Some(branch) = admitted_branch {
                for candidate in ordered_nodes {
                    if &candidate != node_id
                        && statuses.get(&candidate) == Some(&NodeRunStatus::Ready)
                        && branches.get(&candidate).cloned().unwrap_or(None) == Some(branch.clone())
                    {
                        statuses.insert(candidate, NodeRunStatus::Pending);
                    }
                }
            }
            statuses.insert(node_id.clone(), NodeRunStatus::Running);
            all_statuses
        }

        fn mob_machine_frame_ready_queue_after_admit(
            all_ready_queues: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            all_statuses: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>>,
            frame_ordered_nodes: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            frame_id: &FrameId,
        ) -> std::collections::BTreeMap<FrameId, Vec<FlowNodeId>> {
            let mut all_ready_queues = all_ready_queues.clone();
            let statuses = all_statuses.get(&frame_id).cloned().unwrap_or_default();
            let ordered_nodes = frame_ordered_nodes.get(&frame_id).cloned().unwrap_or_default();
            let ready = ordered_nodes
                .into_iter()
                .filter(|node_id| statuses.get(node_id) == Some(&NodeRunStatus::Ready))
                .collect();
            all_ready_queues.insert(frame_id.clone(), ready);
            all_ready_queues
        }

        fn mob_machine_node_terminal(status: NodeRunStatus) -> bool {
            matches!(
                status,
                NodeRunStatus::Completed
                    | NodeRunStatus::Failed
                    | NodeRunStatus::Skipped
                    | NodeRunStatus::Canceled
            )
        }

        fn mob_machine_step_status_from_frame_node_status(status: &NodeRunStatus) -> StepRunStatus {
            match *status {
                NodeRunStatus::Completed => StepRunStatus::Completed,
                NodeRunStatus::Skipped => StepRunStatus::Skipped,
                NodeRunStatus::Failed => StepRunStatus::Failed,
                NodeRunStatus::Canceled => StepRunStatus::Canceled,
                NodeRunStatus::Pending | NodeRunStatus::Ready | NodeRunStatus::Running => {
                    StepRunStatus::Dispatched
                }
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn mob_machine_frame_node_status_after_terminal(
            all_statuses: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>>,
            frame_branches: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, Option<BranchId>>>,
            frame_ordered_nodes: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            frame_node_dependencies: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, Vec<FlowNodeId>>>,
            frame_node_dependency_modes: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, DependencyMode>>,
            frame_id: &FrameId,
            node_id: &FlowNodeId,
            terminal_status: &NodeRunStatus,
        ) -> std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>> {
            let mut all_statuses = all_statuses.clone();
            let statuses = all_statuses.entry(frame_id.clone()).or_default();
            statuses.insert(node_id.clone(), *terminal_status);
            let ordered_nodes = frame_ordered_nodes
                .get(&frame_id)
                .cloned()
                .unwrap_or_default();
            let branches = frame_branches.get(&frame_id).cloned().unwrap_or_default();
            if *terminal_status == NodeRunStatus::Completed {
                if let Some(branch) = branches.get(&node_id).cloned().unwrap_or(None) {
                    for candidate in &ordered_nodes {
                        if candidate != node_id
                            && branches.get(candidate).cloned().unwrap_or(None) == Some(branch.clone())
                            && statuses
                                .get(candidate)
                                .copied()
                                .is_some_and(|status| {
                                    !Self::mob_machine_node_terminal(status)
                                        && status != NodeRunStatus::Running
                                })
                        {
                            statuses.insert(candidate.clone(), NodeRunStatus::Skipped);
                        }
                    }
                }
            }
            let dependencies = frame_node_dependencies
                .get(&frame_id)
                .cloned()
                .unwrap_or_default();
            let dependency_modes = frame_node_dependency_modes
                .get(&frame_id)
                .cloned()
                .unwrap_or_default();
            for candidate in ordered_nodes {
                if statuses.get(&candidate) != Some(&NodeRunStatus::Pending) {
                    continue;
                }
                let deps = dependencies.get(&candidate).cloned().unwrap_or_default();
                let dep_mode = dependency_modes
                    .get(&candidate)
                    .copied()
                    .unwrap_or(DependencyMode::All);
                let failed = !deps.is_empty()
                    && match dep_mode {
                    DependencyMode::All => deps.iter().any(|dep| {
                        statuses.get(dep).copied().is_some_and(|status| {
                            matches!(
                                status,
                                NodeRunStatus::Failed
                                    | NodeRunStatus::Skipped
                                    | NodeRunStatus::Canceled
                            )
                        })
                    }),
                    DependencyMode::Any => deps.iter().all(|dep| {
                        statuses.get(dep).copied().is_some_and(|status| {
                            matches!(
                                status,
                                NodeRunStatus::Failed
                                    | NodeRunStatus::Skipped
                                    | NodeRunStatus::Canceled
                            )
                        })
                    }),
                };
                if failed {
                    statuses.insert(candidate, NodeRunStatus::Skipped);
                    continue;
                }
                let satisfied = deps.is_empty()
                    || (dep_mode == DependencyMode::All
                        && deps
                            .iter()
                            .all(|dep| statuses.get(dep) == Some(&NodeRunStatus::Completed)))
                    || (dep_mode == DependencyMode::Any
                        && deps
                            .iter()
                            .any(|dep| statuses.get(dep) == Some(&NodeRunStatus::Completed)));
                if satisfied {
                    statuses.insert(candidate, NodeRunStatus::Ready);
                }
            }
            all_statuses
        }

        fn mob_machine_frame_ready_queue_after_terminal(
            all_ready_queues: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            all_statuses: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, NodeRunStatus>>,
            frame_ordered_nodes: &std::collections::BTreeMap<FrameId, Vec<FlowNodeId>>,
            frame_id: &FrameId,
        ) -> std::collections::BTreeMap<FrameId, Vec<FlowNodeId>> {
            let mut all_ready_queues = all_ready_queues.clone();
            let statuses = all_statuses.get(&frame_id).cloned().unwrap_or_default();
            let ordered_nodes = frame_ordered_nodes.get(&frame_id).cloned().unwrap_or_default();
            let ready = ordered_nodes
                .into_iter()
                .filter(|node_id| statuses.get(node_id) == Some(&NodeRunStatus::Ready))
                .collect();
            all_ready_queues.insert(frame_id.clone(), ready);
            all_ready_queues
        }
        }
    };
}

crate::mob_catalog_machine_dsl!("self", "catalog::dsl::mob_machine");

pub type MobToolCallerProvenance = meerkat_core::service::MobToolCallerProvenance;
pub type OpaquePrincipalToken = meerkat_core::service::OpaquePrincipalToken;

// ---------------------------------------------------------------------------
// Bridging newtypes
// ---------------------------------------------------------------------------
//
// These types bridge between the DSL's flat representation and the real mob
// domain types in `crate::ids`. The DSL needs Ord+Hash+Clone for Set/Map;
// these newtypes satisfy that while providing From/Into mappings.

/// Bridging type for agent identity. Maps to `crate::ids::AgentIdentity`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct AgentIdentity(pub String);

impl<T: Into<String>> From<T> for AgentIdentity {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Canonical peer identity for respawn topology-restore feedback. Local
/// member edges use `AgentIdentity`; external peer edges use `PeerId`, not the
/// display-only peer name.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RespawnTopologyPeerId(pub String);

impl<T: Into<String>> From<T> for RespawnTopologyPeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for agent runtime ID. Maps to `crate::ids::AgentRuntimeId`.
///
/// The real `AgentRuntimeId` is a struct `{ identity: AgentIdentity, generation: Generation }`.
/// The DSL uses a single string key `"identity:generation"` for Set/Map operations.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct AgentRuntimeId(pub String);

impl<T: Into<String>> From<T> for AgentRuntimeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for mob id. Maps to `crate::ids::MobId`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct MobId(pub String);

impl<T: Into<String>> From<T> for MobId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for fence token. Maps to `crate::ids::FenceToken`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FenceToken(pub u64);

impl From<u64> for FenceToken {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Bridging type for generation counter. Maps to `crate::ids::Generation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(pub u64);

impl From<u64> for Generation {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

/// Bridging type for work reference. Maps to `crate::ids::WorkRef`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct WorkId(pub String);

impl<T: Into<String>> From<T> for WorkId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for flow run identity. Maps to `crate::ids::RunId`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct RunId(pub String);

impl<T: Into<String>> From<T> for RunId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for frame identity. Maps to `crate::ids::FrameId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FrameId(pub String);

impl<T: Into<String>> From<T> for FrameId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}
impl FrameId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bridging type for loop instance identity. Maps to `crate::ids::LoopInstanceId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct LoopInstanceId(pub String);

impl<T: Into<String>> From<T> for LoopInstanceId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}
impl LoopInstanceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bridging type for loop definition identity. Maps to `crate::ids::LoopId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct LoopId(pub String);

impl<T: Into<String>> From<T> for LoopId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for flow-node identity. Maps to `crate::ids::FlowNodeId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FlowNodeId(pub String);

impl<T: Into<String>> From<T> for FlowNodeId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for branch identity. Maps to `crate::ids::BranchId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct BranchId(pub String);

impl<T: Into<String>> From<T> for BranchId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for step identity. Maps to `crate::ids::StepId`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct StepId(pub String);

impl<T: Into<String>> From<T> for StepId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}
impl StepId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Composite key for run-scoped step state projected into MobMachine.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RunStepKey(pub String);

impl<T: Into<String>> From<T> for RunStepKey {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Composite key for frame-scoped node state projected into MobMachine.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct FrameNodeKey(pub String);

impl<T: Into<String>> From<T> for FrameNodeKey {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for bridge session id. Maps to
/// `meerkat_core::session::SessionId` — the bridge session a mob member is
/// attached to for the current runtime generation. The DSL only needs the
/// stringified form for Ord/Hash/Clone/Default; the realtime WS observer
/// materializes it back into the typed core id.
#[derive(
    Debug,
    Clone,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SessionId(pub String);

impl<T: Into<String>> From<T> for SessionId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

impl SessionId {
    /// Project a real `meerkat_core::types::SessionId` into the DSL bridging type.
    pub fn from_domain(id: &meerkat_core::types::SessionId) -> Self {
        Self(id.to_string())
    }
}

// ---------------------------------------------------------------------------
// Projection helpers: domain types → bridging types
// ---------------------------------------------------------------------------

/// Kickoff lifecycle phase for a member's initial autonomous turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KickoffPhase {
    Pending,
    Starting,
    CallbackPending,
    Started,
    Failed,
    Cancelled,
}

/// Dependency satisfaction mode for a step or frame node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DependencyMode {
    #[default]
    All,
    Any,
}

/// Collection policy for a step's fan-out execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CollectionPolicyKind {
    #[default]
    All,
    Any,
    Quorum,
}

/// Canonical flow-run lifecycle state once run-local semantics are absorbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowRunStatus {
    #[default]
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Canonical frame lifecycle state once frame-local semantics are absorbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FrameStatus {
    #[default]
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Canonical loop lifecycle state once loop-local semantics are absorbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LoopStatus {
    #[default]
    Running,
    Completed,
    Exhausted,
    Failed,
    Canceled,
}

/// Canonical step execution status once run-local semantics are absorbed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StepRunStatus {
    #[default]
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

/// Root-vs-body frame scope for a frame snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FrameScope {
    #[default]
    Root,
    Body,
}

/// Flow node kind inside a frame DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowNodeKind {
    #[default]
    Step,
    Loop,
}

/// Per-node execution status within a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum NodeRunStatus {
    Pending,
    #[default]
    Ready,
    Running,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

/// Loop-body/evaluate lifecycle stage for an active repeat-until node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LoopIterationStage {
    #[default]
    AwaitingBodyFrame,
    BodyFrameActive,
    AwaitingUntilEvaluation,
}

/// Flow-run projection reducer command authorized by MobMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowRunReducerCommandKind {
    #[default]
    CreateRun,
    StartRun,
    DispatchStep,
    CompleteStep,
    RecordStepOutput,
    ConditionPassed,
    ConditionRejected,
    FailStep,
    SkipStep,
    ProjectFrameStepStatus,
    CancelStep,
    RegisterTargets,
    RecordTargetSuccess,
    RecordTargetTerminalFailure,
    RecordTargetCanceled,
    RecordTargetFailure,
    RegisterReadyFrame,
    PumpNodeScheduler,
    RegisterPendingBodyFrame,
    PumpFrameScheduler,
    NodeExecutionReleased,
    FrameTerminated,
    TerminalizeCompleted,
    TerminalizeFailed,
    TerminalizeCanceled,
}

/// Flow-frame projection reducer command authorized by MobMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowFrameReducerCommandKind {
    #[default]
    StartRootFrame,
    StartBodyFrame,
    AdmitNextReadyNode,
    CompleteNode,
    RecordNodeOutput,
    FailNode,
    SkipNode,
    CancelNode,
    SealFrame,
}

/// Loop-iteration projection reducer command authorized by MobMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum LoopIterationReducerCommandKind {
    #[default]
    StartLoop,
    BodyFrameStarted,
    BodyFrameCompleted,
    BodyFrameFailed,
    BodyFrameCanceled,
    UntilConditionMet,
    UntilConditionFailed,
    CancelLoop,
}

/// Per-runtime lifecycle marker tracking whether a member is actively serving
/// work or draining toward retirement. Generated SubmitWork guards consume this
/// marker so work-routing admission stays inside MobMachine authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberState {
    #[default]
    Active,
    Retiring,
}

/// Typed public rejection class for [`MobMachineInput::SubmitWork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SubmitWorkRejectReasonKind {
    #[default]
    MobNotRunning,
    MemberNotFound,
    StaleFenceToken,
    NotExternallyAddressable,
}

/// Typed public rejection class for [`MobMachineInput::CancelAllWork`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum CancelAllWorkRejectReasonKind {
    #[default]
    MobNotRunning,
    MemberNotFound,
    StaleFenceToken,
}

/// Typed public rejection class for generated agent event subscription
/// authority. The runtime shell may open a stream only when MobMachine emits
/// an authorize effect; rejection effects own the public result class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum EventSubscriptionRejectReasonKind {
    #[default]
    MemberNotFound,
    NoSessionBinding,
}

/// Typed work-origin classification for
/// [`MobMachineInput::SubmitWork`] / [`MobMachineEffect::RequestRuntimeIngress`].
/// Closed mirror of [`crate::ids::WorkOrigin`] — the DSL uses this enum as
/// guard-visible truth instead of the former `origin == "External"` /
/// `origin == "Internal"` string compares. The `Ingest` variant is only
/// valid on the receiving side of the admission seam
/// (`MeerkatMachine::Ingest` fired by the runtime control plane); mob
/// transitions never produce it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WorkOrigin {
    #[default]
    External,
    Internal,
    Ingest,
}

/// Typed runtime-mode override carried by generated spawn-policy resolution
/// handoff. The shell may observe an opaque callback result, but MobMachine
/// records this closed enum before unknown-member work may auto-spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SpawnPolicyRuntimeMode {
    #[default]
    AutonomousHost,
    TurnDriven,
}

/// Typed public result class for the respawn topology-restore follow-up.
/// The shell observes concrete peer restoration attempts, but MobMachine owns
/// whether the public respawn envelope is complete or topology-restoration
/// partial failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RespawnTopologyRestoreResultKind {
    #[default]
    Completed,
    TopologyRestoreFailed,
}

/// Fallible reverse mapping: the `Ingest` variant has no counterpart in the
/// shell-side [`crate::ids::WorkOrigin`] (which only classifies mob-submitted
/// work lanes); callers on the mob-domain side assert it away and surface a
/// domain error if the DSL ever produces it back across the seam.
/// Typed member lifecycle notice kind. Replaces the former literal-string
/// `kind` field on [`MobMachineEffect::EmitMemberLifecycleNotice`] — closed
/// set of observed member-lifecycle transitions the orchestrator emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberLifecycleKind {
    #[default]
    Spawned,
    Retiring,
    Retired,
    Reset,
    Respawned,
    Completed,
    Destroyed,
}

/// Typed durable mob lifecycle journal request. The runtime may append the
/// matching event only after a generated transition emits this local effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobLifecycleJournalKind {
    #[default]
    Completed,
    Destroying,
    DestroyStorageFinalizing,
    MemberSpawned,
    MemberRetired,
    Reset,
}

impl MemberLifecycleKind {
    /// Stable discriminant for logging / wire surfaces.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spawned => "spawned",
            Self::Retiring => "retiring",
            Self::Retired => "retired",
            Self::Reset => "reset",
            Self::Respawned => "respawned",
            Self::Completed => "completed",
            Self::Destroyed => "destroyed",
        }
    }
}

impl std::fmt::Display for MemberLifecycleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed wiring lifecycle notice kind for
/// [`MobMachineEffect::EmitWiringLifecycleNotice`]. Pair-valued (edge-keyed)
/// counterpart to [`MemberLifecycleKind`] (member-keyed). Emitted alongside
/// [`MobMachineEffect::WiringGraphChanged`] by `WireMembers`/`UnwireMembers`
/// transitions so external observers can reconstruct which identity pair
/// was wired or unwired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum WiringLifecycleKind {
    #[default]
    Wired,
    Unwired,
}

impl WiringLifecycleKind {
    /// Stable discriminant for logging / wire surfaces.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wired => "wired",
            Self::Unwired => "unwired",
        }
    }
}

impl std::fmt::Display for WiringLifecycleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed kickoff-notice intent. Replaces the former literal-string `intent`
/// field on [`MobMachineEffect::EmitKickoffLifecycleNotice`] — closed mirror
/// of [`KickoffPhase`] with an additional `Started` intent variant for the
/// `KickoffResolveStarted` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum KickoffIntent {
    #[default]
    Pending,
    Starting,
    Started,
    CallbackPending,
    Failed,
    Cancelled,
}

impl KickoffIntent {
    /// Stable discriminant for logging / wire surfaces.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Starting => "Starting",
            Self::Started => "Started",
            Self::CallbackPending => "CallbackPending",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

impl std::fmt::Display for KickoffIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Undirected wiring edge between two identities. Callers MUST normalize
/// to `(smaller, larger)` before constructing so that edge equality is
/// independent of insertion order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WiringEdge {
    pub a: AgentIdentity,
    pub b: AgentIdentity,
}

impl WiringEdge {
    /// Constructs an edge, normalizing so `a <= b`.
    pub fn new(lhs: AgentIdentity, rhs: AgentIdentity) -> Self {
        if lhs <= rhs {
            Self { a: lhs, b: rhs }
        } else {
            Self { a: rhs, b: lhs }
        }
    }
}

/// Descriptor-bearing member trust endpoint. MobMachine owns this fact when a
/// member runtime registers its comms identity, so member trust wiring can
/// authorize the exact peer descriptor that will be installed.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct MemberPeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}

impl From<&meerkat_core::comms::TrustedPeerDescriptor> for MemberPeerEndpoint {
    fn from(spec: &meerkat_core::comms::TrustedPeerDescriptor) -> Self {
        Self {
            name: PeerName(spec.name.as_str().to_owned()),
            peer_id: PeerId(spec.peer_id.to_string()),
            address: PeerAddress(spec.address.to_string()),
            signing_key: PeerSigningKey(spec.pubkey),
        }
    }
}

/// Descriptor-bearing external peer trust endpoint. Unlike `WiringEdge`, this
/// preserves the routing id, transport address, and signing key that make an
/// external trust edge authoritative.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct ExternalPeerEndpoint {
    pub name: PeerName,
    pub peer_id: PeerId,
    pub address: PeerAddress,
    pub signing_key: PeerSigningKey,
}

impl From<&meerkat_core::comms::TrustedPeerDescriptor> for ExternalPeerEndpoint {
    fn from(spec: &meerkat_core::comms::TrustedPeerDescriptor) -> Self {
        Self {
            name: PeerName(spec.name.as_str().to_owned()),
            peer_id: PeerId(spec.peer_id.to_string()),
            address: PeerAddress(spec.address.to_string()),
            signing_key: PeerSigningKey(spec.pubkey),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ExternalPeerEdge {
    pub local: AgentIdentity,
    pub endpoint: ExternalPeerEndpoint,
}

impl ExternalPeerEdge {
    pub fn new(local: AgentIdentity, endpoint: ExternalPeerEndpoint) -> Self {
        Self { local, endpoint }
    }
}

impl Default for ExternalPeerEdge {
    fn default() -> Self {
        Self {
            local: AgentIdentity(String::new()),
            endpoint: ExternalPeerEndpoint::default(),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerName(pub String);
impl<T: Into<String>> From<T> for PeerName {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct ExternalPeerKey {
    pub local: AgentIdentity,
    pub name: PeerName,
}

impl ExternalPeerKey {
    pub fn new(local: AgentIdentity, name: PeerName) -> Self {
        Self { local, name }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerId(pub String);
impl<T: Into<String>> From<T> for PeerId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerAddress(pub String);
impl<T: Into<String>> From<T> for PeerAddress {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PeerSigningKey(pub [u8; 32]);
impl From<[u8; 32]> for PeerSigningKey {
    fn from(key: [u8; 32]) -> Self {
        Self(key)
    }
}
