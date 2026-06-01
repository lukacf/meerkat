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
            flow_authority_schema_version: u64,
            run_status: Map<RunId, Enum<FlowRunStatus>>,
            run_ordered_steps: Map<RunId, Seq<StepId>>,
            run_tracked_steps: Map<RunId, Set<StepId>>,
            run_step_status: Map<RunId, Map<StepId, Option<Enum<StepRunStatus>>>>,
            run_output_recorded: Map<RunId, Map<StepId, bool>>,
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
            run_target_retry_counts: Map<RunId, Map<String, u64>>,
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
            run_last_granted_frame: Map<RunId, Option<FrameId>>,
            run_last_granted_loop: Map<RunId, Option<LoopInstanceId>>,
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
            frame_last_admitted_node: Map<FrameId, Option<FlowNodeId>>,
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
            // Mob-wide supervisor bridge authority. The runtime may mint
            // cryptographic candidates, but the current public supervisor
            // identity/epoch/protocol and pending rotation record are
            // machine facts because they drive trust, stale-authority
            // rejection, and bridge command admission.
            supervisor_authority_peer_id: Option<PeerId>,
            supervisor_authority_signing_key: Option<PeerSigningKey>,
            supervisor_authority_epoch: Option<u64>,
            supervisor_authority_protocol_version: Option<SupervisorProtocolVersion>,
            supervisor_pending_authority_peer_id: Option<PeerId>,
            supervisor_pending_authority_signing_key: Option<PeerSigningKey>,
            supervisor_pending_authority_epoch: Option<u64>,
            supervisor_pending_authority_protocol_version: Option<SupervisorProtocolVersion>,
            supervisor_pending_authority_accepted_peer_ids: Set<PeerId>,
            // Owner bridge-session lifecycle authority. The shell may observe
            // the owning session, but MobMachine owns whether the mob is
            // indexed to that session, whether owner archive should destroy
            // it, and whether it is an implicit delegation mob.
            owner_bridge_session_id: Option<SessionId>,
            owner_bridge_destroy_on_archive: bool,
            implicit_delegation_mob: bool,
            // Identity → current runtime binding. Survives within a
            // generation; respawn replaces the runtime id for the same
            // identity.
            identity_to_runtime: Map<AgentIdentity, AgentRuntimeId>,
            member_session_bindings: Map<AgentIdentity, SessionId>,
            member_profile_names: Map<AgentIdentity, String>,
            member_runtime_modes: Map<AgentIdentity, Enum<SpawnPolicyRuntimeMode>>,
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
            // Effective profile material authorized for the next member
            // session build. The shell may resolve inline/realm profiles and
            // inherited-tool overlays, but MobMachine owns the exact material
            // digest and addressability fact that `Spawn` may admit.
            spawn_profile_authority_profile_names: Map<AgentIdentity, String>,
            spawn_profile_authority_models: Map<AgentIdentity, String>,
            spawn_profile_authority_material_digests: Map<AgentIdentity, String>,
            spawn_profile_authority_tool_config_digests: Map<AgentIdentity, String>,
            spawn_profile_authority_skills_digests: Map<AgentIdentity, String>,
            spawn_profile_authority_provider_params_digests: Map<AgentIdentity, Option<String>>,
            spawn_profile_authority_output_schema_digests: Map<AgentIdentity, Option<String>>,
            spawn_profile_authority_external_addressable: Map<AgentIdentity, bool>,
            topology_epoch: u64,
            // Mob coordination board authority (folded from the former
            // standalone MobCoordinationLifecycleAuthorityMachine). MobMachine
            // owns the per-entity work-intent and resource-claim records,
            // their optimistic-concurrency revisions, affected resource sets,
            // owner-presence facts, raw expiry timestamps (NOT a pre-reduced
            // `is_expired` bool), and the monotonic coordination event cursor.
            // Every admission/status/overlap conclusion is recomputed in-DSL
            // from these maps plus raw caller facts — no trusted reducer.
            work_intent_status: Map<WorkIntentId, Enum<MobCoordinationWorkIntentStatus>>,
            work_intent_revision: Map<WorkIntentId, u64>,
            work_intent_resources: Map<WorkIntentId, Set<CoordinationResourceRef>>,
            work_intent_owner_present: Map<WorkIntentId, bool>,
            work_intent_expires_at_ms: Map<WorkIntentId, Option<u64>>,
            resource_claim_status: Map<ResourceClaimId, Enum<MobCoordinationResourceClaimStatus>>,
            resource_claim_kind: Map<ResourceClaimId, Enum<MobCoordinationResourceClaimKind>>,
            resource_claim_revision: Map<ResourceClaimId, u64>,
            resource_claim_resources: Map<ResourceClaimId, Set<CoordinationResourceRef>>,
            resource_claim_owner_present: Map<ResourceClaimId, bool>,
            resource_claim_expires_at_ms: Map<ResourceClaimId, Option<u64>>,
            coordination_event_next_sequence: u64,
        }

        init(Running) {
            destroy_admitted = false,
            live_runtime_ids = EmptySet,
            externally_addressable_runtime_ids = EmptySet,
            runtime_fence_tokens = EmptyMap,
            identity_runtime_generations = EmptyMap,
            identity_runtime_fence_tokens = EmptyMap,
            active_run_count = 0,
            flow_authority_schema_version = 7,
            run_status = EmptyMap,
            run_ordered_steps = EmptyMap,
            run_tracked_steps = EmptyMap,
            run_step_status = EmptyMap,
            run_output_recorded = EmptyMap,
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
            run_target_retry_counts = EmptyMap,
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
            supervisor_authority_peer_id = None,
            supervisor_authority_signing_key = None,
            supervisor_authority_epoch = None,
            supervisor_authority_protocol_version = None,
            supervisor_pending_authority_peer_id = None,
            supervisor_pending_authority_signing_key = None,
            supervisor_pending_authority_epoch = None,
            supervisor_pending_authority_protocol_version = None,
            supervisor_pending_authority_accepted_peer_ids = EmptySet,
            owner_bridge_session_id = None,
            owner_bridge_destroy_on_archive = false,
            implicit_delegation_mob = false,
            identity_to_runtime = EmptyMap,
            member_session_bindings = EmptyMap,
            member_profile_names = EmptyMap,
            member_runtime_modes = EmptyMap,
            member_peer_ids = EmptyMap,
            member_peer_endpoints = EmptyMap,
            pending_session_ingress_detach_runtime_ids = EmptySet,
            spawn_policy_enabled = false,
            spawn_policy_revision = 0,
            spawn_policy_resolution_revision = EmptyMap,
            spawn_policy_resolution_profiles = EmptyMap,
            spawn_policy_resolution_runtime_modes = EmptyMap,
            spawn_policy_resolution_absent = EmptySet,
            spawn_profile_authority_profile_names = EmptyMap,
            spawn_profile_authority_models = EmptyMap,
            spawn_profile_authority_material_digests = EmptyMap,
            spawn_profile_authority_tool_config_digests = EmptyMap,
            spawn_profile_authority_skills_digests = EmptyMap,
            spawn_profile_authority_provider_params_digests = EmptyMap,
            spawn_profile_authority_output_schema_digests = EmptyMap,
            spawn_profile_authority_external_addressable = EmptyMap,
            topology_epoch = 0,
            work_intent_status = EmptyMap,
            work_intent_revision = EmptyMap,
            work_intent_resources = EmptyMap,
            work_intent_owner_present = EmptyMap,
            work_intent_expires_at_ms = EmptyMap,
            resource_claim_status = EmptyMap,
            resource_claim_kind = EmptyMap,
            resource_claim_revision = EmptyMap,
            resource_claim_resources = EmptyMap,
            resource_claim_owner_present = EmptyMap,
            resource_claim_expires_at_ms = EmptyMap,
            coordination_event_next_sequence = 1,
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
                step_status: Map<StepId, Option<Enum<StepRunStatus>>>,
                output_recorded: Map<StepId, bool>,
                step_condition_results: Map<StepId, Option<bool>>,
                step_has_conditions: Map<StepId, bool>,
                step_dependencies: Map<StepId, Seq<StepId>>,
                step_dependency_modes: Map<StepId, Enum<DependencyMode>>,
                step_branches: Map<StepId, Option<BranchId>>,
                step_collection_policies: Map<StepId, Enum<CollectionPolicyKind>>,
                step_quorum_thresholds: Map<StepId, u32>,
                step_target_counts: Map<StepId, u64>,
                step_target_success_counts: Map<StepId, u64>,
                step_target_terminal_failure_counts: Map<StepId, u64>,
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
                step_status: Map<StepId, Option<Enum<StepRunStatus>>>,
                output_recorded: Map<StepId, bool>,
                step_condition_results: Map<StepId, Option<bool>>,
                step_has_conditions: Map<StepId, bool>,
                step_dependencies: Map<StepId, Seq<StepId>>,
                step_dependency_modes: Map<StepId, Enum<DependencyMode>>,
                step_branches: Map<StepId, Option<BranchId>>,
                step_collection_policies: Map<StepId, Enum<CollectionPolicyKind>>,
                step_quorum_thresholds: Map<StepId, u32>,
                step_target_counts: Map<StepId, u64>,
                step_target_success_counts: Map<StepId, u64>,
                step_target_terminal_failure_counts: Map<StepId, u64>,
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
                output_recorded: Map<FlowNodeId, bool>,
                node_condition_results: Map<FlowNodeId, Option<bool>>,
                last_admitted_node: Option<FlowNodeId>,
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
            ClassifyFlowRunTerminality { run_id: RunId, status: Enum<FlowRunStatus> },
            ClassifyFlowStepTerminality { run_id: RunId, step_id: StepId, status: Enum<StepRunStatus> },
            ClassifyFlowFrameTerminalStatus { frame_id: FrameId },
            ClassifyFlowRunPublicResult { run_id: RunId, status: Enum<FlowRunStatus> },
            Spawn { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: bool, runtime_mode: Enum<SpawnPolicyRuntimeMode>, bridge_session_id: Option<SessionId>, replacing: Option<SessionId> },
            AuthorizeSpawnProfile { agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: bool },
            ClassifySpawnManyFailure { observation: Enum<MobSpawnManyFailureObservationKind> },
            ClassifyMemberWait { agent_identity: AgentIdentity },
            // Flow topology edge admission. The shell extracts the pure
            // rule-match verdict from the declarative `TopologyRules`
            // (`evaluate_topology`) and the configured enforcement mode, then
            // feeds both here as a witness. The MobMachine — not the shell —
            // decides the admission verdict (`Admitted` / `DeniedStrict` /
            // `DeniedAdvisory`) and the shell mirrors that verdict. The roles
            // are echoed so the shell can reconstruct the `TopologyViolation`
            // error / advisory notice from the machine's emitted verdict.
            ResolveFlowDelegationEdgeAdmission { from_role: String, to_role: String, rule_verdict: Enum<MobFlowDelegationEdgeRuleVerdictKind>, mode: Enum<MobFlowDelegationEdgeModeKind> },
            // Remote-member runtime observation terminality. The bridge consumer
            // observes a remote member's runtime state (a pure wire projection)
            // during respawn/destroy cleanup; MobMachine — not the bridge shell
            // — owns whether that observed state is terminal. The shell mirrors
            // the emitted verdict to decide whether cleanup may stop or must
            // force-destroy.
            ClassifyRemoteMemberRuntimeObservation { observed_state: Enum<MobRemoteMemberRuntimeObservedState> },
            // Composite spawn-member operator admission. The tool shell extracts
            // three raw observations: whether the operator holds manage scope
            // over the target mob, whether the operator holds spawn-profile scope
            // for the requested profile (both machine-owned operator-scope
            // projections), and whether the request carries privileged args
            // (a pure typed argument observation). MobMachine — not the tool
            // shell — composes these into the Allow/Deny admission verdict, which
            // the shell mirrors (Deny -> access_denied).
            ResolveSpawnMemberAdmission { manage_scope_present: bool, profile_scope_present: bool, privileged_args_present: bool },
            // Per-mob operator admission for current-mob-scoped tools. The tool
            // shell extracts a single pure observation — whether the operator
            // holds manage scope over the current mob (a machine-owned
            // operator-scope projection) — and feeds it here. MobMachine — not
            // the tool shell — decides the Allow/Deny admission verdict, which
            // the shell mirrors (Deny -> access_denied).
            ResolveCurrentMobAdmission { can_manage_mob: bool },
            // Coarse spawn-tool admission for the spawn-member tool surfaces
            // (`spawn_member` / `spawn_many_members`). The tool shell extracts a
            // single pure observation — whether the operator can spawn ANY
            // profile in the current mob (a machine-owned operator-scope
            // set-non-empty projection) — and feeds it here. MobMachine — not
            // the tool shell — decides the Allow/Deny admission verdict, which
            // the shell mirrors (Deny -> access_denied). This coarse gate has
            // unique coverage over the empty-specs `spawn_many_members` case
            // (zero per-member iterations fire no per-member admission), so it
            // must be machine-routed rather than reduced in the shell.
            ResolveSpawnToolAdmission { can_spawn_any_profile: bool },
            // Operator create-mob admission for the mob-creation tool surface.
            // The tool shell extracts a single pure observation — whether the
            // operator holds the create-mobs capability bit (a machine-minted
            // operator-authority projection) — and feeds it here. MobMachine —
            // not the tool shell — decides the Allow/Deny admission verdict,
            // which the shell mirrors (Deny -> access_denied).
            ResolveCreateMobAdmission { can_create_mobs: bool },
            // Operator profile-mutation admission for realm-profile mutation
            // tools. The tool shell extracts a single pure observation —
            // whether the operator holds the mutate-profiles capability bit (a
            // machine-minted operator-authority projection) — and feeds it
            // here. MobMachine — not the tool shell — decides the Allow/Deny
            // admission verdict, which the shell mirrors (Deny ->
            // access_denied).
            ResolveProfileMutationAdmission { can_mutate_profiles: bool },
            // Within-mob member-operation eligibility classification. Spawn
            // finalization, peer messaging, and respawn finalization require the
            // mob to be live and running. MobMachine — not a handwritten shell
            // phase pre-check off `self.state()` — owns that eligibility over
            // its own lifecycle phase plus the `destroy_admitted` marker. The
            // owning actor extracts no fact (the verdict is read entirely from
            // machine-owned state); it drives this input and mirrors
            // `DeniedNotRunning` to the same `InvalidTransition` rejection it
            // previously produced. Pure classification across all phases.
            ClassifyMemberOperationEligibility {},
            // Bridge-rejection recovery classification. When a mob sends a
            // bridge command that requires an already-bound supervisor (e.g.
            // `AuthorizeSupervisor`) and the member replies with a typed
            // rejection cause (a pure wire projection), MobMachine — not a
            // handwritten shell helper — owns whether that cause is recoverable
            // by re-running `BindMember` (`RebindRecover`) or must bubble up as
            // fatal (`FatalBubbleUp`). The machine-owning caller (actor /
            // resume builder) mirrors the emitted recovery verdict to gate the
            // real rebind control flow.
            ClassifyBridgeRejectionRecovery { rejection_cause: Enum<MobBridgeRejectionCause> },
            // Pending-supervisor-acceptance classification. When a supervisor
            // rotation has already-accepted remote peers and the actor
            // re-verifies a pending peer's acceptance by replaying the
            // `AuthorizeSupervisor` command under the pending authority, the
            // member may reply with a typed wire rejection cause (a pure wire
            // projection). MobMachine — not a handwritten shell reducer — owns
            // whether that cause means the peer is NOT confirmed and must be
            // re-attempted (`NotConfirmedReattempt`: the peer rebound to current
            // authority), the pending authority is stale (`StalePendingAuthority`),
            // or it is a hard fatal rejection (`Fatal`). The actor mirrors the
            // emitted verdict to gate re-attempt-vs-error control flow.
            ClassifyPendingSupervisorAcceptance { rejection_cause: Enum<MobBridgeRejectionCause> },
            EnsureMember { agent_identity: AgentIdentity },
            Reconcile { desired: Set<AgentIdentity>, retire_stale: bool },
            Retire { mob_id: MobId, agent_runtime_id: AgentRuntimeId, agent_identity: AgentIdentity, generation: Generation, releasing: Option<SessionId>, session_id: Option<SessionId> },
            RetireAbsent { agent_identity: AgentIdentity },
            RequestPendingSessionIngressDetachForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            Respawn { agent_runtime_id: AgentRuntimeId },
            RetireAll,
            BindOwnerBridgeSession { bridge_session_id: SessionId, destroy_on_owner_archive: bool, implicit_delegation_mob: bool },
            // Track-B (R5): explicit identity-level wiring and session-binding
            // mutation inputs. These drive `wiring_edges` and
            // `member_session_bindings` directly at DSL authority,
            // independent of the Spawn/Retire lifecycle.
            //
            // The `edge` field on `WireMembers`/`UnwireMembers` carries a
            // pre-normalized `WiringEdge` (a <= b). Callers construct the
            // edge via `WiringEdge::new(a, b)` before submitting.
            WireMembers { edge: WiringEdge },
            WireMembersWithTrust { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            UnwireMembers { edge: WiringEdge },
            WireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RegisterMemberPeer { agent_identity: AgentIdentity, peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberPeerRebind { agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberPeerOverlay { agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberTrustWiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustUnwiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanup { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanupObserved { edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId },
            AuthorizeExternalPeerReciprocalTrust { key: ExternalPeerKey, agent_identity: AgentIdentity },
            UnwireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            ProvisionSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion },
            ClearSupervisorPendingRotation { current_peer_id: PeerId, current_epoch: u64, protocol_version: SupervisorProtocolVersion },
            RecordSupervisorPendingRotation { current_peer_id: PeerId, current_epoch: u64, pending_peer_id: PeerId, pending_signing_key: PeerSigningKey, pending_epoch: u64, protocol_version: SupervisorProtocolVersion, accepted_peer_ids: Set<PeerId> },
            CommitSupervisorRotation { current_peer_id: PeerId, current_epoch: u64, next_peer_id: PeerId, next_signing_key: PeerSigningKey, next_epoch: u64, protocol_version: SupervisorProtocolVersion },
            ClearSupervisorAuthorityForDestroy { current_peer_id: PeerId, current_signing_key: PeerSigningKey, current_epoch: u64, protocol_version: SupervisorProtocolVersion },
            RestoreSupervisorAuthorityAfterDestroyRollback { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId> },
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
            // ---------------------------------------------------------------
            // Mob coordination board inputs (folded). These carry only RAW
            // typed caller facts: no `already_exists`, no `current_revision`
            // as a trusted input, no `owning_mob_ref_matches`, no `is_expired`,
            // no `current_next_sequence`, and no pre-decided `overlap_ids`.
            // MobMachine recomputes/revalidates every conclusion in-DSL.
            // ---------------------------------------------------------------
            RecordCoordinationWorkIntent {
                intent_id: WorkIntentId,
                requested_status: Enum<MobCoordinationWorkIntentStatus>,
                owner_present: bool,
                summary_present: bool,
                metadata_public: bool,
                draft_mob_id: MobId,
                authority_mob_id: MobId,
                resource_tokens: Set<CoordinationResourceRef>,
                expires_at_ms: Option<u64>,
            },
            RecordCoordinationResourceClaim {
                claim_id: ResourceClaimId,
                requested_kind: Enum<MobCoordinationResourceClaimKind>,
                requested_status: Enum<MobCoordinationResourceClaimStatus>,
                owner_present: bool,
                metadata_public: bool,
                draft_mob_id: MobId,
                authority_mob_id: MobId,
                resource_tokens: Set<CoordinationResourceRef>,
                expires_at_ms: Option<u64>,
            },
            UpdateCoordinationWorkIntentStatus {
                intent_id: WorkIntentId,
                expected_revision: u64,
                requested_status: Enum<MobCoordinationWorkIntentStatus>,
                now_ms: u64,
            },
            UpdateCoordinationResourceClaimStatus {
                claim_id: ResourceClaimId,
                expected_revision: u64,
                requested_status: Enum<MobCoordinationResourceClaimStatus>,
                now_ms: u64,
            },
            ObserveCoordinationResourceClaimOverlap {
                claim_id: ResourceClaimId,
                now_ms: u64,
                candidate_overlap_ids: Seq<ResourceClaimId>,
            },
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
            RetireMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: Option<SessionId> },
            AdmitDestroyMemberRetire { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: Option<SessionId> },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveDestroyMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId> },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool, session_id: SessionId },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool, session_id: SessionId },
            ResolveRespawnTopologyRestore { agent_identity: AgentIdentity, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            DestroyMob { session_id: SessionId },
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RecoverRosterMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool },
            RecoverMemberSessionBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, replacing: Option<SessionId> },
            RecoverRosterMemberReset { agent_identity: AgentIdentity, previous_agent_runtime_id: AgentRuntimeId, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecoverRosterMemberRetired { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId },
            RecoverMemberKickoff { member_id: String, phase: KickoffPhase, error: Option<String> },
            RecoverRosterWiring { edge: WiringEdge },
            RecoverRosterUnwire { edge: WiringEdge },
            RecoverExternalPeerWiring { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RecoverExternalPeerUnwire { key: ExternalPeerKey },
            RecoverSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId> },
            RecoverOwnerBridgeSession { bridge_session_id: SessionId, destroy_on_owner_archive: bool, implicit_delegation_mob: bool },
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
            RequestRuntimeBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, session_id: SessionId },
            SpawnProfileAuthorized { agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: bool },
            RequestRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, session_id: SessionId, work_id: WorkId, origin: Enum<WorkOrigin> },
            RequestPeerRuntimeIngress { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Option<Generation>, work_id: WorkId, origin: Enum<WorkOrigin> },
            SubmitWorkRejected { agent_runtime_id: AgentRuntimeId, origin: Enum<WorkOrigin>, reason: Enum<SubmitWorkRejectReasonKind>, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken> },
            CancelAllWorkRejected { agent_runtime_id: AgentRuntimeId, reason: Enum<CancelAllWorkRejectReasonKind>, expected_fence_token: Option<FenceToken>, actual_fence_token: Option<FenceToken> },
            RequestRuntimeRetire { session_id: SessionId },
            RequestRuntimeDestroy { session_id: SessionId },
            PendingSpawnOperationOwnerAuthorized { agent_identity: AgentIdentity, session_id: SessionId },
            RequestSessionIngressDetachForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            AppendLifecycleJournal { kind: Enum<MobLifecycleJournalKind>, agent_identity: Option<AgentIdentity>, agent_runtime_id: Option<AgentRuntimeId>, fence_token: Option<FenceToken>, generation: Option<Generation>, session_id: Option<SessionId> },
            AppendOperatorActionProvenance { tool_name: String, principal_token: OpaquePrincipalToken, caller_provenance: Option<MobToolCallerProvenance>, audit_invocation_id: Option<String> },
            EmitMemberLifecycleNotice { kind: Enum<MemberLifecycleKind> },
            EmitRunLifecycleNotice,
            EmitFlowRunNotice,
            AppendFailureLedger,
            FlowTerminalized,
            FlowRunTerminal { run_id: RunId },
            FlowRunNonTerminal { run_id: RunId },
            FlowStepTerminal { run_id: RunId, step_id: StepId },
            FlowStepNonTerminal { run_id: RunId, step_id: StepId },
            FlowFrameTerminalStatusClassified { frame_id: FrameId, terminal_status: Enum<FrameStatus> },
            FlowFrameTerminalStatusUnavailable { frame_id: FrameId },
            FlowRunPublicResultClassified { run_id: RunId, result: Enum<FlowRunPublicResultClassKind> },
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
            OwnerBridgeSessionBound { bridge_session_id: SessionId, destroy_on_owner_archive: bool, implicit_delegation_mob: bool },
            RespawnTopologyRestoreResolved { agent_identity: AgentIdentity, result: Enum<RespawnTopologyRestoreResultKind>, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            SpawnManyFailureClassified { observation: Enum<MobSpawnManyFailureObservationKind>, cause: Enum<MobSpawnManyFailureCauseKind> },
            MemberWaitClassified { agent_identity: AgentIdentity, result: Enum<MemberWaitClassificationKind> },
            // Machine-owned flow topology edge admission verdict. The shell
            // mirrors this (DeniedStrict -> TopologyViolation block;
            // DeniedAdvisory -> advisory notice; Admitted -> proceed) instead
            // of computing+enforcing the admission itself.
            FlowDelegationEdgeAdmissionResolved { from_role: String, to_role: String, admission: Enum<MobFlowDelegationEdgeAdmissionKind> },
            // Machine-owned terminality verdict for an observed remote-member
            // runtime state. The bridge shell mirrors this to decide whether
            // confirmatory cleanup may stop (Terminal) or must force-destroy
            // (NonTerminal).
            RemoteMemberRuntimeTerminalityClassified { observed_state: Enum<MobRemoteMemberRuntimeObservedState>, terminality: Enum<MobRemoteMemberRuntimeTerminality> },
            // Machine-owned composite spawn-member operator admission verdict.
            // The tool shell mirrors this (Denied -> access_denied; Allowed ->
            // proceed) instead of composing+enforcing the admission itself.
            SpawnMemberAdmissionResolved { admission: Enum<MobSpawnMemberAdmissionKind> },
            // Machine-owned per-mob operator admission verdict for current-mob
            // tools. The tool shell mirrors this (Denied -> access_denied;
            // Allowed -> proceed) instead of composing the admission itself.
            CurrentMobAdmissionResolved { admission: Enum<MobCurrentMobAdmissionKind> },
            // Machine-owned coarse spawn-tool admission verdict for the
            // spawn-member tool surfaces. The tool shell mirrors this (Denied ->
            // access_denied; Allowed -> proceed) instead of composing the
            // admission itself.
            SpawnToolAdmissionResolved { admission: Enum<MobSpawnToolAdmissionKind> },
            // Machine-owned operator create-mob admission verdict. The tool
            // shell mirrors this (Denied -> access_denied; Allowed -> proceed)
            // instead of composing the admission itself.
            CreateMobAdmissionResolved { admission: Enum<MobCreateMobAdmissionKind> },
            // Machine-owned operator profile-mutation admission verdict. The
            // tool shell mirrors this (Denied -> access_denied; Allowed ->
            // proceed) instead of composing the admission itself.
            ProfileMutationAdmissionResolved { admission: Enum<MobProfileMutationAdmissionKind> },
            MemberOperationEligibilityResolved { admission: Enum<MobMemberOperationEligibilityKind> },
            // Machine-owned bridge-rejection recovery verdict. The mob shell
            // mirrors this (RebindRecover -> re-run BindMember; FatalBubbleUp ->
            // bubble the rejection up) instead of reducing the raw wire cause
            // into a recoverable-vs-fatal conclusion itself.
            BridgeRejectionRecoveryClassified { rejection_cause: Enum<MobBridgeRejectionCause>, recovery: Enum<MobBridgeRejectionRecovery> },
            // Machine-owned pending-supervisor-acceptance verdict. The actor
            // mirrors this (NotConfirmedReattempt -> drop the accepted peer and
            // re-attempt; StalePendingAuthority -> error with the stale-pending
            // message; Fatal -> bubble the rejection) instead of reducing the
            // raw wire cause into an acceptance/recovery conclusion itself.
            PendingSupervisorAcceptanceClassified { rejection_cause: Enum<MobBridgeRejectionCause>, verdict: Enum<MobPendingSupervisorAcceptanceKind> },
            // Machine-owned frame-seed disposition. `CreateFrameSeed` is
            // idempotent: a fresh seed emits `Seeded`, while re-seeding an
            // already-tracked frame emits `AlreadySeeded` (a no-op) instead of
            // a guard rejection the shell would have to reinterpret. The flow
            // shell mirrors both as success — `AlreadySeeded` short-circuits
            // (idempotent re-confirm), `Seeded` proceeds to snapshot validation.
            FrameSeedConfirmed { frame_id: FrameId, disposition: Enum<MobFrameSeedDisposition> },
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
            SessionProvisionOperationOwnerAuthorized { agent_identity: AgentIdentity, session_id: SessionId },
            MemberTrustWiringRequested { edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, a_endpoint: MemberPeerEndpoint, b_endpoint: MemberPeerEndpoint, epoch: u64 },
            MemberTrustUnwiringRequested { edge: WiringEdge, a_peer_id: PeerId, b_peer_id: PeerId, epoch: u64 },
            WiringTrustRepairRequested { edge: WiringEdge },
            ExternalPeerTrustWiringRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            ExternalPeerTrustUnwiringRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            ExternalPeerTrustRepairRequested { edge: ExternalPeerEdge, local_peer_id: PeerId, peer_id: PeerId, epoch: u64 },
            MemberPeerRegistered { agent_identity: AgentIdentity, peer_id: PeerId },
            MemberPeerRebindAuthorized { agent_identity: AgentIdentity, peer_id: PeerId, peer_endpoint: MemberPeerEndpoint },
            MemberPeerOverlayAuthorized { agent_identity: AgentIdentity, peer_id: PeerId, peer_overlay_endpoints: Set<MemberPeerEndpoint>, epoch: u64 },
            ExternalPeerReciprocalTrustRequested { key: ExternalPeerKey, edge: ExternalPeerEdge, peer_id: PeerId, peer_endpoint: MemberPeerEndpoint, epoch: u64 },
            PersistSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId> },
            DeleteSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion },
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
            // Mob coordination board effects (folded). Each carries the
            // machine-computed revision / event sequence.
            WorkIntentRecorded {
                intent_id: WorkIntentId,
                status: Enum<MobCoordinationWorkIntentStatus>,
                revision: u64,
                resource_tokens: Set<CoordinationResourceRef>,
                expires_at_ms: Option<u64>,
                event_kind: Enum<MobCoordinationEventKind>,
                sequence: u64,
            },
            ResourceClaimRecorded {
                claim_id: ResourceClaimId,
                kind: Enum<MobCoordinationResourceClaimKind>,
                status: Enum<MobCoordinationResourceClaimStatus>,
                revision: u64,
                resource_tokens: Set<CoordinationResourceRef>,
                expires_at_ms: Option<u64>,
                event_kind: Enum<MobCoordinationEventKind>,
                sequence: u64,
            },
            WorkIntentStatusChanged {
                intent_id: WorkIntentId,
                status: Enum<MobCoordinationWorkIntentStatus>,
                revision: u64,
                event_kind: Enum<MobCoordinationEventKind>,
                sequence: u64,
            },
            ResourceClaimStatusChanged {
                claim_id: ResourceClaimId,
                status: Enum<MobCoordinationResourceClaimStatus>,
                revision: u64,
                event_kind: Enum<MobCoordinationEventKind>,
                sequence: u64,
            },
            ResourceClaimOverlapObserved {
                claim_id: ResourceClaimId,
                overlap_ids: Seq<ResourceClaimId>,
                event_kind: Enum<MobCoordinationEventKind>,
                sequence: u64,
            },
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine],
        disposition SpawnProfileAuthorized => local,
        disposition RequestRuntimeIngress => routed [MeerkatMachine],
        disposition RequestPeerRuntimeIngress => local,
        disposition SubmitWorkRejected => local,
        disposition CancelAllWorkRejected => local,
        disposition RequestRuntimeRetire => routed [MeerkatMachine],
        disposition RequestRuntimeDestroy => routed [MeerkatMachine],
        disposition PendingSpawnOperationOwnerAuthorized => local,
        disposition RequestSessionIngressDetachForMobDestroy => external handoff mob_destroying_session_ingress,
        disposition AppendLifecycleJournal => local,
        disposition AppendOperatorActionProvenance => local,
        disposition EmitMemberLifecycleNotice => external,
        disposition EmitRunLifecycleNotice => external,
        disposition EmitFlowRunNotice => external,
        disposition AppendFailureLedger => local,
        disposition FlowTerminalized => external,
        disposition FlowRunTerminal => local,
        disposition FlowRunNonTerminal => local,
        disposition FlowStepTerminal => local,
        disposition FlowStepNonTerminal => local,
        disposition FlowFrameTerminalStatusClassified => local,
        disposition FlowFrameTerminalStatusUnavailable => local,
        disposition FlowRunPublicResultClassified => local,
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
        disposition OwnerBridgeSessionBound => local,
        disposition RespawnTopologyRestoreResolved => local,
        disposition SpawnManyFailureClassified => local,
        disposition MemberWaitClassified => local,
        disposition FlowDelegationEdgeAdmissionResolved => local,
        disposition RemoteMemberRuntimeTerminalityClassified => local,
        disposition SpawnMemberAdmissionResolved => local,
        disposition CurrentMobAdmissionResolved => local,
        disposition SpawnToolAdmissionResolved => local,
        disposition CreateMobAdmissionResolved => local,
        disposition ProfileMutationAdmissionResolved => local,
        disposition MemberOperationEligibilityResolved => local,
        disposition BridgeRejectionRecoveryClassified => local,
        disposition PendingSupervisorAcceptanceClassified => local,
        disposition FrameSeedConfirmed => local,
        disposition WiringGraphChanged => external,
        disposition MemberSessionBindingChanged => external,
        disposition SessionProvisionOperationOwnerAuthorized => local,
        disposition MemberTrustWiringRequested => external handoff mob_member_trust_wiring,
        disposition MemberTrustUnwiringRequested => external handoff mob_member_trust_unwiring,
        disposition WiringTrustRepairRequested => local,
        disposition ExternalPeerTrustWiringRequested => external handoff mob_external_peer_trust_wiring,
        disposition ExternalPeerTrustUnwiringRequested => external handoff mob_external_peer_trust_unwiring,
        disposition ExternalPeerTrustRepairRequested => external handoff mob_external_peer_trust_repair,
        disposition MemberPeerRegistered => local,
        disposition MemberPeerRebindAuthorized => local,
        disposition MemberPeerOverlayAuthorized => external handoff mob_member_peer_overlay,
        disposition ExternalPeerReciprocalTrustRequested => external handoff mob_external_peer_reciprocal_trust,
        disposition PersistSupervisorAuthority => local,
        disposition DeleteSupervisorAuthority => local,
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
        disposition WorkIntentRecorded => local,
        disposition ResourceClaimRecorded => local,
        disposition WorkIntentStatusChanged => local,
        disposition ResourceClaimStatusChanged => local,
        disposition ResourceClaimOverlapObserved => local,

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

        invariant member_spawn_material_matches_runtime_binding {
            for_all(id in self.identity_to_runtime.keys(), self.member_profile_names.contains_key(id))
            && for_all(id in self.identity_to_runtime.keys(), self.member_runtime_modes.contains_key(id))
            && for_all(id in self.member_profile_names.keys(), self.identity_to_runtime.contains_key(id))
            && for_all(id in self.member_runtime_modes.keys(), self.identity_to_runtime.contains_key(id))
        }

        invariant external_peer_edges_are_keyed_coherently {
            for_all(key in self.external_peer_edges_by_key.keys(),
                mob_machine_external_peer_key_matches_edge(key, self.external_peer_edges_by_key.get_cloned(key).get("value"))
                && self.external_peer_edges.contains(self.external_peer_edges_by_key.get_cloned(key).get("value")))
            && for_all(edge in self.external_peer_edges,
                mob_machine_external_peer_edge_has_matching_key(self.external_peer_edges_by_key, edge))
        }

        invariant supervisor_authority_tuple_consistent {
            (
                self.supervisor_authority_peer_id == None
                && self.supervisor_authority_signing_key == None
                && self.supervisor_authority_epoch == None
                && self.supervisor_authority_protocol_version == None
            )
            || (
                self.supervisor_authority_peer_id != None
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch != None
                && self.supervisor_authority_protocol_version != None
            )
        }

        invariant supervisor_pending_authority_tuple_consistent {
            (
                self.supervisor_pending_authority_peer_id == None
                && self.supervisor_pending_authority_signing_key == None
                && self.supervisor_pending_authority_epoch == None
                && self.supervisor_pending_authority_protocol_version == None
                && self.supervisor_pending_authority_accepted_peer_ids == EmptySet
            )
            || (
                self.supervisor_pending_authority_peer_id != None
                && self.supervisor_pending_authority_signing_key != None
                && self.supervisor_pending_authority_epoch != None
                && self.supervisor_pending_authority_protocol_version != None
            )
        }

        invariant supervisor_pending_authority_requires_current {
            self.supervisor_pending_authority_peer_id == None
            || self.supervisor_authority_peer_id != None
        }

        invariant owner_bridge_cleanup_requires_owner {
            self.owner_bridge_destroy_on_archive == false
            || self.owner_bridge_session_id != None
        }

        invariant implicit_delegation_requires_owner {
            self.implicit_delegation_mob == false
            || self.owner_bridge_session_id != None
        }

        invariant implicit_delegation_requires_cleanup {
            self.implicit_delegation_mob == false
            || self.owner_bridge_destroy_on_archive == true
        }

        // =====================================================================
        // Direct transitions
        // =====================================================================

        transition ClassifyFlowRunTerminalityTerminal {
            per_phase [Running]
            on input ClassifyFlowRunTerminality { run_id, status }
            guard "run_status_terminal" {
                status == FlowRunStatus::Completed
                || status == FlowRunStatus::Failed
                || status == FlowRunStatus::Canceled
            }
            update {}
            to Running
            emit FlowRunTerminal { run_id: run_id }
        }

        transition ClassifyFlowRunTerminalityNonTerminal {
            per_phase [Running]
            on input ClassifyFlowRunTerminality { run_id, status }
            guard "run_status_non_terminal" {
                status == FlowRunStatus::Absent
                || status == FlowRunStatus::Pending
                || status == FlowRunStatus::Running
            }
            update {}
            to Running
            emit FlowRunNonTerminal { run_id: run_id }
        }

        transition ClassifyFlowStepTerminalityTerminal {
            per_phase [Running]
            on input ClassifyFlowStepTerminality { run_id, step_id, status }
            guard "step_status_terminal" {
                status == StepRunStatus::Completed
                || status == StepRunStatus::Failed
                || status == StepRunStatus::Skipped
                || status == StepRunStatus::Canceled
            }
            update {}
            to Running
            emit FlowStepTerminal { run_id: run_id, step_id: step_id }
        }

        transition ClassifyFlowStepTerminalityNonTerminal {
            per_phase [Running]
            on input ClassifyFlowStepTerminality { run_id, step_id, status }
            guard "step_status_non_terminal" { status == StepRunStatus::Dispatched }
            update {}
            to Running
            emit FlowStepNonTerminal { run_id: run_id, step_id: step_id }
        }

        transition ClassifyFlowFrameTerminalStatusFailed {
            per_phase [Running, Stopped, Completed]
            on input ClassifyFlowFrameTerminalStatus { frame_id }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_tracked_nodes_known" { self.frame_tracked_nodes.contains_key(frame_id) == true }
            guard "frame_node_status_known" { self.frame_node_status.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "all_nodes_terminal" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Completed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Failed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Skipped)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Canceled))
            }
            guard "any_node_failed" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed)) == false
            }
            update {}
            to Running
            emit FlowFrameTerminalStatusClassified {
                frame_id: frame_id,
                terminal_status: FrameStatus::Failed
            }
        }

        transition ClassifyFlowFrameTerminalStatusCanceled {
            per_phase [Running, Stopped, Completed]
            on input ClassifyFlowFrameTerminalStatus { frame_id }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_tracked_nodes_known" { self.frame_tracked_nodes.contains_key(frame_id) == true }
            guard "frame_node_status_known" { self.frame_node_status.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "all_nodes_terminal" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Completed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Failed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Skipped)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Canceled))
            }
            guard "no_node_failed" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed))
            }
            guard "any_node_canceled" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Canceled)) == false
            }
            update {}
            to Running
            emit FlowFrameTerminalStatusClassified {
                frame_id: frame_id,
                terminal_status: FrameStatus::Canceled
            }
        }

        transition ClassifyFlowFrameTerminalStatusCompleted {
            per_phase [Running, Stopped, Completed]
            on input ClassifyFlowFrameTerminalStatus { frame_id }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_tracked_nodes_known" { self.frame_tracked_nodes.contains_key(frame_id) == true }
            guard "frame_node_status_known" { self.frame_node_status.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "all_nodes_terminal" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Completed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Failed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Skipped)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Canceled))
            }
            guard "no_node_failed" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Failed))
            }
            guard "no_node_canceled" {
                for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) != Some(NodeRunStatus::Canceled))
            }
            update {}
            to Running
            emit FlowFrameTerminalStatusClassified {
                frame_id: frame_id,
                terminal_status: FrameStatus::Completed
            }
        }

        transition ClassifyFlowFrameTerminalStatusUnavailable {
            per_phase [Running, Stopped, Completed]
            on input ClassifyFlowFrameTerminalStatus { frame_id }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_tracked_nodes_known" { self.frame_tracked_nodes.contains_key(frame_id) == true }
            guard "frame_node_status_known" { self.frame_node_status.contains_key(frame_id) == true }
            guard "terminal_status_unavailable" {
                self.frame_phase.get_cloned(frame_id) != Some(FrameStatus::Running)
                || for_all(seal_node_id in self.frame_tracked_nodes.get_cloned(frame_id).get("value"),
                    self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Completed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Failed)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Skipped)
                    || self.frame_node_status.get_cloned(frame_id).get("value").get_cloned(seal_node_id) == Some(NodeRunStatus::Canceled)) == false
            }
            update {}
            to Running
            emit FlowFrameTerminalStatusUnavailable { frame_id: frame_id }
        }

        transition ClassifyFlowRunPublicResultSuccess {
            per_phase [Running]
            on input ClassifyFlowRunPublicResult { run_id, status }
            guard "run_status_success" { status == FlowRunStatus::Completed }
            update {}
            to Running
            emit FlowRunPublicResultClassified {
                run_id: run_id,
                result: FlowRunPublicResultClassKind::Success
            }
        }

        transition ClassifyFlowRunPublicResultFailure {
            per_phase [Running]
            on input ClassifyFlowRunPublicResult { run_id, status }
            guard "run_status_failure" {
                status == FlowRunStatus::Failed
                || status == FlowRunStatus::Canceled
            }
            update {}
            to Running
            emit FlowRunPublicResultClassified {
                run_id: run_id,
                result: FlowRunPublicResultClassKind::Failure
            }
        }

        transition ClassifyMemberWaitRuntimeMaterialPresentRunning {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_material_present" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_runtime_generations.contains_key(agent_identity) == true
                && self.identity_runtime_fence_tokens.contains_key(agent_identity) == true
            }
            update {}
            to Running
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::RuntimeMaterialPresent }
        }

        transition ClassifyMemberWaitRuntimeMaterialPresentStopped {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "runtime_material_present" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_runtime_generations.contains_key(agent_identity) == true
                && self.identity_runtime_fence_tokens.contains_key(agent_identity) == true
            }
            update {}
            to Stopped
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::RuntimeMaterialPresent }
        }

        transition ClassifyMemberWaitRuntimeMaterialPresentCompleted {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "runtime_material_present" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_runtime_generations.contains_key(agent_identity) == true
                && self.identity_runtime_fence_tokens.contains_key(agent_identity) == true
            }
            update {}
            to Completed
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::RuntimeMaterialPresent }
        }

        transition ClassifyMemberWaitRuntimeMaterialPresentDestroyed {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "runtime_material_present" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_runtime_generations.contains_key(agent_identity) == true
                && self.identity_runtime_fence_tokens.contains_key(agent_identity) == true
            }
            update {}
            to Destroyed
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::RuntimeMaterialPresent }
        }

        transition ClassifyMemberWaitMissingRuntimeMaterialRunning {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_material_missing" {
                self.identity_to_runtime.contains_key(agent_identity) == false
                || self.identity_runtime_generations.contains_key(agent_identity) == false
                || self.identity_runtime_fence_tokens.contains_key(agent_identity) == false
            }
            update {}
            to Running
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::MissingRuntimeMaterial }
        }

        transition ClassifyMemberWaitMissingRuntimeMaterialStopped {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "runtime_material_missing" {
                self.identity_to_runtime.contains_key(agent_identity) == false
                || self.identity_runtime_generations.contains_key(agent_identity) == false
                || self.identity_runtime_fence_tokens.contains_key(agent_identity) == false
            }
            update {}
            to Stopped
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::MissingRuntimeMaterial }
        }

        transition ClassifyMemberWaitMissingRuntimeMaterialCompleted {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "runtime_material_missing" {
                self.identity_to_runtime.contains_key(agent_identity) == false
                || self.identity_runtime_generations.contains_key(agent_identity) == false
                || self.identity_runtime_fence_tokens.contains_key(agent_identity) == false
            }
            update {}
            to Completed
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::MissingRuntimeMaterial }
        }

        transition ClassifyMemberWaitMissingRuntimeMaterialDestroyed {
            on input ClassifyMemberWait { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "runtime_material_missing" {
                self.identity_to_runtime.contains_key(agent_identity) == false
                || self.identity_runtime_generations.contains_key(agent_identity) == false
                || self.identity_runtime_fence_tokens.contains_key(agent_identity) == false
            }
            update {}
            to Destroyed
            emit MemberWaitClassified { agent_identity: agent_identity, result: MemberWaitClassificationKind::MissingRuntimeMaterial }
        }

        // Flow topology edge admission. The shell extracts the pure rule-match
        // verdict (Allow/Deny) from declarative `TopologyRules` and the
        // enforcement mode; the MobMachine decides the admission verdict the
        // shell mirrors. No machine state is read or mutated — the rule-match
        // is a config-only observation — so these are pure classification
        // transitions across all lifecycle phases, like ClassifyMemberWait.
        transition ResolveFlowDelegationEdgeAdmissionAllowed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveFlowDelegationEdgeAdmission { from_role, to_role, rule_verdict, mode }
            guard "rule_allows_edge" { rule_verdict == MobFlowDelegationEdgeRuleVerdictKind::Allow }
            update {}
            to Running
            emit FlowDelegationEdgeAdmissionResolved { from_role: from_role, to_role: to_role, admission: MobFlowDelegationEdgeAdmissionKind::Admitted }
        }

        transition ResolveFlowDelegationEdgeAdmissionDeniedStrict {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveFlowDelegationEdgeAdmission { from_role, to_role, rule_verdict, mode }
            guard "rule_denies_edge_strict_mode" {
                rule_verdict == MobFlowDelegationEdgeRuleVerdictKind::Deny
                && mode == MobFlowDelegationEdgeModeKind::Strict
            }
            update {}
            to Running
            emit FlowDelegationEdgeAdmissionResolved { from_role: from_role, to_role: to_role, admission: MobFlowDelegationEdgeAdmissionKind::DeniedStrict }
        }

        transition ResolveFlowDelegationEdgeAdmissionDeniedAdvisory {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveFlowDelegationEdgeAdmission { from_role, to_role, rule_verdict, mode }
            guard "rule_denies_edge_advisory_mode" {
                rule_verdict == MobFlowDelegationEdgeRuleVerdictKind::Deny
                && mode == MobFlowDelegationEdgeModeKind::Advisory
            }
            update {}
            to Running
            emit FlowDelegationEdgeAdmissionResolved { from_role: from_role, to_role: to_role, admission: MobFlowDelegationEdgeAdmissionKind::DeniedAdvisory }
        }

        // --- Remote-member runtime observation terminality ---
        //
        // The bridge consumer observes a remote member's runtime state during
        // respawn/destroy cleanup. The observed state is a pure wire projection;
        // MobMachine owns whether it is terminal. Pure classification across all
        // lifecycle phases, like ClassifySpawnManyFailure.

        transition ClassifyRemoteMemberRuntimeObservationTerminal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyRemoteMemberRuntimeObservation { observed_state }
            guard "observed_state_terminal" {
                observed_state == MobRemoteMemberRuntimeObservedState::Retired
                || observed_state == MobRemoteMemberRuntimeObservedState::Stopped
                || observed_state == MobRemoteMemberRuntimeObservedState::Destroyed
            }
            update {}
            to Running
            emit RemoteMemberRuntimeTerminalityClassified { observed_state: observed_state, terminality: MobRemoteMemberRuntimeTerminality::Terminal }
        }

        transition ClassifyRemoteMemberRuntimeObservationNonTerminal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyRemoteMemberRuntimeObservation { observed_state }
            guard "observed_state_non_terminal" {
                observed_state == MobRemoteMemberRuntimeObservedState::Initializing
                || observed_state == MobRemoteMemberRuntimeObservedState::Idle
                || observed_state == MobRemoteMemberRuntimeObservedState::Attached
                || observed_state == MobRemoteMemberRuntimeObservedState::Running
            }
            update {}
            to Running
            emit RemoteMemberRuntimeTerminalityClassified { observed_state: observed_state, terminality: MobRemoteMemberRuntimeTerminality::NonTerminal }
        }

        // --- Spawn-member operator admission ---
        //
        // The tool surface extracts three raw observations (manage scope,
        // profile scope, privileged-arg presence) and feeds them here; MobMachine
        // composes the Allow/Deny verdict. Pure classification across all phases.

        transition ResolveSpawnMemberAdmissionManageScope {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission { manage_scope_present, profile_scope_present, privileged_args_present }
            guard "manage_scope_allows" { manage_scope_present == true }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Allowed }
        }

        transition ResolveSpawnMemberAdmissionPrivilegedArgsDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission { manage_scope_present, profile_scope_present, privileged_args_present }
            guard "privileged_args_without_manage_scope" {
                manage_scope_present == false
                && privileged_args_present == true
            }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Denied }
        }

        transition ResolveSpawnMemberAdmissionProfileScope {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission { manage_scope_present, profile_scope_present, privileged_args_present }
            guard "profile_scope_allows" {
                manage_scope_present == false
                && privileged_args_present == false
                && profile_scope_present == true
            }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Allowed }
        }

        transition ResolveSpawnMemberAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission { manage_scope_present, profile_scope_present, privileged_args_present }
            guard "no_scope_denies" {
                manage_scope_present == false
                && privileged_args_present == false
                && profile_scope_present == false
            }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Denied }
        }

        // --- Current-mob operator admission ---
        //
        // The tool surface extracts a single raw observation (whether the
        // operator holds manage scope over the current mob) and feeds it here;
        // MobMachine decides the Allow/Deny verdict. Pure classification across
        // all phases.

        transition ResolveCurrentMobAdmissionAllowed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveCurrentMobAdmission { can_manage_mob }
            guard "manage_scope_allows" { can_manage_mob == true }
            update {}
            to Running
            emit CurrentMobAdmissionResolved { admission: MobCurrentMobAdmissionKind::Allowed }
        }

        transition ResolveCurrentMobAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveCurrentMobAdmission { can_manage_mob }
            guard "no_manage_scope_denies" { can_manage_mob == false }
            update {}
            to Running
            emit CurrentMobAdmissionResolved { admission: MobCurrentMobAdmissionKind::Denied }
        }

        // --- Coarse spawn-tool admission ---
        //
        // The tool surface extracts a single raw observation (whether the
        // operator can spawn ANY profile in the current mob) and feeds it here;
        // MobMachine decides the Allow/Deny verdict. This coarse gate uniquely
        // covers the empty-specs spawn_many_members case (no per-member
        // admission fires when zero specs are submitted). Pure classification
        // across all phases.

        transition ResolveSpawnToolAdmissionAllowed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnToolAdmission { can_spawn_any_profile }
            guard "spawn_any_profile_allows" { can_spawn_any_profile == true }
            update {}
            to Running
            emit SpawnToolAdmissionResolved { admission: MobSpawnToolAdmissionKind::Allowed }
        }

        transition ResolveSpawnToolAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnToolAdmission { can_spawn_any_profile }
            guard "no_spawn_any_profile_denies" { can_spawn_any_profile == false }
            update {}
            to Running
            emit SpawnToolAdmissionResolved { admission: MobSpawnToolAdmissionKind::Denied }
        }

        // --- Operator create-mob admission ---
        //
        // The tool surface extracts a single raw observation (whether the
        // operator holds the create-mobs capability bit) and feeds it here;
        // MobMachine decides the Allow/Deny verdict. Pure classification across
        // all phases.

        transition ResolveCreateMobAdmissionAllowed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveCreateMobAdmission { can_create_mobs }
            guard "create_capability_allows" { can_create_mobs == true }
            update {}
            to Running
            emit CreateMobAdmissionResolved { admission: MobCreateMobAdmissionKind::Allowed }
        }

        transition ResolveCreateMobAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveCreateMobAdmission { can_create_mobs }
            guard "no_create_capability_denies" { can_create_mobs == false }
            update {}
            to Running
            emit CreateMobAdmissionResolved { admission: MobCreateMobAdmissionKind::Denied }
        }

        // --- Operator profile-mutation admission ---
        //
        // The tool surface extracts a single raw observation (whether the
        // operator holds the mutate-profiles capability bit) and feeds it here;
        // MobMachine decides the Allow/Deny verdict. Pure classification across
        // all phases.

        transition ResolveProfileMutationAdmissionAllowed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveProfileMutationAdmission { can_mutate_profiles }
            guard "mutate_capability_allows" { can_mutate_profiles == true }
            update {}
            to Running
            emit ProfileMutationAdmissionResolved { admission: MobProfileMutationAdmissionKind::Allowed }
        }

        transition ResolveProfileMutationAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveProfileMutationAdmission { can_mutate_profiles }
            guard "no_mutate_capability_denies" { can_mutate_profiles == false }
            update {}
            to Running
            emit ProfileMutationAdmissionResolved { admission: MobProfileMutationAdmissionKind::Denied }
        }

        // --- Within-mob member-operation eligibility classification ---
        //
        // Spawn finalization, peer messaging, and respawn finalization require
        // the mob to be live and running. MobMachine owns that eligibility over
        // its own lifecycle phase plus the `destroy_admitted` marker: `Admitted`
        // iff `Running` and destruction has not been admitted, `DeniedNotRunning`
        // otherwise. The owning actor mirrors `DeniedNotRunning` to the same
        // `InvalidTransition { from: self.state(), to: Running }` rejection it
        // previously produced from the handwritten `require_state` pre-check.

        transition ClassifyMemberOperationEligibilityAdmitted {
            per_phase [Running]
            on input ClassifyMemberOperationEligibility {}
            guard "running_and_not_destroying_is_eligible" { self.destroy_admitted == false }
            update {}
            to Running
            emit MemberOperationEligibilityResolved { admission: MobMemberOperationEligibilityKind::Admitted }
        }

        transition ClassifyMemberOperationEligibilityRunningDestroyDenied {
            per_phase [Running]
            on input ClassifyMemberOperationEligibility {}
            guard "running_but_destroying_is_denied" { self.destroy_admitted == true }
            update {}
            to Running
            emit MemberOperationEligibilityResolved { admission: MobMemberOperationEligibilityKind::DeniedNotRunning }
        }

        transition ClassifyMemberOperationEligibilityNotRunning {
            per_phase [Stopped, Completed, Destroyed]
            on input ClassifyMemberOperationEligibility {}
            update {}
            to Running
            emit MemberOperationEligibilityResolved { admission: MobMemberOperationEligibilityKind::DeniedNotRunning }
        }

        // --- Bridge-rejection recovery classification ---
        //
        // The mob bridge consumer receives a typed wire rejection cause when an
        // `AuthorizeSupervisor` command is rejected. The cause is a pure wire
        // projection; MobMachine owns whether it is recoverable by re-running
        // `BindMember`. Pure classification across all lifecycle phases, like
        // ClassifyRemoteMemberRuntimeObservation. NotBound / StaleSupervisor /
        // SenderMismatch indicate the member is reachable but its supervisor
        // authority is out of sync — a fresh bind reconciles. Every other cause
        // is a hard contract violation that a rebind cannot fix.

        transition ClassifyBridgeRejectionRecoveryRebind {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyBridgeRejectionRecovery { rejection_cause }
            guard "rejection_recoverable_by_rebind" {
                rejection_cause == MobBridgeRejectionCause::NotBound
                || rejection_cause == MobBridgeRejectionCause::StaleSupervisor
                || rejection_cause == MobBridgeRejectionCause::SenderMismatch
            }
            update {}
            to Running
            emit BridgeRejectionRecoveryClassified { rejection_cause: rejection_cause, recovery: MobBridgeRejectionRecovery::RebindRecover }
        }

        transition ClassifyBridgeRejectionRecoveryFatal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyBridgeRejectionRecovery { rejection_cause }
            guard "rejection_fatal" {
                rejection_cause == MobBridgeRejectionCause::AlreadyBound
                || rejection_cause == MobBridgeRejectionCause::InvalidBootstrapToken
                || rejection_cause == MobBridgeRejectionCause::UnsupportedProtocolVersion
                || rejection_cause == MobBridgeRejectionCause::InvalidSupervisorSpec
                || rejection_cause == MobBridgeRejectionCause::InvalidPeerSpec
                || rejection_cause == MobBridgeRejectionCause::AddressMismatch
                || rejection_cause == MobBridgeRejectionCause::Unsupported
                || rejection_cause == MobBridgeRejectionCause::Internal
            }
            update {}
            to Running
            emit BridgeRejectionRecoveryClassified { rejection_cause: rejection_cause, recovery: MobBridgeRejectionRecovery::FatalBubbleUp }
        }

        // --- Pending-supervisor-acceptance classification ---
        //
        // When a supervisor rotation already has accepted remote peers, the
        // actor re-verifies a pending peer's acceptance by replaying the
        // `AuthorizeSupervisor` command under the pending authority. The member
        // may reply with a typed wire rejection cause (a pure wire projection);
        // MobMachine — not a handwritten shell reducer — owns the acceptance /
        // recovery verdict. Pure classification across all lifecycle phases,
        // like ClassifyBridgeRejectionRecovery. NotBound / SenderMismatch mean
        // the peer rebound to current authority so its prior acceptance is NOT
        // confirmed and the rotation must re-attempt it. StaleSupervisor means
        // the pending authority itself is stale. Every other cause is a hard
        // fatal rejection.

        transition ClassifyPendingSupervisorAcceptanceNotConfirmed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyPendingSupervisorAcceptance { rejection_cause }
            guard "pending_acceptance_not_confirmed_reattempt" {
                rejection_cause == MobBridgeRejectionCause::NotBound
                || rejection_cause == MobBridgeRejectionCause::SenderMismatch
            }
            update {}
            to Running
            emit PendingSupervisorAcceptanceClassified { rejection_cause: rejection_cause, verdict: MobPendingSupervisorAcceptanceKind::NotConfirmedReattempt }
        }

        transition ClassifyPendingSupervisorAcceptanceStale {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyPendingSupervisorAcceptance { rejection_cause }
            guard "pending_acceptance_stale_authority" {
                rejection_cause == MobBridgeRejectionCause::StaleSupervisor
            }
            update {}
            to Running
            emit PendingSupervisorAcceptanceClassified { rejection_cause: rejection_cause, verdict: MobPendingSupervisorAcceptanceKind::StalePendingAuthority }
        }

        transition ClassifyPendingSupervisorAcceptanceFatal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyPendingSupervisorAcceptance { rejection_cause }
            guard "pending_acceptance_fatal" {
                rejection_cause == MobBridgeRejectionCause::AlreadyBound
                || rejection_cause == MobBridgeRejectionCause::InvalidBootstrapToken
                || rejection_cause == MobBridgeRejectionCause::UnsupportedProtocolVersion
                || rejection_cause == MobBridgeRejectionCause::InvalidSupervisorSpec
                || rejection_cause == MobBridgeRejectionCause::InvalidPeerSpec
                || rejection_cause == MobBridgeRejectionCause::AddressMismatch
                || rejection_cause == MobBridgeRejectionCause::Unsupported
                || rejection_cause == MobBridgeRejectionCause::Internal
            }
            update {}
            to Running
            emit PendingSupervisorAcceptanceClassified { rejection_cause: rejection_cause, verdict: MobPendingSupervisorAcceptanceKind::Fatal }
        }

        transition ClassifySpawnManyFailureProfileNotFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "profile_not_found" { observation == MobSpawnManyFailureObservationKind::ProfileNotFound }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::ProfileNotFound }
        }

        transition ClassifySpawnManyFailureMemberNotFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "member_not_found" { observation == MobSpawnManyFailureObservationKind::MemberNotFound }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::MemberNotFound }
        }

        transition ClassifySpawnManyFailureMemberAlreadyExists {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "member_already_exists" { observation == MobSpawnManyFailureObservationKind::MemberAlreadyExists }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::MemberAlreadyExists }
        }

        transition ClassifySpawnManyFailureNotExternallyAddressable {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "not_externally_addressable" { observation == MobSpawnManyFailureObservationKind::NotExternallyAddressable }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::NotExternallyAddressable }
        }

        transition ClassifySpawnManyFailureInvalidTransition {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "invalid_transition" { observation == MobSpawnManyFailureObservationKind::InvalidTransition }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::InvalidTransition }
        }

        transition ClassifySpawnManyFailureWiringError {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "wiring_error" {
                observation == MobSpawnManyFailureObservationKind::WiringError
                || observation == MobSpawnManyFailureObservationKind::SupervisorRotationIncomplete
            }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::WiringError }
        }

        transition ClassifySpawnManyFailureBridgeCommandRejected {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "bridge_command_rejected" { observation == MobSpawnManyFailureObservationKind::BridgeCommandRejected }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::BridgeCommandRejected }
        }

        transition ClassifySpawnManyFailureMemberRestoreFailed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "member_restore_failed" { observation == MobSpawnManyFailureObservationKind::MemberRestoreFailed }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::MemberRestoreFailed }
        }

        transition ClassifySpawnManyFailureKickoffWaitTimedOut {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "kickoff_wait_timed_out" { observation == MobSpawnManyFailureObservationKind::KickoffWaitTimedOut }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::KickoffWaitTimedOut }
        }

        transition ClassifySpawnManyFailureReadyWaitTimedOut {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "ready_wait_timed_out" { observation == MobSpawnManyFailureObservationKind::ReadyWaitTimedOut }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::ReadyWaitTimedOut }
        }

        transition ClassifySpawnManyFailureDefinitionError {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "definition_error" { observation == MobSpawnManyFailureObservationKind::DefinitionError }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::DefinitionError }
        }

        transition ClassifySpawnManyFailureFlowNotFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "flow_not_found" { observation == MobSpawnManyFailureObservationKind::FlowNotFound }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::FlowNotFound }
        }

        transition ClassifySpawnManyFailureFlowFailed {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "flow_failed" { observation == MobSpawnManyFailureObservationKind::FlowFailed }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::FlowFailed }
        }

        transition ClassifySpawnManyFailureRunNotFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "run_not_found" { observation == MobSpawnManyFailureObservationKind::RunNotFound }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::RunNotFound }
        }

        transition ClassifySpawnManyFailureRunCanceled {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "run_canceled" { observation == MobSpawnManyFailureObservationKind::RunCanceled }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::RunCanceled }
        }

        transition ClassifySpawnManyFailureFlowTurnTimedOut {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "flow_turn_timed_out" { observation == MobSpawnManyFailureObservationKind::FlowTurnTimedOut }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::FlowTurnTimedOut }
        }

        transition ClassifySpawnManyFailureFrameDepthLimitExceeded {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "frame_depth_limit_exceeded" { observation == MobSpawnManyFailureObservationKind::FrameDepthLimitExceeded }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::FrameDepthLimitExceeded }
        }

        transition ClassifySpawnManyFailureFrameAtomicPersistenceUnavailable {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "frame_atomic_persistence_unavailable" { observation == MobSpawnManyFailureObservationKind::FrameAtomicPersistenceUnavailable }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::FrameAtomicPersistenceUnavailable }
        }

        transition ClassifySpawnManyFailureSpecRevisionConflict {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "spec_revision_conflict" { observation == MobSpawnManyFailureObservationKind::SpecRevisionConflict }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::SpecRevisionConflict }
        }

        transition ClassifySpawnManyFailureSchemaValidation {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "schema_validation" { observation == MobSpawnManyFailureObservationKind::SchemaValidation }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::SchemaValidation }
        }

        transition ClassifySpawnManyFailureInsufficientTargets {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "insufficient_targets" { observation == MobSpawnManyFailureObservationKind::InsufficientTargets }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::InsufficientTargets }
        }

        transition ClassifySpawnManyFailureTopologyViolation {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "topology_violation" { observation == MobSpawnManyFailureObservationKind::TopologyViolation }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::TopologyViolation }
        }

        transition ClassifySpawnManyFailureBridgeDeliveryRejected {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "bridge_delivery_rejected" { observation == MobSpawnManyFailureObservationKind::BridgeDeliveryRejected }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::BridgeDeliveryRejected }
        }

        transition ClassifySpawnManyFailureSupervisorEscalation {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "supervisor_escalation" { observation == MobSpawnManyFailureObservationKind::SupervisorEscalation }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::SupervisorEscalation }
        }

        transition ClassifySpawnManyFailureUnsupportedForMode {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "unsupported_for_mode" { observation == MobSpawnManyFailureObservationKind::UnsupportedForMode }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::UnsupportedForMode }
        }

        transition ClassifySpawnManyFailureMissingMemberCapability {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "missing_member_capability" { observation == MobSpawnManyFailureObservationKind::MissingMemberCapability }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::MissingMemberCapability }
        }

        transition ClassifySpawnManyFailureResetBarrier {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "reset_barrier" { observation == MobSpawnManyFailureObservationKind::ResetBarrier }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::ResetBarrier }
        }

        transition ClassifySpawnManyFailureStorageError {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "storage_error" { observation == MobSpawnManyFailureObservationKind::StorageError }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::StorageError }
        }

        transition ClassifySpawnManyFailureSessionError {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "session_error" { observation == MobSpawnManyFailureObservationKind::SessionError }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::SessionError }
        }

        transition ClassifySpawnManyFailureCommsError {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "comms_error" { observation == MobSpawnManyFailureObservationKind::CommsError }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::CommsError }
        }

        transition ClassifySpawnManyFailureCallbackPending {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "callback_pending" { observation == MobSpawnManyFailureObservationKind::CallbackPending }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::CallbackPending }
        }

        transition ClassifySpawnManyFailureStaleFenceToken {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "stale_fence_token" { observation == MobSpawnManyFailureObservationKind::StaleFenceToken }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::StaleFenceToken }
        }

        transition ClassifySpawnManyFailureStaleEventCursor {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "stale_event_cursor" { observation == MobSpawnManyFailureObservationKind::StaleEventCursor }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::StaleEventCursor }
        }

        transition ClassifySpawnManyFailureWorkNotFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "work_not_found" { observation == MobSpawnManyFailureObservationKind::WorkNotFound }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::WorkNotFound }
        }

        transition ClassifySpawnManyFailureInternal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifySpawnManyFailure { observation }
            guard "internal" { observation == MobSpawnManyFailureObservationKind::Internal }
            update {}
            to Running
            emit SpawnManyFailureClassified { observation: observation, cause: MobSpawnManyFailureCauseKind::Internal }
        }

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
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_present" { bridge_session_id != None }
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
                self.member_profile_names.insert(agent_identity, self.spawn_profile_authority_profile_names.get_cloned(agent_identity).get("value"));
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: Some(generation), session_id: bridge_session_id.get("value") }
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberSpawned,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: bridge_session_id
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: None, new_session_id: bridge_session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition SpawnRunningFreshPeerOnly {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_absent" { bridge_session_id == None }
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
                self.member_profile_names.insert(agent_identity, self.spawn_profile_authority_profile_names.get_cloned(agent_identity).get("value"));
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberSpawned,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: None
            }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition SpawnRunningReplacing {
            on input Spawn { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            guard "replacing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(replacing.get("value")) }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_present" { bridge_session_id != None }
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
                self.member_profile_names.insert(agent_identity, self.spawn_profile_authority_profile_names.get_cloned(agent_identity).get("value"));
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: Some(generation), session_id: bridge_session_id.get("value") }
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberSpawned,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: bridge_session_id
            }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(replacing.get("value")), new_session_id: bridge_session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Spawned }
        }

        transition AuthorizeSpawnProfileRunning {
            on input AuthorizeSpawnProfile { agent_identity, profile_name, model, profile_material_digest, tool_config_digest, skills_digest, provider_params_digest, output_schema_digest, external_addressable }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            update {
                self.spawn_profile_authority_profile_names.insert(agent_identity, profile_name);
                self.spawn_profile_authority_models.insert(agent_identity, model);
                self.spawn_profile_authority_material_digests.insert(agent_identity, profile_material_digest);
                self.spawn_profile_authority_tool_config_digests.insert(agent_identity, tool_config_digest);
                self.spawn_profile_authority_skills_digests.insert(agent_identity, skills_digest);
                self.spawn_profile_authority_provider_params_digests.insert(agent_identity, provider_params_digest);
                self.spawn_profile_authority_output_schema_digests.insert(agent_identity, output_schema_digest);
                self.spawn_profile_authority_external_addressable.insert(agent_identity, external_addressable);
            }
            to Running
            emit SpawnProfileAuthorized {
                agent_identity: agent_identity,
                profile_name: profile_name,
                model: model,
                profile_material_digest: profile_material_digest,
                tool_config_digest: tool_config_digest,
                skills_digest: skills_digest,
                provider_params_digest: provider_params_digest,
                output_schema_digest: output_schema_digest,
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
            on signal RecoverRosterMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable }
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
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_restore_failures.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterMemberAddressabilityRunning {
            on signal RecoverRosterMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable }
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
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
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
            emit SessionProvisionOperationOwnerAuthorized { agent_identity: agent_identity, session_id: bridge_session_id }
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
            emit SessionProvisionOperationOwnerAuthorized { agent_identity: agent_identity, session_id: bridge_session_id }
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
            emit SessionProvisionOperationOwnerAuthorized { agent_identity: agent_identity, session_id: bridge_session_id }
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
                self.member_profile_names.remove(agent_identity);
                self.member_runtime_modes.remove(agent_identity);
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
                self.member_profile_names.remove(agent_identity);
                self.member_runtime_modes.remove(agent_identity);
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
            guard "generation_binding_present" { self.identity_runtime_generations.get_copied(agent_identity) != None }
            guard "session_binding_present" { self.member_session_bindings.get_cloned(agent_identity) != None }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "external_origin" { origin == WorkOrigin::External }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            update {}
            to Running
            emit RequestRuntimeIngress {
                agent_runtime_id: agent_runtime_id,
                fence_token: fence_token,
                generation: self.identity_runtime_generations.get_copied(agent_identity),
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value"),
                work_id: work_id,
                origin: origin
            }
        }

        transition SubmitWorkRunningExternalPeerOnly {
            on input SubmitWork { agent_identity, agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_binding_present" { self.identity_runtime_generations.get_copied(agent_identity) != None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "external_origin" { origin == WorkOrigin::External }
            guard "runtime_externally_addressable" { self.externally_addressable_runtime_ids.contains(agent_runtime_id) }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) }
            update {}
            to Running
            emit RequestPeerRuntimeIngress {
                agent_runtime_id: agent_runtime_id,
                fence_token: fence_token,
                generation: self.identity_runtime_generations.get_copied(agent_identity),
                work_id: work_id,
                origin: origin
            }
        }

        transition SubmitWorkRunningInternal {
            on input SubmitWork { agent_identity, agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_binding_present" { self.identity_runtime_generations.get_copied(agent_identity) != None }
            guard "session_binding_present" { self.member_session_bindings.get_cloned(agent_identity) != None }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "internal_origin" { origin == WorkOrigin::Internal }
            update {}
            to Running
            emit RequestRuntimeIngress {
                agent_runtime_id: agent_runtime_id,
                fence_token: fence_token,
                generation: self.identity_runtime_generations.get_copied(agent_identity),
                session_id: self.member_session_bindings.get_cloned(agent_identity).get("value"),
                work_id: work_id,
                origin: origin
            }
        }

        transition SubmitWorkRunningInternalPeerOnly {
            on input SubmitWork { agent_identity, agent_runtime_id, fence_token, work_id, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_binding_present" { self.identity_runtime_generations.get_copied(agent_identity) != None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "internal_origin" { origin == WorkOrigin::Internal }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) }
            update {}
            to Running
            emit RequestPeerRuntimeIngress {
                agent_runtime_id: agent_runtime_id,
                fence_token: fence_token,
                generation: self.identity_runtime_generations.get_copied(agent_identity),
                work_id: work_id,
                origin: origin
            }
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

        transition ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable {
            on input ResolveSubmitWorkRejection { agent_identity, agent_runtime_id, fence_token, origin }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "internal_origin" { origin == WorkOrigin::Internal }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "member_peer_absent" { !self.member_peer_ids.contains_key(agent_identity) }
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
            on signal RetireMember { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
        }

        transition RetireMemberPeerOnly {
            on signal RetireMember { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
        }

        transition AdmitDestroyMemberRetireLiveRunning {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
        }

        transition AdmitDestroyMemberRetireLiveStopped {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
        }

        transition AdmitDestroyMemberRetirePeerOnlyRunning {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
        }

        transition AdmitDestroyMemberRetirePeerOnlyStopped {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringRunning {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "session_matches_binding_or_absent" {
                (session_id == None && self.member_session_bindings.get_cloned(agent_identity) == None)
                || (session_id != None && self.member_session_bindings.get_cloned(agent_identity) == session_id)
            }
            update {}
            to Running
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringStopped {
            on signal AdmitDestroyMemberRetire { agent_identity, agent_runtime_id, fence_token, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "session_matches_binding_or_absent" {
                (session_id == None && self.member_session_bindings.get_cloned(agent_identity) == None)
                || (session_id != None && self.member_session_bindings.get_cloned(agent_identity) == session_id)
            }
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
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id }
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
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: Some(generation), session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Reset }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id }
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
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: Some(generation), session_id: session_id }
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
                self.member_profile_names = EmptyMap;
                self.member_runtime_modes = EmptyMap;
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

        // Owner bridge-session authority. The runtime shell may observe the
        // owner session that requested creation, but this transition owns the
        // facts that later drive lookup, archive cleanup, and implicit-mob
        // admission.
        transition BindOwnerBridgeSessionRunning {
            on input BindOwnerBridgeSession { bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob }
            guard { self.lifecycle_phase == Phase::Running }
            guard "owner_bridge_session_absent" { self.owner_bridge_session_id == None }
            guard "implicit_delegation_requires_cleanup" { implicit_delegation_mob == false || destroy_on_owner_archive == true }
            update {
                self.owner_bridge_session_id = Some(bridge_session_id);
                self.owner_bridge_destroy_on_archive = destroy_on_owner_archive;
                self.implicit_delegation_mob = implicit_delegation_mob;
            }
            to Running
            emit OwnerBridgeSessionBound {
                bridge_session_id: bridge_session_id,
                destroy_on_owner_archive: destroy_on_owner_archive,
                implicit_delegation_mob: implicit_delegation_mob
            }
        }

        transition RecoverOwnerBridgeSessionRunning {
            on signal RecoverOwnerBridgeSession { bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob }
            guard { self.lifecycle_phase == Phase::Running }
            guard "owner_bridge_session_absent" { self.owner_bridge_session_id == None }
            guard "implicit_delegation_requires_cleanup" { implicit_delegation_mob == false || destroy_on_owner_archive == true }
            update {
                self.owner_bridge_session_id = Some(bridge_session_id);
                self.owner_bridge_destroy_on_archive = destroy_on_owner_archive;
                self.implicit_delegation_mob = implicit_delegation_mob;
            }
            to Running
        }

        transition RecoverOwnerBridgeSessionAlreadyCurrent {
            on signal RecoverOwnerBridgeSession { bridge_session_id, destroy_on_owner_archive, implicit_delegation_mob }
            guard { self.lifecycle_phase == Phase::Running }
            guard "owner_bridge_session_current" { self.owner_bridge_session_id == Some(bridge_session_id) }
            guard "cleanup_policy_current" { self.owner_bridge_destroy_on_archive == destroy_on_owner_archive }
            guard "implicit_classification_current" { self.implicit_delegation_mob == implicit_delegation_mob }
            guard "implicit_delegation_requires_cleanup" { implicit_delegation_mob == false || destroy_on_owner_archive == true }
            update {}
            to Running
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

        transition WireMembersWithTrustRunning {
            on input WireMembersWithTrust { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_not_already_wired" { self.wiring_edges.contains(edge) == false }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            guard "a_member_endpoint_registered" { self.member_peer_endpoints.contains_key(a_identity) == true }
            guard "b_member_endpoint_registered" { self.member_peer_endpoints.contains_key(b_identity) == true }
            update {
                self.wiring_edges.insert(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit MemberTrustWiringRequested {
                edge: edge,
                a_peer_id: self.member_peer_ids.get_cloned(a_identity).get("value"),
                b_peer_id: self.member_peer_ids.get_cloned(b_identity).get("value"),
                a_endpoint: self.member_peer_endpoints.get_cloned(a_identity).get("value"),
                b_endpoint: self.member_peer_endpoints.get_cloned(b_identity).get("value"),
                epoch: self.topology_epoch
            }
            emit EmitWiringLifecycleNotice { kind: WiringLifecycleKind::Wired, edge: edge }
        }

        transition WireMembersWithTrustAlreadyWired {
            on input WireMembersWithTrust { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "edge_already_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            update {}
            to Running
            emit WiringTrustRepairRequested { edge: edge }
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

        transition AuthorizeMemberPeerRebindRunning {
            on input AuthorizeMemberPeerRebind { agent_identity, expected_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_endpoint_matches_expected" { self.member_peer_endpoints.get_cloned(agent_identity) == Some(expected_peer_endpoint) }
            guard "current_peer_id_matches_expected" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(expected_peer_endpoint)) }
            update {}
            to Running
            emit MemberPeerRebindAuthorized {
                agent_identity: agent_identity,
                peer_id: mob_machine_member_peer_endpoint_peer_id(expected_peer_endpoint),
                peer_endpoint: expected_peer_endpoint
            }
        }

        transition AuthorizeMemberPeerOverlayRunning {
            on input AuthorizeMemberPeerOverlay { agent_identity, expected_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_endpoint_matches_expected" { self.member_peer_endpoints.get_cloned(agent_identity) == Some(expected_peer_endpoint) }
            guard "current_peer_id_matches_expected" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(expected_peer_endpoint)) }
            guard "overlay_member_endpoints_complete" { mob_machine_member_peer_overlay_complete(self.wiring_edges, self.member_peer_endpoints, agent_identity) == true }
            guard "overlay_peer_ids_unique" { mob_machine_member_peer_overlay_peer_ids_unique(self.wiring_edges, self.member_peer_endpoints, self.external_peer_edges, agent_identity) == true }
            update {}
            to Running
            emit MemberPeerOverlayAuthorized {
                agent_identity: agent_identity,
                peer_id: mob_machine_member_peer_endpoint_peer_id(expected_peer_endpoint),
                peer_overlay_endpoints: mob_machine_member_peer_overlay(self.wiring_edges, self.member_peer_endpoints, self.external_peer_edges, agent_identity),
                epoch: self.topology_epoch
            }
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

        // Mob-wide supervisor authority. The shell may generate key material,
        // but MobMachine owns whether the current/pending authority tuple is
        // admitted and which exact tuple may be persisted or used by the
        // bridge.

        transition ProvisionSupervisorAuthority {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ProvisionSupervisorAuthority { peer_id, signing_key, epoch, protocol_version }
            guard "supervisor_authority_absent" {
                self.supervisor_authority_peer_id == None
                && self.supervisor_authority_signing_key == None
                && self.supervisor_authority_epoch == None
                && self.supervisor_authority_protocol_version == None
            }
            guard "initial_epoch" { epoch == 0 }
            update {
                self.supervisor_authority_peer_id = Some(peer_id);
                self.supervisor_authority_signing_key = Some(signing_key);
                self.supervisor_authority_epoch = Some(epoch);
                self.supervisor_authority_protocol_version = Some(protocol_version);
                self.supervisor_pending_authority_peer_id = None;
                self.supervisor_pending_authority_signing_key = None;
                self.supervisor_pending_authority_epoch = None;
                self.supervisor_pending_authority_protocol_version = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: peer_id,
                signing_key: signing_key,
                epoch: epoch,
                protocol_version: protocol_version,
                pending_peer_id: None,
                pending_signing_key: None,
                pending_epoch: None,
                pending_protocol_version: None,
                pending_accepted_peer_ids: EmptySet
            }
        }

        transition RecoverSupervisorAuthority {
            per_phase [Running, Stopped, Completed, Destroyed]
            on signal RecoverSupervisorAuthority {
                peer_id,
                signing_key,
                epoch,
                protocol_version,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids
            }
            guard "supervisor_authority_absent" {
                self.supervisor_authority_peer_id == None
                && self.supervisor_authority_signing_key == None
                && self.supervisor_authority_epoch == None
                && self.supervisor_authority_protocol_version == None
            }
            guard "pending_shape_consistent" {
                (
                    pending_peer_id == None
                    && pending_signing_key == None
                    && pending_epoch == None
                    && pending_protocol_version == None
                    && pending_accepted_peer_ids == EmptySet
                )
                || (
                    pending_peer_id != None
                    && pending_signing_key != None
                    && pending_epoch != None
                    && pending_protocol_version != None
                )
            }
            update {
                self.supervisor_authority_peer_id = Some(peer_id);
                self.supervisor_authority_signing_key = Some(signing_key);
                self.supervisor_authority_epoch = Some(epoch);
                self.supervisor_authority_protocol_version = Some(protocol_version);
                self.supervisor_pending_authority_peer_id = pending_peer_id;
                self.supervisor_pending_authority_signing_key = pending_signing_key;
                self.supervisor_pending_authority_epoch = pending_epoch;
                self.supervisor_pending_authority_protocol_version = pending_protocol_version;
                self.supervisor_pending_authority_accepted_peer_ids = pending_accepted_peer_ids;
            }
            to Running
        }

        transition ClearSupervisorPendingRotation {
            per_phase [Running, Stopped, Completed]
            on input ClearSupervisorPendingRotation { current_peer_id, current_epoch, protocol_version }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(protocol_version)
            }
            update {
                self.supervisor_pending_authority_peer_id = None;
                self.supervisor_pending_authority_signing_key = None;
                self.supervisor_pending_authority_epoch = None;
                self.supervisor_pending_authority_protocol_version = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: current_peer_id,
                signing_key: self.supervisor_authority_signing_key.get("value"),
                epoch: current_epoch,
                protocol_version: protocol_version,
                pending_peer_id: None,
                pending_signing_key: None,
                pending_epoch: None,
                pending_protocol_version: None,
                pending_accepted_peer_ids: EmptySet
            }
        }

        transition RecordSupervisorPendingRotation {
            per_phase [Running, Stopped, Completed]
            on input RecordSupervisorPendingRotation {
                current_peer_id,
                current_epoch,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                protocol_version,
                accepted_peer_ids
            }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(protocol_version)
            }
            guard "pending_authority_changes_peer" { pending_peer_id != current_peer_id }
            update {
                self.supervisor_pending_authority_peer_id = Some(pending_peer_id);
                self.supervisor_pending_authority_signing_key = Some(pending_signing_key);
                self.supervisor_pending_authority_epoch = Some(pending_epoch);
                self.supervisor_pending_authority_protocol_version = Some(protocol_version);
                self.supervisor_pending_authority_accepted_peer_ids = accepted_peer_ids;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: current_peer_id,
                signing_key: self.supervisor_authority_signing_key.get("value"),
                epoch: current_epoch,
                protocol_version: protocol_version,
                pending_peer_id: Some(pending_peer_id),
                pending_signing_key: Some(pending_signing_key),
                pending_epoch: Some(pending_epoch),
                pending_protocol_version: Some(protocol_version),
                pending_accepted_peer_ids: accepted_peer_ids
            }
        }

        transition CommitSupervisorRotation {
            per_phase [Running, Stopped, Completed]
            on input CommitSupervisorRotation { current_peer_id, current_epoch, next_peer_id, next_signing_key, next_epoch, protocol_version }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(protocol_version)
            }
            guard "next_epoch_is_successor" { next_epoch == current_epoch + 1 }
            guard "next_authority_changes_peer" { next_peer_id != current_peer_id }
            update {
                self.supervisor_authority_peer_id = Some(next_peer_id);
                self.supervisor_authority_signing_key = Some(next_signing_key);
                self.supervisor_authority_epoch = Some(next_epoch);
                self.supervisor_authority_protocol_version = Some(protocol_version);
                self.supervisor_pending_authority_peer_id = None;
                self.supervisor_pending_authority_signing_key = None;
                self.supervisor_pending_authority_epoch = None;
                self.supervisor_pending_authority_protocol_version = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: next_peer_id,
                signing_key: next_signing_key,
                epoch: next_epoch,
                protocol_version: protocol_version,
                pending_peer_id: None,
                pending_signing_key: None,
                pending_epoch: None,
                pending_protocol_version: None,
                pending_accepted_peer_ids: EmptySet
            }
        }

        transition ClearSupervisorAuthorityForDestroy {
            on input ClearSupervisorAuthorityForDestroy { current_peer_id, current_signing_key, current_epoch, protocol_version }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key == Some(current_signing_key)
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(protocol_version)
            }
            update {
                self.supervisor_authority_peer_id = None;
                self.supervisor_authority_signing_key = None;
                self.supervisor_authority_epoch = None;
                self.supervisor_authority_protocol_version = None;
                self.supervisor_pending_authority_peer_id = None;
                self.supervisor_pending_authority_signing_key = None;
                self.supervisor_pending_authority_epoch = None;
                self.supervisor_pending_authority_protocol_version = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
            }
            to Destroyed
            emit DeleteSupervisorAuthority {
                peer_id: current_peer_id,
                signing_key: current_signing_key,
                epoch: current_epoch,
                protocol_version: protocol_version
            }
        }

        transition RestoreSupervisorAuthorityAfterDestroyRollback {
            on input RestoreSupervisorAuthorityAfterDestroyRollback {
                peer_id,
                signing_key,
                epoch,
                protocol_version,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids
            }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "supervisor_authority_absent" {
                self.supervisor_authority_peer_id == None
                && self.supervisor_authority_signing_key == None
                && self.supervisor_authority_epoch == None
                && self.supervisor_authority_protocol_version == None
            }
            guard "pending_shape_consistent" {
                (
                    pending_peer_id == None
                    && pending_signing_key == None
                    && pending_epoch == None
                    && pending_protocol_version == None
                    && pending_accepted_peer_ids == EmptySet
                )
                || (
                    pending_peer_id != None
                    && pending_signing_key != None
                    && pending_epoch != None
                    && pending_protocol_version != None
                )
            }
            update {
                self.supervisor_authority_peer_id = Some(peer_id);
                self.supervisor_authority_signing_key = Some(signing_key);
                self.supervisor_authority_epoch = Some(epoch);
                self.supervisor_authority_protocol_version = Some(protocol_version);
                self.supervisor_pending_authority_peer_id = pending_peer_id;
                self.supervisor_pending_authority_signing_key = pending_signing_key;
                self.supervisor_pending_authority_epoch = pending_epoch;
                self.supervisor_pending_authority_protocol_version = pending_protocol_version;
                self.supervisor_pending_authority_accepted_peer_ids = pending_accepted_peer_ids;
            }
            to Destroyed
            emit PersistSupervisorAuthority {
                peer_id: peer_id,
                signing_key: signing_key,
                epoch: epoch,
                protocol_version: protocol_version,
                pending_peer_id: pending_peer_id,
                pending_signing_key: pending_signing_key,
                pending_epoch: pending_epoch,
                pending_protocol_version: pending_protocol_version,
                pending_accepted_peer_ids: pending_accepted_peer_ids
            }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Running
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionStopped {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Stopped
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionCompleted {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Completed
            emit RejectAgentEventSubscription { agent_identity: agent_identity, reason: EventSubscriptionRejectReasonKind::NoSessionBinding }
        }
        transition SubscribeAgentEventsMissingSessionDestroyed {
            on input SubscribeAgentEvents { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "runtime_live" { self.live_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_bound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == true }
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
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Running
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundStopped {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Stopped
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundCompleted {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Completed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
            update {}
            to Completed
            emit AuthorizeMobEventRouterMemberRemoval { agent_identity: agent_identity }
        }
        transition AuthorizeMobEventRouterMemberRemovalUnboundDestroyed {
            on input AuthorizeMobEventRouterMemberRemoval { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_unbound" { mob_machine_identity_has_session_binding(self.member_session_bindings, agent_identity) == false }
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
            emit PendingSpawnOperationOwnerAuthorized { agent_identity: agent_identity, session_id: session_id }
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
            on input RunFlow { run_id, step_ids, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "run_seed_is_new" { self.run_status.contains_key(run_id) == false }
            guard "run_seed_ordered_steps_match_tracked_steps" {
                for_all(candidate in step_ids, ordered_steps.contains(candidate))
                && for_all(candidate in ordered_steps, step_ids.contains(candidate))
            }
            guard "run_seed_defaults_cover_tracked_steps" {
                for_all(candidate in step_ids,
                    step_status.get_cloned(candidate) == Some(None)
                    && output_recorded.get_cloned(candidate) == Some(false)
                    && step_condition_results.get_cloned(candidate) == Some(None)
                    && step_target_counts.get_cloned(candidate) == Some(0)
                    && step_target_success_counts.get_cloned(candidate) == Some(0)
                    && step_target_terminal_failure_counts.get_cloned(candidate) == Some(0))
            }
            guard "run_seed_maps_cover_tracked_steps" {
                for_all(candidate in step_ids,
                    step_has_conditions.contains_key(candidate)
                    && step_dependencies.contains_key(candidate)
                    && step_dependency_modes.contains_key(candidate)
                    && step_branches.contains_key(candidate)
                    && step_collection_policies.contains_key(candidate)
                    && step_quorum_thresholds.contains_key(candidate))
            }
            guard "run_seed_map_keys_are_tracked" {
                for_all(candidate in step_status.keys(), step_ids.contains(candidate))
                && for_all(candidate in output_recorded.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_condition_results.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_has_conditions.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_dependencies.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_dependency_modes.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_branches.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_collection_policies.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_quorum_thresholds.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_counts.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_success_counts.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_terminal_failure_counts.keys(), step_ids.contains(candidate))
            }
            guard "run_seed_dependency_targets_are_tracked" {
                for_all(candidate in step_dependencies.keys(),
                    for_all(dependency in step_dependencies.get_cloned(candidate).get("value"),
                        step_ids.contains(dependency)))
            }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Pending);
                self.run_tracked_steps.insert(run_id, step_ids);
                self.run_ordered_steps.insert(run_id, ordered_steps);
                self.run_step_status.insert(run_id, step_status);
                self.run_output_recorded.insert(run_id, output_recorded);
                self.run_step_condition_results.insert(run_id, step_condition_results);
                self.run_step_has_conditions.insert(run_id, step_has_conditions);
                self.run_step_dependencies.insert(run_id, step_dependencies);
                self.run_step_dependency_modes.insert(run_id, step_dependency_modes);
                self.run_step_branches.insert(run_id, step_branches);
                self.run_step_collection_policies.insert(run_id, step_collection_policies);
                self.run_step_quorum_thresholds.insert(run_id, step_quorum_thresholds);
                self.run_step_target_counts.insert(run_id, step_target_counts);
                self.run_step_target_success_counts.insert(run_id, step_target_success_counts);
                self.run_step_target_terminal_failure_counts.insert(run_id, step_target_terminal_failure_counts);
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
                self.run_last_granted_frame.insert(run_id, None);
                self.run_last_granted_loop.insert(run_id, None);
                self.run_max_active_nodes.insert(run_id, max_active_nodes);
                self.run_max_active_frames.insert(run_id, max_active_frames);
                self.run_max_frame_depth.insert(run_id, max_frame_depth);
                self.active_run_count += 1;
            }
            to Running
            emit EmitFlowRunNotice
        }

        transition CreateRunSeedRunning {
            on input CreateRunSeed { run_id, step_ids, ordered_steps, step_status, output_recorded, step_condition_results, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, step_target_counts, step_target_success_counts, step_target_terminal_failure_counts, escalation_threshold, max_step_retries, max_active_nodes, max_active_frames, max_frame_depth }
            guard { self.lifecycle_phase == Phase::Running }
            guard "run_seed_is_new" { self.run_status.contains_key(run_id) == false }
            guard "run_seed_ordered_steps_match_tracked_steps" {
                for_all(candidate in step_ids, ordered_steps.contains(candidate))
                && for_all(candidate in ordered_steps, step_ids.contains(candidate))
            }
            guard "run_seed_defaults_cover_tracked_steps" {
                for_all(candidate in step_ids,
                    step_status.get_cloned(candidate) == Some(None)
                    && output_recorded.get_cloned(candidate) == Some(false)
                    && step_condition_results.get_cloned(candidate) == Some(None)
                    && step_target_counts.get_cloned(candidate) == Some(0)
                    && step_target_success_counts.get_cloned(candidate) == Some(0)
                    && step_target_terminal_failure_counts.get_cloned(candidate) == Some(0))
            }
            guard "run_seed_maps_cover_tracked_steps" {
                for_all(candidate in step_ids,
                    step_has_conditions.contains_key(candidate)
                    && step_dependencies.contains_key(candidate)
                    && step_dependency_modes.contains_key(candidate)
                    && step_branches.contains_key(candidate)
                    && step_collection_policies.contains_key(candidate)
                    && step_quorum_thresholds.contains_key(candidate))
            }
            guard "run_seed_map_keys_are_tracked" {
                for_all(candidate in step_status.keys(), step_ids.contains(candidate))
                && for_all(candidate in output_recorded.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_condition_results.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_has_conditions.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_dependencies.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_dependency_modes.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_branches.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_collection_policies.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_quorum_thresholds.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_counts.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_success_counts.keys(), step_ids.contains(candidate))
                && for_all(candidate in step_target_terminal_failure_counts.keys(), step_ids.contains(candidate))
            }
            guard "run_seed_dependency_targets_are_tracked" {
                for_all(candidate in step_dependencies.keys(),
                    for_all(dependency in step_dependencies.get_cloned(candidate).get("value"),
                        step_ids.contains(dependency)))
            }
            update {
                self.run_status.insert(run_id, FlowRunStatus::Pending);
                self.run_tracked_steps.insert(run_id, step_ids);
                self.run_ordered_steps.insert(run_id, ordered_steps);
                self.run_step_status.insert(run_id, step_status);
                self.run_output_recorded.insert(run_id, output_recorded);
                self.run_step_condition_results.insert(run_id, step_condition_results);
                self.run_step_has_conditions.insert(run_id, step_has_conditions);
                self.run_step_dependencies.insert(run_id, step_dependencies);
                self.run_step_dependency_modes.insert(run_id, step_dependency_modes);
                self.run_step_branches.insert(run_id, step_branches);
                self.run_step_collection_policies.insert(run_id, step_collection_policies);
                self.run_step_quorum_thresholds.insert(run_id, step_quorum_thresholds);
                self.run_step_target_counts.insert(run_id, step_target_counts);
                self.run_step_target_success_counts.insert(run_id, step_target_success_counts);
                self.run_step_target_terminal_failure_counts.insert(run_id, step_target_terminal_failure_counts);
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
                self.run_last_granted_frame.insert(run_id, None);
                self.run_last_granted_loop.insert(run_id, None);
                self.run_max_active_nodes.insert(run_id, max_active_nodes);
                self.run_max_active_frames.insert(run_id, max_active_frames);
                self.run_max_frame_depth.insert(run_id, max_frame_depth);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition CreateFrameSeedRunning {
            on input CreateFrameSeed { run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node }
            guard { self.lifecycle_phase == Phase::Running }
            guard "frame_seed_is_new" { self.frame_phase.contains_key(frame_id) == false }
            guard "run_known" { self.run_status.contains_key(run_id) == true }
            guard "body_frame_has_parent_loop" { frame_scope != FrameScope::Body || loop_instance_id != None }
            guard "frame_seed_has_no_last_admitted_node" { last_admitted_node == None }
            guard "frame_seed_ordered_nodes_match_tracked_nodes" {
                for_all(candidate in tracked_nodes, ordered_nodes.contains(candidate))
                && for_all(candidate in ordered_nodes, tracked_nodes.contains(candidate))
            }
            guard "frame_seed_status_covers_tracked_nodes" {
                for_all(candidate in tracked_nodes,
                    node_status.contains_key(candidate))
            }
            guard "frame_seed_defaults_cover_tracked_nodes" {
                for_all(candidate in tracked_nodes,
                    output_recorded.get_cloned(candidate) == Some(false)
                    && node_condition_results.get_cloned(candidate) == Some(None))
            }
            guard "frame_seed_maps_cover_tracked_nodes" {
                for_all(candidate in tracked_nodes,
                    node_dependencies.contains_key(candidate)
                    && node_dependency_modes.contains_key(candidate)
                    && node_branches.contains_key(candidate))
            }
            guard "frame_seed_map_keys_are_tracked" {
                for_all(candidate in node_status.keys(), tracked_nodes.contains(candidate))
                && for_all(candidate in output_recorded.keys(), tracked_nodes.contains(candidate))
                && for_all(candidate in node_condition_results.keys(), tracked_nodes.contains(candidate))
                && for_all(candidate in node_dependencies.keys(), tracked_nodes.contains(candidate))
                && for_all(candidate in node_dependency_modes.keys(), tracked_nodes.contains(candidate))
                && for_all(candidate in node_branches.keys(), tracked_nodes.contains(candidate))
            }
            guard "frame_seed_ready_queue_nodes_are_tracked" {
                for_all(candidate in ready_queue, tracked_nodes.contains(candidate))
            }
            guard "frame_seed_dependency_targets_are_tracked" {
                for_all(candidate in node_dependencies.keys(),
                    for_all(dependency in node_dependencies.get_cloned(candidate).get("value"),
                        tracked_nodes.contains(dependency)))
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
                self.frame_output_recorded.insert(frame_id, output_recorded);
                self.frame_node_condition_results.insert(frame_id, node_condition_results);
                self.frame_last_admitted_node.insert(frame_id, last_admitted_node);
                if frame_scope == FrameScope::Body {
                    self.loop_stage.insert(loop_instance_id.get("value"), LoopIterationStage::BodyFrameActive);
                    self.loop_active_body_frame.insert(loop_instance_id.get("value"), Some(frame_id));
                }
            }
            to Running
            emit EmitRunLifecycleNotice
            emit FrameSeedConfirmed { frame_id: frame_id, disposition: MobFrameSeedDisposition::Seeded }
        }

        // Idempotent re-seed: when the frame is already tracked, `CreateFrameSeed`
        // is a no-op that emits `AlreadySeeded` instead of being rejected by the
        // `frame_seed_is_new` guard. This is the machine-owned idempotency
        // disposition the flow shell mirrors (replacing the former
        // `error.to_string().contains("frame_seed_is_new")` string folklore).
        transition CreateFrameSeedAlreadySeeded {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input CreateFrameSeed { run_id, frame_id, frame_scope, loop_instance_id, iteration, tracked_nodes, ordered_nodes, node_kind, node_dependencies, node_dependency_modes, node_branches, node_step_ids, node_loop_ids, node_status, ready_queue, output_recorded, node_condition_results, last_admitted_node }
            guard "frame_seed_already_tracked" { self.frame_phase.contains_key(frame_id) == true }
            update {}
            to Running
            emit FrameSeedConfirmed { frame_id: frame_id, disposition: MobFrameSeedDisposition::AlreadySeeded }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "dispatch_step_command" { command == FlowRunReducerCommandKind::DispatchStep }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "dispatched_step_status" { step_status == Some(StepRunStatus::Dispatched) }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Dispatched);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandCompleteStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "complete_step_command" { command == FlowRunReducerCommandKind::CompleteStep }
            guard "has_step_id" { step_id != None }
            guard "completed_step_status" { step_status == Some(StepRunStatus::Completed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Completed);
                self.run_consecutive_failure_count.insert(run_id, 0);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordStepOutput {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_step_output_command" { command == FlowRunReducerCommandKind::RecordStepOutput }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_output_recorded = mob_machine_run_step_bool_after_set(self.run_output_recorded, run_id, step_id.get("value"), true);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandConditionPassed {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "condition_passed_command" { command == FlowRunReducerCommandKind::ConditionPassed }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_condition_results = mob_machine_run_step_condition_result_after_set(self.run_step_condition_results, run_id, step_id.get("value"), true);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandConditionRejected {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "condition_rejected_command" { command == FlowRunReducerCommandKind::ConditionRejected }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_condition_results = mob_machine_run_step_condition_result_after_set(self.run_step_condition_results, run_id, step_id.get("value"), false);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandFailStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "fail_step_command" { command == FlowRunReducerCommandKind::FailStep }
            guard "has_step_id" { step_id != None }
            guard "failed_step_status" { step_status == Some(StepRunStatus::Failed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "supervisor_escalation_not_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") == 0
                || self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    < self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
        }

        transition AuthorizeFlowRunReducerCommandFailStepEscalating {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "fail_step_command" { command == FlowRunReducerCommandKind::FailStep }
            guard "has_step_id" { step_id != None }
            guard "failed_step_status" { step_status == Some(StepRunStatus::Failed) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "supervisor_escalation_due" {
                self.run_escalation_threshold.get_cloned(run_id).get("value") > 0
                && self.run_consecutive_failure_count.get_cloned(run_id).get("value") + 1
                    >= self.run_escalation_threshold.get_cloned(run_id).get("value")
            }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
            emit EscalateSupervisor
        }

        transition AuthorizeFlowRunReducerCommandSkipStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "skip_step_command" { command == FlowRunReducerCommandKind::SkipStep }
            guard "has_step_id" { step_id != None }
            guard "skipped_step_status" { step_status == Some(StepRunStatus::Skipped) }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Skipped);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandProjectFrameStepStatus {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == None
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(None)
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(Some(StepRunStatus::Dispatched))
            }
            guard "frame_node_completed_or_skipped" {
                self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Completed)
                || self.frame_node_status.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(NodeRunStatus::Skipped)
            }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(
                    self.run_step_status,
                    run_id,
                    step_id.get("value"),
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == None
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(None)
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(Some(StepRunStatus::Dispatched))
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
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
        }

        transition AuthorizeFlowRunReducerCommandProjectFrameStepStatusFailedEscalating {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "project_frame_step_status_command" { command == FlowRunReducerCommandKind::ProjectFrameStepStatus }
            guard "has_step_id" { step_id != None }
            guard "has_frame_id" { frame_id != None }
            guard "has_node_id" { node_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "frame_belongs_to_run" { self.frame_run.get_cloned(frame_id.get("value")) == Some(run_id) }
            guard "frame_node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id.get("value")).get("value").contains(node_id.get("value")) }
            guard "frame_node_maps_to_step" { self.frame_node_step_ids.get_cloned(frame_id.get("value")).get("value").get_cloned(node_id.get("value")) == Some(step_id.get("value")) }
            guard "run_step_not_already_terminal_projected" {
                self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == None
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(None)
                || self.run_step_status.get_cloned(run_id).get("value").get_cloned(step_id.get("value")) == Some(Some(StepRunStatus::Dispatched))
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
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Failed);
                self.run_failure_count.increment(run_id, 1);
                self.run_consecutive_failure_count.increment(run_id, 1);
            }
            to Running
            emit EmitRunLifecycleNotice
            emit AppendFailureLedger
            emit EscalateSupervisor
        }

        transition AuthorizeFlowRunReducerCommandCancelStep {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "cancel_step_command" { command == FlowRunReducerCommandKind::CancelStep }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            guard "canceled_step_status" { step_status == Some(StepRunStatus::Canceled) }
            update {
                self.run_step_status = mob_machine_run_step_status_after_set(self.run_step_status, run_id, step_id.get("value"), StepRunStatus::Canceled);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterTargets {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "register_targets_command" { command == FlowRunReducerCommandKind::RegisterTargets }
            guard "has_step_id" { step_id != None }
            guard "has_target_count" { target_count != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_counts = mob_machine_run_step_u64_after_set(self.run_step_target_counts, run_id, step_id.get("value"), target_count.get("value"));
                self.run_step_target_success_counts = mob_machine_run_step_u64_after_set(self.run_step_target_success_counts, run_id, step_id.get("value"), 0);
                self.run_step_target_terminal_failure_counts = mob_machine_run_step_u64_after_set(self.run_step_target_terminal_failure_counts, run_id, step_id.get("value"), 0);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetSuccess {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_success_command" { command == FlowRunReducerCommandKind::RecordTargetSuccess }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_success_counts = mob_machine_run_step_u64_after_increment(self.run_step_target_success_counts, run_id, step_id.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetTerminalFailure {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_terminal_failure_command" { command == FlowRunReducerCommandKind::RecordTargetTerminalFailure }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_step_target_terminal_failure_counts = mob_machine_run_step_u64_after_increment(self.run_step_target_terminal_failure_counts, run_id, step_id.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetCanceled {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_canceled_command" { command == FlowRunReducerCommandKind::RecordTargetCanceled }
            guard "has_step_id" { step_id != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {}
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRecordTargetFailure {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_run" { self.run_status.contains_key(run_id) == true }
            guard "run_running" { self.run_status.get_cloned(run_id) == Some(FlowRunStatus::Running) }
            guard "record_target_failure_command" { command == FlowRunReducerCommandKind::RecordTargetFailure }
            guard "has_step_id" { step_id != None }
            guard "has_retry_key" { retry_key != None }
            guard "step_tracked" { self.run_tracked_steps.get_cloned(run_id).get("value").contains(step_id.get("value")) }
            update {
                self.run_target_retry_counts = mob_machine_run_retry_count_after_increment(self.run_target_retry_counts, run_id, retry_key.get("value"), 1);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterReadyFrame {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
                self.run_last_granted_frame.insert(run_id, Some(frame_id.get("value")));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandRegisterPendingBodyFrame {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
                self.run_last_granted_loop.insert(run_id, Some(loop_instance_id.get("value")));
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowRunReducerCommandNodeExecutionReleased {
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowRunReducerCommand { run_id, command, step_id, step_status, target_count, frame_id, node_id, loop_instance_id, retry_key }
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
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
                self.frame_last_admitted_node.insert(frame_id, Some(node_id.get("value")));
                self.frame_ready_queue = mob_machine_frame_ready_queue_after_admit(self.frame_ready_queue, self.frame_node_status, self.frame_ordered_nodes, frame_id);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandCompleteNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
            guard { self.lifecycle_phase == Phase::Running || self.lifecycle_phase == Phase::Stopped || self.lifecycle_phase == Phase::Completed }
            guard "known_frame" { self.frame_phase.contains_key(frame_id) == true }
            guard "frame_running" { self.frame_phase.get_cloned(frame_id) == Some(FrameStatus::Running) }
            guard "record_node_output_command" { command == FlowFrameReducerCommandKind::RecordNodeOutput }
            guard "no_terminal_status" { terminal_status == None }
            guard "has_node_id" { node_id != None }
            guard "node_tracked" { self.frame_tracked_nodes.get_cloned(frame_id).get("value").contains(node_id.get("value")) }
            update {
                self.frame_output_recorded = mob_machine_frame_node_bool_after_set(self.frame_output_recorded, frame_id, node_id.get("value"), true);
            }
            to Running
            emit EmitRunLifecycleNotice
        }

        transition AuthorizeFlowFrameReducerCommandFailNode {
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
            per_phase [Running, Stopped, Completed]
            on input AuthorizeFlowFrameReducerCommand { frame_id, command, node_id, node_status, terminal_status }
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
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            guard "releasing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(releasing.get("value")) }
            guard "session_matches_releasing" { session_id == releasing }
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
                session_id: session_id
            }
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
        }

        transition RetireRunningPreservingBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
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
                session_id: session_id
            }
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
        }

        transition RetireRunningNoBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            guard "session_absent" { session_id == None }
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
                session_id: None
            }
        }

        transition RetireStoppedReleasing {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_present" { releasing != None }
            guard "releasing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(releasing.get("value")) }
            guard "session_matches_releasing" { session_id == releasing }
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
                session_id: session_id
            }
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
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
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "releasing_absent" { releasing == None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
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
                session_id: session_id
            }
            emit RequestRuntimeRetire { session_id: session_id.get("value") }
        }

        transition RetireStoppedNoBinding {
            on input Retire { mob_id, agent_runtime_id, agent_identity, generation, releasing, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "active_members_present" { self.live_runtime_ids != EmptySet }
            guard "runtime_id_present" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "releasing_absent" { releasing == None }
            guard "session_absent" { session_id == None }
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
                session_id: None
            }
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
                self.member_profile_names = EmptyMap;
                self.member_runtime_modes = EmptyMap;
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

        // =====================================================================
        // Mob coordination board transitions (folded from the former
        // standalone MobCoordinationLifecycleAuthorityMachine). MobMachine owns
        // the records; every conclusion is recomputed in-DSL from owned maps +
        // raw caller facts. No pre-reduced inputs.
        // =====================================================================

        // Admission: !exists && summary_present && metadata_public &&
        // draft_mob_id == authority_mob_id && resources non-empty && owner
        // present. Revision is the machine-owned initial revision (1); the
        // event sequence is the machine-owned monotonic cursor.
        transition RecordCoordinationWorkIntent {
            on input RecordCoordinationWorkIntent {
                intent_id,
                requested_status,
                owner_present,
                summary_present,
                metadata_public,
                draft_mob_id,
                authority_mob_id,
                resource_tokens,
                expires_at_ms,
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_is_new" { self.work_intent_status.contains_key(intent_id) == false }
            guard "summary_present" { summary_present == true }
            guard "metadata_public" { metadata_public == true }
            guard "owning_mob_ref_matches" { draft_mob_id == authority_mob_id }
            guard "resources_non_empty" { resource_tokens.len() > 0 }
            guard "owner_present" { owner_present == true }
            update {
                self.work_intent_status.insert(intent_id, requested_status);
                self.work_intent_revision.insert(intent_id, 1);
                self.work_intent_resources.insert(intent_id, resource_tokens);
                self.work_intent_owner_present.insert(intent_id, owner_present);
                self.work_intent_expires_at_ms.insert(intent_id, expires_at_ms);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentRecorded {
                intent_id: intent_id,
                status: requested_status,
                revision: 1,
                resource_tokens: resource_tokens,
                expires_at_ms: expires_at_ms,
                event_kind: MobCoordinationEventKind::WorkIntentRecorded,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition RecordCoordinationResourceClaim {
            on input RecordCoordinationResourceClaim {
                claim_id,
                requested_kind,
                requested_status,
                owner_present,
                metadata_public,
                draft_mob_id,
                authority_mob_id,
                resource_tokens,
                expires_at_ms,
            }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_is_new" { self.resource_claim_status.contains_key(claim_id) == false }
            guard "metadata_public" { metadata_public == true }
            guard "owning_mob_ref_matches" { draft_mob_id == authority_mob_id }
            guard "resources_non_empty" { resource_tokens.len() > 0 }
            guard "owner_present" { owner_present == true }
            update {
                self.resource_claim_status.insert(claim_id, requested_status);
                self.resource_claim_kind.insert(claim_id, requested_kind);
                self.resource_claim_revision.insert(claim_id, 1);
                self.resource_claim_resources.insert(claim_id, resource_tokens);
                self.resource_claim_owner_present.insert(claim_id, owner_present);
                self.resource_claim_expires_at_ms.insert(claim_id, expires_at_ms);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimRecorded {
                claim_id: claim_id,
                kind: requested_kind,
                status: requested_status,
                revision: 1,
                resource_tokens: resource_tokens,
                expires_at_ms: expires_at_ms,
                event_kind: MobCoordinationEventKind::ResourceClaimRecorded,
                sequence: self.coordination_event_next_sequence
            }
        }

        // Work intent status update: revision CAS against the machine-owned
        // stored revision; emit stored+1. Terminal targets (Completed,
        // Cancelled) are always legal; live targets (Planned, Active, Blocked)
        // require the record not be expired — the expiry conclusion is computed
        // in-DSL from the stored expiry + the raw now_ms input.
        transition UpdateCoordinationWorkIntentPlanned {
            on input UpdateCoordinationWorkIntentStatus { intent_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_present" { self.work_intent_status.contains_key(intent_id) }
            guard "revision_cas" {
                self.work_intent_revision.get_copied(intent_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_planned" { requested_status == MobCoordinationWorkIntentStatus::Planned }
            guard "not_expired" {
                mob_coordination_work_intent_unexpired(self.work_intent_expires_at_ms.get_cloned(intent_id).get("value"), now_ms)
            }
            update {
                self.work_intent_status.insert(intent_id, MobCoordinationWorkIntentStatus::Planned);
                self.work_intent_revision.insert(intent_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentStatusChanged {
                intent_id: intent_id,
                status: MobCoordinationWorkIntentStatus::Planned,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationWorkIntentActive {
            on input UpdateCoordinationWorkIntentStatus { intent_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_present" { self.work_intent_status.contains_key(intent_id) }
            guard "revision_cas" {
                self.work_intent_revision.get_copied(intent_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_active" { requested_status == MobCoordinationWorkIntentStatus::Active }
            guard "not_expired" {
                mob_coordination_work_intent_unexpired(self.work_intent_expires_at_ms.get_cloned(intent_id).get("value"), now_ms)
            }
            update {
                self.work_intent_status.insert(intent_id, MobCoordinationWorkIntentStatus::Active);
                self.work_intent_revision.insert(intent_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentStatusChanged {
                intent_id: intent_id,
                status: MobCoordinationWorkIntentStatus::Active,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationWorkIntentBlocked {
            on input UpdateCoordinationWorkIntentStatus { intent_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_present" { self.work_intent_status.contains_key(intent_id) }
            guard "revision_cas" {
                self.work_intent_revision.get_copied(intent_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_blocked" { requested_status == MobCoordinationWorkIntentStatus::Blocked }
            guard "not_expired" {
                mob_coordination_work_intent_unexpired(self.work_intent_expires_at_ms.get_cloned(intent_id).get("value"), now_ms)
            }
            update {
                self.work_intent_status.insert(intent_id, MobCoordinationWorkIntentStatus::Blocked);
                self.work_intent_revision.insert(intent_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentStatusChanged {
                intent_id: intent_id,
                status: MobCoordinationWorkIntentStatus::Blocked,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationWorkIntentCompleted {
            on input UpdateCoordinationWorkIntentStatus { intent_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_present" { self.work_intent_status.contains_key(intent_id) }
            guard "revision_cas" {
                self.work_intent_revision.get_copied(intent_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_completed" { requested_status == MobCoordinationWorkIntentStatus::Completed }
            update {
                self.work_intent_status.insert(intent_id, MobCoordinationWorkIntentStatus::Completed);
                self.work_intent_revision.insert(intent_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentStatusChanged {
                intent_id: intent_id,
                status: MobCoordinationWorkIntentStatus::Completed,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationWorkIntentCancelled {
            on input UpdateCoordinationWorkIntentStatus { intent_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "intent_present" { self.work_intent_status.contains_key(intent_id) }
            guard "revision_cas" {
                self.work_intent_revision.get_copied(intent_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_cancelled" { requested_status == MobCoordinationWorkIntentStatus::Cancelled }
            update {
                self.work_intent_status.insert(intent_id, MobCoordinationWorkIntentStatus::Cancelled);
                self.work_intent_revision.insert(intent_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit WorkIntentStatusChanged {
                intent_id: intent_id,
                status: MobCoordinationWorkIntentStatus::Cancelled,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::WorkIntentStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        // Resource claim status update: revision CAS; Released/Expired/Cancelled
        // are terminal targets (always legal); Active requires not expired.
        transition UpdateCoordinationResourceClaimActive {
            on input UpdateCoordinationResourceClaimStatus { claim_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_present" { self.resource_claim_status.contains_key(claim_id) }
            guard "revision_cas" {
                self.resource_claim_revision.get_copied(claim_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_active" { requested_status == MobCoordinationResourceClaimStatus::Active }
            guard "not_expired" {
                mob_coordination_resource_claim_unexpired(self.resource_claim_expires_at_ms.get_cloned(claim_id).get("value"), now_ms)
            }
            update {
                self.resource_claim_status.insert(claim_id, MobCoordinationResourceClaimStatus::Active);
                self.resource_claim_revision.insert(claim_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimStatusChanged {
                claim_id: claim_id,
                status: MobCoordinationResourceClaimStatus::Active,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::ResourceClaimStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationResourceClaimReleased {
            on input UpdateCoordinationResourceClaimStatus { claim_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_present" { self.resource_claim_status.contains_key(claim_id) }
            guard "revision_cas" {
                self.resource_claim_revision.get_copied(claim_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_released" { requested_status == MobCoordinationResourceClaimStatus::Released }
            update {
                self.resource_claim_status.insert(claim_id, MobCoordinationResourceClaimStatus::Released);
                self.resource_claim_revision.insert(claim_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimStatusChanged {
                claim_id: claim_id,
                status: MobCoordinationResourceClaimStatus::Released,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::ResourceClaimStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationResourceClaimExpired {
            on input UpdateCoordinationResourceClaimStatus { claim_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_present" { self.resource_claim_status.contains_key(claim_id) }
            guard "revision_cas" {
                self.resource_claim_revision.get_copied(claim_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_expired" { requested_status == MobCoordinationResourceClaimStatus::Expired }
            update {
                self.resource_claim_status.insert(claim_id, MobCoordinationResourceClaimStatus::Expired);
                self.resource_claim_revision.insert(claim_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimStatusChanged {
                claim_id: claim_id,
                status: MobCoordinationResourceClaimStatus::Expired,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::ResourceClaimStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        transition UpdateCoordinationResourceClaimCancelled {
            on input UpdateCoordinationResourceClaimStatus { claim_id, expected_revision, requested_status, now_ms }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_present" { self.resource_claim_status.contains_key(claim_id) }
            guard "revision_cas" {
                self.resource_claim_revision.get_copied(claim_id) == Some(expected_revision)
                && expected_revision > 0
            }
            guard "target_is_cancelled" { requested_status == MobCoordinationResourceClaimStatus::Cancelled }
            update {
                self.resource_claim_status.insert(claim_id, MobCoordinationResourceClaimStatus::Cancelled);
                self.resource_claim_revision.insert(claim_id, expected_revision + 1);
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimStatusChanged {
                claim_id: claim_id,
                status: MobCoordinationResourceClaimStatus::Cancelled,
                revision: expected_revision + 1,
                event_kind: MobCoordinationEventKind::ResourceClaimStatusChanged,
                sequence: self.coordination_event_next_sequence
            }
        }

        // Overlap observation. The caller proposes `candidate_overlap_ids`;
        // MobMachine REVALIDATES it against its own state and rejects any wrong
        // set. The proposed set is exactly the active resource claims (other
        // than the target) whose affected resources intersect the target's.
        //   (a) every proposed id is a DIFFERENT, present, active claim whose
        //       resources intersect the target's;
        //   (b) no omitted present active claim (other than the target) overlaps
        //       the target — i.e. every such claim must appear in the proposal.
        transition ObserveCoordinationResourceClaimOverlap {
            on input ObserveCoordinationResourceClaimOverlap { claim_id, now_ms, candidate_overlap_ids }
            guard { self.lifecycle_phase == Phase::Running }
            guard "claim_present" { self.resource_claim_status.contains_key(claim_id) }
            guard "candidates_are_valid_overlaps" {
                for_all(cid in candidate_overlap_ids,
                    cid != claim_id
                    && self.resource_claim_status.contains_key(cid)
                    && mob_coordination_resource_claim_active_at(
                        self.resource_claim_status.get_cloned(cid).get("value"),
                        self.resource_claim_expires_at_ms.get_cloned(cid).get("value"),
                        now_ms)
                    && exists(r in self.resource_claim_resources.get_cloned(cid).get("value"),
                        self.resource_claim_resources.get_cloned(claim_id).get("value").contains(r)))
            }
            guard "no_omitted_overlap" {
                for_all(other in self.resource_claim_status.keys(),
                    other == claim_id
                    || mob_coordination_resource_claim_inactive_at(
                        self.resource_claim_status.get_cloned(other).get("value"),
                        self.resource_claim_expires_at_ms.get_cloned(other).get("value"),
                        now_ms)
                    || !exists(r in self.resource_claim_resources.get_cloned(other).get("value"),
                        self.resource_claim_resources.get_cloned(claim_id).get("value").contains(r))
                    || candidate_overlap_ids.contains(other))
            }
            update {
                self.coordination_event_next_sequence += 1;
            }
            to Running
            emit ResourceClaimOverlapObserved {
                claim_id: claim_id,
                overlap_ids: candidate_overlap_ids,
                event_kind: MobCoordinationEventKind::ResourceClaimOverlapObserved,
                sequence: self.coordination_event_next_sequence
            }
        }

    }
        }

        impl MobMachineAuthority {
        fn mob_machine_identity_has_session_binding(
            member_session_bindings: &std::collections::BTreeMap<AgentIdentity, SessionId>,
            agent_identity: &AgentIdentity,
        ) -> bool {
            member_session_bindings.contains_key(agent_identity)
        }

        fn mob_machine_session_bound_live_runtime_ids_match(
            identity_to_runtime: &std::collections::BTreeMap<AgentIdentity, AgentRuntimeId>,
            member_session_bindings: &std::collections::BTreeMap<AgentIdentity, SessionId>,
            live_runtime_ids: &std::collections::BTreeSet<AgentRuntimeId>,
            expected_runtime_ids: &std::collections::BTreeSet<AgentRuntimeId>,
        ) -> bool {
            let actual_runtime_ids = member_session_bindings
                .iter()
                .filter_map(|(identity, _)| identity_to_runtime.get(identity))
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

        fn mob_machine_external_peer_identity_absent(
            edges: &std::collections::BTreeSet<ExternalPeerEdge>,
            agent_identity: &AgentIdentity,
        ) -> bool {
            edges
                .iter()
                .all(|edge| edge.endpoint.name.0 != agent_identity.0)
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

        fn mob_machine_external_peer_endpoint_as_member(
            endpoint: &ExternalPeerEndpoint,
        ) -> MemberPeerEndpoint {
            MemberPeerEndpoint {
                name: endpoint.name.clone(),
                peer_id: endpoint.peer_id.clone(),
                address: endpoint.address.clone(),
                signing_key: endpoint.signing_key.clone(),
            }
        }

        fn mob_machine_member_peer_overlay_complete(
            wiring_edges: &std::collections::BTreeSet<WiringEdge>,
            member_peer_endpoints: &std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,
            agent_identity: &AgentIdentity,
        ) -> bool {
            wiring_edges.iter().all(|edge| {
                if edge.a == *agent_identity {
                    member_peer_endpoints.contains_key(&edge.b)
                } else if edge.b == *agent_identity {
                    member_peer_endpoints.contains_key(&edge.a)
                } else {
                    true
                }
            })
        }

        fn mob_machine_member_peer_overlay(
            wiring_edges: &std::collections::BTreeSet<WiringEdge>,
            member_peer_endpoints: &std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,
            external_peer_edges: &std::collections::BTreeSet<ExternalPeerEdge>,
            agent_identity: &AgentIdentity,
        ) -> std::collections::BTreeSet<MemberPeerEndpoint> {
            let mut endpoints = std::collections::BTreeSet::new();
            for edge in wiring_edges {
                let peer_identity = if edge.a == *agent_identity {
                    Some(&edge.b)
                } else if edge.b == *agent_identity {
                    Some(&edge.a)
                } else {
                    None
                };
                if let Some(peer_identity) = peer_identity
                    && let Some(endpoint) = member_peer_endpoints.get(peer_identity)
                {
                    endpoints.insert(endpoint.clone());
                }
            }
            for edge in external_peer_edges {
                if edge.local == *agent_identity {
                    endpoints.insert(Self::mob_machine_external_peer_endpoint_as_member(
                        &edge.endpoint,
                    ));
                }
            }
            endpoints
        }

        fn mob_machine_member_peer_overlay_peer_ids_unique(
            wiring_edges: &std::collections::BTreeSet<WiringEdge>,
            member_peer_endpoints: &std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,
            external_peer_edges: &std::collections::BTreeSet<ExternalPeerEdge>,
            agent_identity: &AgentIdentity,
        ) -> bool {
            let mut peer_ids = std::collections::BTreeSet::new();
            Self::mob_machine_member_peer_overlay(
                wiring_edges,
                member_peer_endpoints,
                external_peer_edges,
                agent_identity,
            )
            .into_iter()
            .all(|endpoint| peer_ids.insert(endpoint.peer_id))
        }

        fn mob_machine_wiring_edge_matches_members(
            edge: &WiringEdge,
            a_identity: &AgentIdentity,
            b_identity: &AgentIdentity,
        ) -> bool {
            edge.a == *a_identity && edge.b == *b_identity
        }

        fn mob_machine_run_step_status_after_set(
            all_statuses: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, Option<StepRunStatus>>>,
            run_id: &RunId,
            step_id: &StepId,
            status: &StepRunStatus,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, Option<StepRunStatus>>> {
            let mut all_statuses = all_statuses.clone();
            all_statuses
                .entry(run_id.clone())
                .or_default()
                .insert(step_id.clone(), Some(*status));
            all_statuses
        }

        fn mob_machine_run_step_bool_after_set(
            all_values: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, bool>>,
            run_id: &RunId,
            step_id: &StepId,
            value: &bool,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, bool>> {
            let mut all_values = all_values.clone();
            all_values
                .entry(run_id.clone())
                .or_default()
                .insert(step_id.clone(), *value);
            all_values
        }

        fn mob_machine_run_step_condition_result_after_set(
            all_results: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, Option<bool>>>,
            run_id: &RunId,
            step_id: &StepId,
            result: &bool,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, Option<bool>>> {
            let mut all_results = all_results.clone();
            all_results
                .entry(run_id.clone())
                .or_default()
                .insert(step_id.clone(), Some(*result));
            all_results
        }

        fn mob_machine_run_step_u64_after_set(
            all_counts: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, u64>>,
            run_id: &RunId,
            step_id: &StepId,
            value: &u64,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, u64>> {
            let mut all_counts = all_counts.clone();
            all_counts
                .entry(run_id.clone())
                .or_default()
                .insert(step_id.clone(), *value);
            all_counts
        }

        fn mob_machine_run_step_u64_after_increment(
            all_counts: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, u64>>,
            run_id: &RunId,
            step_id: &StepId,
            delta: &u64,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<StepId, u64>> {
            let mut all_counts = all_counts.clone();
            let count = all_counts
                .entry(run_id.clone())
                .or_default()
                .entry(step_id.clone())
                .or_default();
            *count = count.saturating_add(*delta);
            all_counts
        }

        fn mob_machine_run_retry_count_after_increment(
            all_counts: &std::collections::BTreeMap<RunId, std::collections::BTreeMap<String, u64>>,
            run_id: &RunId,
            retry_key: &str,
            delta: &u64,
        ) -> std::collections::BTreeMap<RunId, std::collections::BTreeMap<String, u64>> {
            let mut all_counts = all_counts.clone();
            let count = all_counts
                .entry(run_id.clone())
                .or_default()
                .entry(retry_key.to_owned())
                .or_default();
            *count = count.saturating_add(*delta);
            all_counts
        }

        fn mob_machine_frame_node_bool_after_set(
            all_values: &std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, bool>>,
            frame_id: &FrameId,
            node_id: &FlowNodeId,
            value: &bool,
        ) -> std::collections::BTreeMap<FrameId, std::collections::BTreeMap<FlowNodeId, bool>> {
            let mut all_values = all_values.clone();
            all_values
                .entry(frame_id.clone())
                .or_default()
                .insert(node_id.clone(), *value);
            all_values
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

        // -----------------------------------------------------------------
        // Mob coordination expiry/active classification (folded). These are
        // pure structural functions over RAW machine-owned facts (stored
        // expiry timestamp + the caller's raw `now_ms` clock reading). They
        // do not accept any pre-reduced `is_expired` input; the expiry
        // conclusion is computed here from `Some(e) && e <= now_ms`.
        // -----------------------------------------------------------------
        fn mob_coordination_expired_at(expires_at_ms: &Option<u64>, now_ms: &u64) -> bool {
            matches!(expires_at_ms, Some(expiry) if *expiry <= *now_ms)
        }

        fn mob_coordination_work_intent_unexpired(expires_at_ms: &Option<u64>, now_ms: &u64) -> bool {
            !Self::mob_coordination_expired_at(expires_at_ms, now_ms)
        }

        fn mob_coordination_resource_claim_unexpired(
            expires_at_ms: &Option<u64>,
            now_ms: &u64,
        ) -> bool {
            !Self::mob_coordination_expired_at(expires_at_ms, now_ms)
        }

        // A resource claim is active at `now_ms` iff its status is `Active`
        // (the only non-terminal status) and it is not expired. The expired
        // classification matches the former board, which treats both the
        // `Expired` status and an elapsed `expires_at` as expired; since the
        // status arm already requires `Active`, only the timestamp matters.
        fn mob_coordination_resource_claim_active_at(
            status: &MobCoordinationResourceClaimStatus,
            expires_at_ms: &Option<u64>,
            now_ms: &u64,
        ) -> bool {
            matches!(status, MobCoordinationResourceClaimStatus::Active)
                && !Self::mob_coordination_expired_at(expires_at_ms, now_ms)
        }

        fn mob_coordination_resource_claim_inactive_at(
            status: &MobCoordinationResourceClaimStatus,
            expires_at_ms: &Option<u64>,
            now_ms: &u64,
        ) -> bool {
            !Self::mob_coordination_resource_claim_active_at(status, expires_at_ms, now_ms)
        }
        }
    };
}

crate::mob_catalog_machine_dsl!("self", "catalog::dsl::mob_machine");

pub type MobToolCallerProvenance = meerkat_core::service::MobToolCallerProvenance;
pub type OpaquePrincipalToken = meerkat_core::service::OpaquePrincipalToken;

// ---------------------------------------------------------------------------
// Mob coordination board bridging newtypes / enums (folded). These mirror the
// product-neutral coordination domain types and let the DSL Set/Map machinery
// treat them as opaque ordered keys/values.
// ---------------------------------------------------------------------------

/// Stable identifier for a product-neutral mob work intent.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct WorkIntentId(pub String);

/// Stable identifier for a mob resource claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ResourceClaimId(pub String);

/// Product-neutral reference to a resource affected by mob coordination.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct CoordinationResourceRef(pub String);

/// Work-intent lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationWorkIntentStatus {
    #[default]
    Planned,
    Active,
    Blocked,
    Completed,
    Cancelled,
}

/// Resource-claim lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimStatus {
    #[default]
    Active,
    Released,
    Expired,
    Cancelled,
}

/// Resource-claim advisory strength.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationResourceClaimKind {
    #[default]
    Advisory,
    SoftReservation,
    Exclusive,
}

/// Coordination event discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCoordinationEventKind {
    #[default]
    WorkIntentRecorded,
    WorkIntentStatusChanged,
    ResourceClaimRecorded,
    ResourceClaimStatusChanged,
    ResourceClaimOverlapObserved,
}

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

/// Typed public result class for surfaces that expose terminal flow-run
/// completion. MobMachine owns whether a terminal flow status maps to a
/// success envelope or an error envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum FlowRunPublicResultClassKind {
    #[default]
    Failure,
    Success,
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

/// Typed public wait-admission result for member waits. MobMachine emits this
/// class before wait surfaces decide whether an absent runtime-material
/// snapshot is a hard failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberWaitClassificationKind {
    #[default]
    RuntimeMaterialPresent,
    MissingRuntimeMaterial,
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

/// Typed shell observation for a per-row `mob/spawn_many` failure.
/// MobMachine maps this observation to the public failure cause before any
/// surface can serialize the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnManyFailureObservationKind {
    #[default]
    ProfileNotFound,
    MemberNotFound,
    MemberAlreadyExists,
    NotExternallyAddressable,
    InvalidTransition,
    WiringError,
    SupervisorRotationIncomplete,
    BridgeCommandRejected,
    MemberRestoreFailed,
    KickoffWaitTimedOut,
    ReadyWaitTimedOut,
    DefinitionError,
    FlowNotFound,
    FlowFailed,
    RunNotFound,
    RunCanceled,
    FlowTurnTimedOut,
    FrameDepthLimitExceeded,
    FrameAtomicPersistenceUnavailable,
    SpecRevisionConflict,
    SchemaValidation,
    InsufficientTargets,
    TopologyViolation,
    BridgeDeliveryRejected,
    SupervisorEscalation,
    UnsupportedForMode,
    MissingMemberCapability,
    ResetBarrier,
    StorageError,
    SessionError,
    CommsError,
    CallbackPending,
    StaleFenceToken,
    StaleEventCursor,
    WorkNotFound,
    Internal,
}

/// Typed public result class for per-row `mob/spawn_many` failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnManyFailureCauseKind {
    #[default]
    ProfileNotFound,
    MemberNotFound,
    MemberAlreadyExists,
    NotExternallyAddressable,
    InvalidTransition,
    WiringError,
    BridgeCommandRejected,
    MemberRestoreFailed,
    KickoffWaitTimedOut,
    ReadyWaitTimedOut,
    DefinitionError,
    FlowNotFound,
    FlowFailed,
    RunNotFound,
    RunCanceled,
    FlowTurnTimedOut,
    FrameDepthLimitExceeded,
    FrameAtomicPersistenceUnavailable,
    SpecRevisionConflict,
    SchemaValidation,
    InsufficientTargets,
    TopologyViolation,
    BridgeDeliveryRejected,
    SupervisorEscalation,
    UnsupportedForMode,
    MissingMemberCapability,
    ResetBarrier,
    StorageError,
    SessionError,
    CommsError,
    CallbackPending,
    StaleFenceToken,
    StaleEventCursor,
    WorkNotFound,
    Internal,
}

/// Pure flow-topology rule-match verdict. The shell extracts this from the
/// declarative `TopologyRules` via `evaluate_topology` and feeds it to
/// MobMachine as an observation; MobMachine — not the shell — derives the
/// admission verdict from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeRuleVerdictKind {
    #[default]
    Allow,
    Deny,
}

/// Configured topology enforcement mode for a flow delegation edge. Mirrors
/// the shell `PolicyMode`; fed alongside the rule verdict so MobMachine can
/// decide whether a denial blocks (Strict) or merely warns (Advisory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeModeKind {
    #[default]
    Advisory,
    Strict,
}

/// Machine-owned admission verdict for a flow delegation edge. The shell
/// mirrors this: `DeniedStrict` blocks the delegation step
/// (`MobError::TopologyViolation`), `DeniedAdvisory` emits an advisory notice
/// and proceeds, `Admitted` proceeds silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFlowDelegationEdgeAdmissionKind {
    #[default]
    Admitted,
    DeniedStrict,
    DeniedAdvisory,
}

/// Pure observation of a remote member's runtime state, extracted by the bridge
/// consumer from the wire `BridgeMemberRuntimeState` projection and fed to
/// MobMachine for terminality classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobRemoteMemberRuntimeObservedState {
    #[default]
    Initializing,
    Idle,
    Attached,
    Running,
    Retired,
    Stopped,
    Destroyed,
}

/// Machine-owned terminality verdict for an observed remote-member runtime
/// state. The bridge shell mirrors this: `Terminal` lets cleanup stop,
/// `NonTerminal` forces a destroy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobRemoteMemberRuntimeTerminality {
    #[default]
    NonTerminal,
    Terminal,
}

/// Machine-owned composite spawn-member operator admission verdict. The tool
/// shell mirrors this: `Denied` -> `access_denied`, `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnMemberAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned per-mob operator admission verdict for current-mob-scoped
/// tools. The tool shell mirrors this: `Denied` -> `access_denied`, `Allowed`
/// -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCurrentMobAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned coarse spawn-tool admission verdict for the spawn-member tool
/// surfaces. The tool shell mirrors this: `Denied` -> `access_denied`,
/// `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobSpawnToolAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned operator create-mob admission verdict for the mob-creation
/// tool. The tool shell mirrors this: `Denied` -> `access_denied`, `Allowed`
/// -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobCreateMobAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned operator profile-mutation admission verdict for realm-profile
/// mutation tools. The tool shell mirrors this: `Denied` -> `access_denied`,
/// `Allowed` -> proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobProfileMutationAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

/// Machine-owned eligibility verdict for a within-mob member operation (spawn
/// finalization, peer messaging, respawn finalization) that requires the mob to
/// be live and running. MobMachine — not a handwritten shell phase pre-check —
/// owns the eligibility over its own lifecycle phase plus the `destroy_admitted`
/// projection marker: an operation is `Admitted` iff the machine is `Running`
/// and destruction has not been admitted, and `DeniedNotRunning` otherwise. The
/// owning actor mirrors `DeniedNotRunning` to the same `InvalidTransition`
/// rejection it previously produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobMemberOperationEligibilityKind {
    #[default]
    DeniedNotRunning,
    Admitted,
}

/// Pure wire-projection bridge rejection cause, mirroring every variant of the
/// wire `BridgeRejectionCause`. The mob bridge consumer maps the typed wire
/// cause onto this and feeds it to MobMachine for recovery classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobBridgeRejectionCause {
    #[default]
    NotBound,
    StaleSupervisor,
    SenderMismatch,
    AlreadyBound,
    InvalidBootstrapToken,
    UnsupportedProtocolVersion,
    InvalidSupervisorSpec,
    InvalidPeerSpec,
    AddressMismatch,
    Unsupported,
    Internal,
}

/// Machine-owned bridge-rejection recovery verdict. The mob shell mirrors this:
/// `RebindRecover` re-runs `BindMember`, `FatalBubbleUp` bubbles the rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobBridgeRejectionRecovery {
    #[default]
    FatalBubbleUp,
    RebindRecover,
}

/// Machine-owned pending-supervisor-acceptance verdict for a re-verified
/// already-accepted remote peer during supervisor rotation. The actor mirrors
/// this: `NotConfirmedReattempt` drops the accepted peer and re-attempts the
/// rotation against it; `StalePendingAuthority` errors with the stale-pending
/// message; `Fatal` bubbles the rejection up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobPendingSupervisorAcceptanceKind {
    #[default]
    Fatal,
    NotConfirmedReattempt,
    StalePendingAuthority,
}

/// Machine-owned frame-seed idempotency disposition. `CreateFrameSeed` emits
/// `Seeded` for a fresh seed and `AlreadySeeded` (a no-op) when re-seeding an
/// already-tracked frame. The flow shell mirrors both as success — replacing
/// the former guard-name string match (`frame_seed_is_new`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MobFrameSeedDisposition {
    #[default]
    Seeded,
    AlreadySeeded,
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
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SupervisorProtocolVersion(pub String);
impl From<String> for SupervisorProtocolVersion {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl From<&str> for SupervisorProtocolVersion {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
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
