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
            member_kickoff_pending: Set<AgentIdentity>,
            member_kickoff_starting: Set<AgentIdentity>,
            member_kickoff_callback_pending: Set<AgentIdentity>,
            member_kickoff_started: Set<AgentIdentity>,
            member_kickoff_failed: Set<AgentIdentity>,
            member_kickoff_cancelled: Set<AgentIdentity>,
            member_kickoff_error: Map<AgentIdentity, String>,
            member_kickoff_objective_ids: Map<AgentIdentity, String>,
            objective_owner_ids: Map<String, AgentIdentity>,
            objective_outcomes: Map<String, String>,
            concluded_objective_ids: Set<String>,
            // Identity-level restore failures are lifecycle facts owned by
            // MobMachine. Projection code may surface the reason, but the
            // Broken/terminal classification comes from this map.
            member_restore_failures: Map<AgentIdentity, String>,
            // Stable consumer refusal code paired with a restore failure that
            // specifically came from the generated runtime-binding route.
            // The reason remains display detail; this map preserves the
            // typed code without parsing that detail.
            member_restore_failure_codes: Map<AgentIdentity, String>,
            // Runtime retirement is a durable pending obligation from
            // admission until archival completion. Consumer-refusal code and
            // detail are diagnostic facts; the session correlation is broader
            // than a refusal and survives restart so the route can be retried.
            runtime_retire_refusal_codes: Map<AgentRuntimeId, String>,
            runtime_retire_refusal_reasons: Map<AgentRuntimeId, String>,
            runtime_retire_pending_sessions: Map<AgentRuntimeId, SessionId>,
            // Durable intermediate proof for peer-only/external runtimes: the
            // remote runtime has acknowledged retirement, but supervisor
            // revocation and terminal member archival may still be pending.
            // This exact runtime-scoped marker prevents a cold retry from
            // re-authorizing or re-retiring an already-terminal remote owner.
            remote_runtime_retired_ids: Set<AgentRuntimeId>,
            // Durable second-stage proof for peer-only/external runtimes: the
            // exact supervisor binding has acknowledged revocation. Once this
            // marker exists, retry is purely local (recipient-trust cleanup +
            // terminal journal publication) and must never depend on the
            // intentionally retired remote runtime remaining reachable.
            remote_supervisor_revoked_ids: Set<AgentRuntimeId>,
            // Machine-owned revival obligation: members whose live
            // materialization was observed missing while a durable session
            // snapshot still exists. Inserted by the revivable arm of
            // ClassifyMemberLiveMaterialization; cleared by the
            // ResolveMemberRevival* resolutions and by every lifecycle
            // transition that replaces or clears the member's restore
            // classification. The session-registry cache never owns this fact.
            member_revival_pending: Set<AgentIdentity>,
            // Machine-owned member execution-health projection. The shell
            // supplies raw execution snapshots; MobMachine detects progress
            // token changes, owns the last-progress timestamp, and classifies
            // degradation/wedging from elapsed time.
            member_run_open: Map<AgentIdentity, bool>,
            member_in_flight_work: Map<AgentIdentity, u64>,
            member_progress_tokens: Map<AgentIdentity, String>,
            // Highest wall-clock observation accepted for this identity. Raw
            // clocks may move backwards; stale samples are ignored so neither
            // progress time nor machine-owned health can regress.
            member_last_observed_at_ms: Map<AgentIdentity, u64>,
            member_last_progress_at_ms: Map<AgentIdentity, u64>,
            member_last_progress_event: Map<AgentIdentity, Enum<MemberProgressEventKind>>,
            member_health_class: Map<AgentIdentity, Enum<MemberHealthClass>>,
            // --- Spawn-execution phase ladder (machine owns the phase ORDER) ---
            // A lightweight per-identity phase ladder: `BeginSpawnExec` opens
            // the window (Opened), `CommitSpawnMembership` commits the membership
            // facts (Opened -> MembershipCommitted), `CommitSpawnActivation`
            // closes it (-> Settled, the entry is removed). The shell still runs
            // each individual finalization step; the machine only OWNS + VALIDATES
            // the phase order (Begin before Commit before Activate). No
            // per-obligation tracking and no per-step request effects (those
            // exploded the mob x meerkat composition state space). Absence of an
            // entry == Settled (no spawn in flight), making every late-arrival /
            // post-teardown input total.
            spawn_exec_phase: Map<AgentIdentity, Enum<SpawnExecPhase>>,
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
            // Stable mob-wide operation identity for the in-flight supervisor
            // rotation. This is recorded before the first remote submission so
            // caller retries observe/re-submit the same operation rather than
            // inferring mutation truth from a bridge reply.
            supervisor_pending_authority_operation_id: Option<String>,
            supervisor_pending_authority_accepted_peer_ids: Set<PeerId>,
            supervisor_pending_authority_member_target_names: Map<PeerId, String>,
            supervisor_pending_authority_member_target_addresses: Map<PeerId, String>,
            // Machine-owned trust-install-before-authorization-terminality
            // window (dogma row R044). The comms router requires recipient
            // trust before a bridge command can be routed, so the shell must
            // install trust ahead of the `AuthorizeSupervisor`/`BindMember`
            // terminality verdict. Every such window is recorded here as an
            // explicit obligation: recorded before trust install, removed by
            // `ResolvePendingRecipientTrust` on confirmed-accept terminality
            // or by `RollbackPendingRecipientTrust` after the shell rolls the
            // installed trust back on a failure path. Volatile like the trust
            // it tracks (no wall-clock state; trivially recoverable as empty).
            pending_recipient_trust: Set<PeerId>,
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
            // Generation-scoped endpoint history retained when durable
            // session-head recovery rotates a member's comms identity without
            // changing its runtime generation. The current endpoint is never
            // present in this set. Historical endpoints authorize exact,
            // reciprocal stale-trust cleanup on still-wired member edges and
            // are cleared whenever the generation is replaced or terminal.
            member_prior_peer_endpoints: Map<AgentIdentity, Set<MemberPeerEndpoint>>,
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
            // Wave-G1 catalog folds.
            // Row #293: declared default topology-edge policy. Applied in-DSL
            // for edges with no matching declarative rule. Defaults to Allow to
            // preserve the former shell `map_or(Allow, ..)` behavior.
            topology_default_policy: Enum<PolicyDecision>,
            // Row #314: per-identity external-member rebind capability. Minted
            // by the bridge/spawn chokepoints; read by
            // project_external_member_observation instead of deriving from
            // roster bootstrap_token presence.
            external_member_rebind_capability: Map<AgentIdentity, Enum<ExternalMemberRebindCapability>>,
            // Row #320: orphan budget for detached turns. Seeded once from
            // `definition.limits.max_orphaned_turns` via `SeedOrphanBudget`.
            // A non-retryable timeout detaches while budget > 0 (decrementing
            // it), otherwise cancels.
            orphan_budget: u64,
            // Row #351: machine-owned membership intent. `ReconcileRunning`
            // recomputes the spawn/retire diff against `identity_to_runtime`
            // into these sets; the handle reads them and executes mechanically.
            desired_members: Set<AgentIdentity>,
            members_to_spawn: Set<AgentIdentity>,
            members_to_retire: Set<AgentIdentity>,
            adaptive_active_run: Option<AdaptiveRunId>,
            adaptive_run_phase: Map<AdaptiveRunId, Enum<AdaptiveRunPhase>>,
            adaptive_stop_reason: Map<AdaptiveRunId, Enum<AdaptiveStopReason>>,
            adaptive_limit_max_depth: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_total_decisions: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_repair_attempts: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_layer_failures: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_attempts_per_layer: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_members_per_layer: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_total_spawned_members: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_active_members: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_retained_layer_mobs: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_aggregate_tokens: Map<AdaptiveRunId, u64>,
            adaptive_limit_max_aggregate_tool_calls: Map<AdaptiveRunId, u64>,
            adaptive_limit_allowed_model_classes: Map<AdaptiveRunId, Set<String>>,
            adaptive_limit_allowed_tool_classes: Map<AdaptiveRunId, Set<String>>,
            adaptive_limit_allowed_skill_identities: Map<AdaptiveRunId, Set<String>>,
            adaptive_limit_allowed_auth_binding_refs: Map<AdaptiveRunId, Set<String>>,
            adaptive_deadline_ms: Map<AdaptiveRunId, u64>,
            adaptive_depth: Map<AdaptiveRunId, u64>,
            adaptive_total_decisions: Map<AdaptiveRunId, u64>,
            adaptive_repair_attempts: Map<AdaptiveRunId, u64>,
            adaptive_layer_failures: Map<AdaptiveRunId, u64>,
            adaptive_total_spawned_members: Map<AdaptiveRunId, u64>,
            adaptive_active_members: Map<AdaptiveRunId, u64>,
            adaptive_retained_layer_mobs: Map<AdaptiveRunId, u64>,
            adaptive_aggregate_token_reserved: Map<AdaptiveRunId, u64>,
            adaptive_aggregate_token_actual: Map<AdaptiveRunId, u64>,
            adaptive_aggregate_tool_call_reserved: Map<AdaptiveRunId, u64>,
            adaptive_aggregate_tool_call_actual: Map<AdaptiveRunId, u64>,
            adaptive_active_layer: Map<AdaptiveRunId, AdaptiveLayerId>,
            adaptive_layer_adaptive_run: Map<AdaptiveLayerId, AdaptiveRunId>,
            adaptive_layer_phase: Map<AdaptiveLayerId, Enum<AdaptiveLayerPhase>>,
            adaptive_layer_attempt: Map<AdaptiveLayerId, u64>,
            adaptive_layer_member_count: Map<AdaptiveLayerId, u64>,
            adaptive_layer_plan_digest: Map<AdaptiveLayerId, String>,
            adaptive_layer_child_mob_id: Map<AdaptiveLayerId, MobId>,
            adaptive_layer_token_reservation: Map<AdaptiveLayerId, u64>,
            adaptive_layer_tool_call_reservation: Map<AdaptiveLayerId, u64>,
            adaptive_layer_run_id: Map<AdaptiveLayerId, RunId>,
            adaptive_layer_result_digest: Map<AdaptiveLayerId, String>,
            adaptive_layer_fault: Map<AdaptiveLayerId, Enum<AdaptiveLayerSetupFaultKind>>,
            adaptive_layer_disposition: Map<AdaptiveLayerId, Enum<AdaptiveLayerDispositionKind>>,
            adaptive_missing_body_digest: Map<AdaptiveRunId, String>,
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
            member_kickoff_objective_ids = EmptyMap,
            objective_owner_ids = EmptyMap,
            objective_outcomes = EmptyMap,
            concluded_objective_ids = EmptySet,
            member_restore_failures = EmptyMap,
            member_restore_failure_codes = EmptyMap,
            runtime_retire_refusal_codes = EmptyMap,
            runtime_retire_refusal_reasons = EmptyMap,
            runtime_retire_pending_sessions = EmptyMap,
            remote_runtime_retired_ids = EmptySet,
            remote_supervisor_revoked_ids = EmptySet,
            member_revival_pending = EmptySet,
            member_run_open = EmptyMap,
            member_in_flight_work = EmptyMap,
            member_progress_tokens = EmptyMap,
            member_last_observed_at_ms = EmptyMap,
            member_last_progress_at_ms = EmptyMap,
            member_last_progress_event = EmptyMap,
            member_health_class = EmptyMap,
            spawn_exec_phase = EmptyMap,
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
            supervisor_pending_authority_operation_id = None,
            supervisor_pending_authority_accepted_peer_ids = EmptySet,
            supervisor_pending_authority_member_target_names = EmptyMap,
            supervisor_pending_authority_member_target_addresses = EmptyMap,
            pending_recipient_trust = EmptySet,
            owner_bridge_session_id = None,
            owner_bridge_destroy_on_archive = false,
            implicit_delegation_mob = false,
            identity_to_runtime = EmptyMap,
            member_session_bindings = EmptyMap,
            member_profile_names = EmptyMap,
            member_runtime_modes = EmptyMap,
            member_peer_ids = EmptyMap,
            member_peer_endpoints = EmptyMap,
            member_prior_peer_endpoints = EmptyMap,
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
            topology_default_policy = PolicyDecision::Allow,
            external_member_rebind_capability = EmptyMap,
            orphan_budget = 0,
            desired_members = EmptySet,
            members_to_spawn = EmptySet,
            members_to_retire = EmptySet,
            adaptive_active_run = None,
            adaptive_run_phase = EmptyMap,
            adaptive_stop_reason = EmptyMap,
            adaptive_limit_max_depth = EmptyMap,
            adaptive_limit_max_total_decisions = EmptyMap,
            adaptive_limit_max_repair_attempts = EmptyMap,
            adaptive_limit_max_layer_failures = EmptyMap,
            adaptive_limit_max_attempts_per_layer = EmptyMap,
            adaptive_limit_max_members_per_layer = EmptyMap,
            adaptive_limit_max_total_spawned_members = EmptyMap,
            adaptive_limit_max_active_members = EmptyMap,
            adaptive_limit_max_retained_layer_mobs = EmptyMap,
            adaptive_limit_max_aggregate_tokens = EmptyMap,
            adaptive_limit_max_aggregate_tool_calls = EmptyMap,
            adaptive_limit_allowed_model_classes = EmptyMap,
            adaptive_limit_allowed_tool_classes = EmptyMap,
            adaptive_limit_allowed_skill_identities = EmptyMap,
            adaptive_limit_allowed_auth_binding_refs = EmptyMap,
            adaptive_deadline_ms = EmptyMap,
            adaptive_depth = EmptyMap,
            adaptive_total_decisions = EmptyMap,
            adaptive_repair_attempts = EmptyMap,
            adaptive_layer_failures = EmptyMap,
            adaptive_total_spawned_members = EmptyMap,
            adaptive_active_members = EmptyMap,
            adaptive_retained_layer_mobs = EmptyMap,
            adaptive_aggregate_token_reserved = EmptyMap,
            adaptive_aggregate_token_actual = EmptyMap,
            adaptive_aggregate_tool_call_reserved = EmptyMap,
            adaptive_aggregate_tool_call_actual = EmptyMap,
            adaptive_active_layer = EmptyMap,
            adaptive_layer_adaptive_run = EmptyMap,
            adaptive_layer_phase = EmptyMap,
            adaptive_layer_attempt = EmptyMap,
            adaptive_layer_member_count = EmptyMap,
            adaptive_layer_plan_digest = EmptyMap,
            adaptive_layer_child_mob_id = EmptyMap,
            adaptive_layer_token_reservation = EmptyMap,
            adaptive_layer_tool_call_reservation = EmptyMap,
            adaptive_layer_run_id = EmptyMap,
            adaptive_layer_result_digest = EmptyMap,
            adaptive_layer_fault = EmptyMap,
            adaptive_layer_disposition = EmptyMap,
            adaptive_missing_body_digest = EmptyMap,
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
            // Spawn-execution phase ladder (machine owns the phase ORDER).
            // A lightweight 3-step ladder, NOT a per-obligation drain: the
            // machine validates Begin-before-Commit-before-Activate, while the
            // shell still executes the individual finalization steps. The
            // per-obligation feedback inputs + per-step request effects were
            // removed because they exploded the mob x meerkat composition state
            // space. `BeginSpawnExec` opens the per-identity window (carrying the
            // original Spawn authority guards); `CommitSpawnMembership` (renamed
            // `Spawn`, full membership field set retained — the verbatim
            // SpawnRunning* update/emit blocks consume those fields) commits the
            // membership facts; `CommitSpawnActivation` closes the window;
            // `AbortSpawnExec` clears it. Activation/abort are total over an
            // absent (Settled) phase entry, mirroring `KickoffQuiesced`.
            BeginSpawnExec {
                agent_identity: AgentIdentity,
                agent_runtime_id: AgentRuntimeId,
                fence_token: FenceToken,
                generation: Generation,
                profile_material_digest: String,
                external_addressable: bool,
                runtime_mode: Enum<SpawnPolicyRuntimeMode>,
                bridge_session_id: Option<SessionId>,
                replacing: Option<SessionId>,
            },
            CommitSpawnMembership { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_material_digest: String, external_addressable: bool, runtime_mode: Enum<SpawnPolicyRuntimeMode>, bridge_session_id: Option<SessionId>, replacing: Option<SessionId> },
            CommitSpawnActivation { agent_identity: AgentIdentity },
            AbortSpawnExec { agent_identity: AgentIdentity },
            AuthorizeSpawnProfile { agent_identity: AgentIdentity, profile_name: String, model: String, profile_material_digest: String, tool_config_digest: String, skills_digest: String, provider_params_digest: Option<String>, output_schema_digest: Option<String>, external_addressable: bool },
            ClassifySpawnManyFailure { observation: Enum<MobSpawnManyFailureObservationKind> },
            ClassifyMemberWait { agent_identity: AgentIdentity },
            ObserveMemberProgress {
                agent_identity: AgentIdentity,
                run_open: bool,
                in_flight_work: u64,
                progress_token: String,
                observed_at_ms: u64,
            },
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
            // RAW, atomic observations and feeds them here WITHOUT pre-composing
            // them: whether the operator holds manage scope over the target mob
            // (`manage_scope_present`), whether the operator's spawn-profile
            // scope SET for the target mob CONTAINS the requested profile
            // (`profile_scope_contains`, a raw per-profile set-membership fact —
            // NOT OR'd with manage scope), and the per-argument presence of every
            // privileged spawn argument (`privileged_*_present`, one pure
            // `.is_some()` observation per argument; a surface fills only the
            // arguments its own tool accepts and leaves the rest `false`).
            // MobMachine — not the tool shell — owns the privileged-argument SET
            // membership POLICY (which arguments are privileged) by OR-ing the
            // presence facts, and composes the `manage_scope_present ||
            // profile_scope_contains` profile-scope disjunction, deciding the
            // Allow/Deny verdict the shell mirrors (Deny -> access_denied).
            ResolveSpawnMemberAdmission {
                manage_scope_present: bool,
                profile_scope_contains: bool,
                privileged_resume_bridge_session_present: bool,
                privileged_resume_session_present: bool,
                privileged_backend_present: bool,
                privileged_runtime_mode_present: bool,
                privileged_launch_mode_present: bool,
                privileged_tool_access_policy_present: bool,
                privileged_budget_split_policy_present: bool,
                privileged_tooling_present: bool,
                privileged_auth_binding_present: bool,
            },
            // Per-mob operator admission for current-mob-scoped tools. The tool
            // shell extracts a single pure observation — whether the operator
            // holds manage scope over the current mob (a machine-owned
            // operator-scope projection) — and feeds it here. MobMachine — not
            // the tool shell — decides the Allow/Deny admission verdict, which
            // the shell mirrors (Deny -> access_denied).
            ResolveCurrentMobAdmission { can_manage_mob: bool },
            // Coarse spawn-tool admission for the spawn-member tool surfaces
            // (`spawn_member` / `spawn_many_members`). The tool shell extracts
            // TWO raw, atomic observations — whether the operator can manage the
            // current mob (`can_manage_mob`, a machine-minted operator-scope
            // membership projection) and whether the operator's spawn-profile
            // scope for the mob is non-empty (`spawn_profile_scope_present`, a
            // machine-owned operator-scope set-non-empty projection) — and feeds
            // BOTH here without pre-composing them. MobMachine — not the tool
            // shell — composes the disjunction (`can_manage_mob ||
            // spawn_profile_scope_present`) and decides the Allow/Deny admission
            // verdict, which the shell mirrors (Deny -> access_denied). This
            // coarse gate has unique coverage over the empty-specs
            // `spawn_many_members` case (zero per-member iterations fire no
            // per-member admission), so it must be machine-routed rather than
            // reduced in the shell.
            ResolveSpawnToolAdmission {
                can_manage_mob: bool,
                spawn_profile_scope_present: bool,
            },
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
            CleanupRetiringMemberWiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RestoreRetiringMemberWiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            WireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RegisterMemberPeer { agent_identity: AgentIdentity, peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberEndpointMigrationTrustCleanup { edge: WiringEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, retained_peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberPeerRebind { agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberPeerOverlay { agent_identity: AgentIdentity, expected_peer_endpoint: MemberPeerEndpoint },
            AuthorizeMemberTrustWiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustUnwiring { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanup { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            AuthorizeMemberTrustCleanupObserved { edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId },
            AuthorizeRetiringMemberTrustCleanupObserved { edge: WiringEdge, a_identity: AgentIdentity, a_peer_id: PeerId, b_identity: AgentIdentity, b_peer_id: PeerId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            AuthorizeExternalPeerReciprocalTrust { key: ExternalPeerKey, agent_identity: AgentIdentity },
            UnwireExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge },
            CleanupRetiringExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RestoreRetiringExternalPeer { key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            CleanupRetiringExternalPeerObservedAbsent { key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RestoreRetiringExternalPeerObservedAbsent { key: ExternalPeerKey, edge: ExternalPeerEdge, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            AdmitSupervisorRotation,
            ProvisionSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion },
            RecordSupervisorPendingRotation { current_peer_id: PeerId, current_epoch: u64, current_protocol_version: SupervisorProtocolVersion, operation_id: String, pending_peer_id: PeerId, pending_signing_key: PeerSigningKey, pending_epoch: u64, pending_protocol_version: SupervisorProtocolVersion, accepted_peer_ids: Set<PeerId>, active_peer_ids: Set<PeerId>, member_target_names: Map<PeerId, String>, member_target_addresses: Map<PeerId, String> },
            CommitSupervisorRotation { current_peer_id: PeerId, current_epoch: u64, current_protocol_version: SupervisorProtocolVersion, operation_id: String, next_peer_id: PeerId, next_signing_key: PeerSigningKey, next_epoch: u64, next_protocol_version: SupervisorProtocolVersion },
            ClearSupervisorAuthorityForDestroy { current_peer_id: PeerId, current_signing_key: PeerSigningKey, current_epoch: u64, protocol_version: SupervisorProtocolVersion },
            RestoreSupervisorAuthorityAfterDestroyRollback { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String> },
            // Dogma row R044: trust-install-before-terminality obligation.
            // Recorded before the shell installs recipient trust for a routed
            // bridge command; removed on authorization terminality success
            // (Resolve) or after the shell rolls the installed trust back on
            // a failure path (Rollback). Set semantics make all three inputs
            // idempotent so nested authorize/bind windows for the same peer
            // compose without double-record/double-resolve failures.
            RecordPendingRecipientTrust { peer_id: PeerId },
            ResolvePendingRecipientTrust { peer_id: PeerId },
            RollbackPendingRecipientTrust { peer_id: PeerId },
            SessionIngressDetachedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId },
            SessionIngressDetachFailedForMobDestroy { mob_id: MobId, agent_runtime_id: AgentRuntimeId, reason: String },
            SubmitWork { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, work_id: WorkId, origin: Enum<WorkOrigin> },
            ResolveSubmitWorkRejection { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, origin: Enum<WorkOrigin> },
            // Generated composition refusal closure. Each input is bound by
            // `meerkat_mob_seam` to one concrete routed effect kind; the shell
            // supplies only the consumer's stable code + display detail.
            ResolveRuntimeBindingRefusal { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String },
            ResolveRuntimeIngressRefusal { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, session_id: SessionId, work_id: WorkId, origin: Enum<WorkOrigin>, refusal_code: String, reason: String },
            ResolveRuntimeRetireRefusal { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String },
            RetryRuntimeRetire { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId },
            RecordRemoteMemberRuntimeRetired { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecordRemoteMemberSupervisorRevoked { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
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
            KickoffMarkPending { member_id: AgentIdentity, objective_id: String },
            BindObjectiveOwner { owner_id: AgentIdentity, objective_id: String },
            ConcludeObjective { member_id: AgentIdentity, objective_id: String, outcome: String },
            KickoffMarkStarting { member_id: AgentIdentity },
            StartupMarkReady { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            KickoffResolveStarted { member_id: AgentIdentity },
            KickoffResolveCallbackPending { member_id: AgentIdentity },
            KickoffResolveFailed { member_id: AgentIdentity, error: String },
            KickoffCancelRequested { member_id: AgentIdentity },
            KickoffClear { member_id: AgentIdentity },
            // 0.7.2 disciplined shell inputs (L5, rows 12/13): feedback input
            // that closes the machine-owned kickoff-waiter drain obligation
            // opened by member retire / destroy-retire transitions via the
            // `RequestKickoffQuiesce` effect. TOTAL over kickoff state: an
            // in-flight kickoff is recorded Cancelled (with persist + notice
            // effects), an idle/terminal kickoff is an accepted no-op, and the
            // Destroyed phase accepts it as a typed late arrival. The shell
            // never probes kickoff state before firing it.
            KickoffQuiesced { member_id: AgentIdentity },
            // 0.7.2 disciplined shell inputs (L5, row 14): typed teardown
            // closure for an in-flight pending spawn. Replaces the former
            // shell habit of laundering teardown cancellation through the
            // `CompleteSpawn` success signal. TOTAL: absent identities and the
            // Destroyed phase are accepted no-ops, so idempotent re-drains
            // and post-teardown stragglers never surface as guard rejections.
            CancelPendingSpawn { agent_identity: AgentIdentity },
            // ---------------------------------------------------------------
            // Wave-G1 catalog folds (rows #14, #181, #260, #261, #293, #314,
            // #320). These lower duplicate-admission, respawn-generation,
            // step-fault classification, supervisor escalation, topology-edge
            // verdict, external-member rebind capability, and turn-timeout
            // disposition into MobMachine authority. Each carries only RAW
            // typed caller facts; MobMachine owns every verdict.
            // ---------------------------------------------------------------
            // Row #14: duplicate-member admission probe. The actor reads the
            // emitted verdict instead of probing the PendingSpawnLineage map.
            ProbeMemberAdmission { agent_identity: AgentIdentity },
            // Row #181: compute the next monotone respawn generation. The
            // actor reads the emitted generation instead of `generation.next()`.
            ComputeRespawnGeneration { agent_identity: AgentIdentity },
            // Row #260: classify a typed step-output fault into retry/terminal.
            ClassifyStepOutputFault {
                run_id: RunId,
                step_id: StepId,
                target_retry_key: String,
                fault: Enum<StepOutputFaultKind>,
                attempt: u32,
                max_retries: u32,
            },
            // Row #261: supervisor escalation. Presence of an eligible target is
            // structural (two inputs, no `Option<AgentIdentity>`): a target was
            // selected, or no eligible target exists.
            EscalateToSupervisor {
                run_id: RunId,
                step_id: StepId,
                supervisor_identity: AgentIdentity,
                turn_timeout_ms: u64,
            },
            EscalateToSupervisorNoEligibleTarget { run_id: RunId, step_id: StepId },
            // Row #293: topology-edge admission verdict. The shell projects the
            // declarative rule match (if any) as `Option<PolicyDecision>`;
            // MobMachine applies the declared default policy for unmatched edges.
            // `from_role`/`to_role` are opaque role strings (no `ProfileName`
            // type is bound in the mob catalog metadata; the existing
            // FlowDelegationEdgeAdmission effect uses `String` too).
            EvaluateTopologyEdge {
                from_role: String,
                to_role: String,
                rule_match: Option<Enum<PolicyDecision>>,
            },
            // Row #314: external-member rebind capability. The bridge supplies
            // the typed capability bit; the projection reads the machine map.
            SetExternalMemberRebindCapability {
                agent_identity: AgentIdentity,
                capability: Enum<ExternalMemberRebindCapability>,
            },
            // Row #320: turn-timeout disposition. `orphan_budget` is a typed
            // machine field; the shell reads the detach/cancel/retryable verdict.
            ClassifyTurnTimeoutDisposition { timed_out_run_id: RunId, retryable: bool },
            // Row #320 (G1 regression fix): one-time orphan-budget seed. The
            // lead dispatches this once at actor build from
            // `definition.limits.max_orphaned_turns` (default 8) so non-retryable
            // timeouts can detach rather than always classifying Canceled.
            SeedOrphanBudget { budget: u64 },
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
            InitializeAdaptiveRun {
                adaptive_run_id: AdaptiveRunId,
                max_depth: u64,
                max_total_decisions: u64,
                max_repair_attempts: u64,
                max_layer_failures: u64,
                max_attempts_per_layer: u64,
                max_members_per_layer: u64,
                max_total_spawned_members: u64,
                max_active_members: u64,
                max_retained_layer_mobs: u64,
                max_aggregate_tokens: u64,
                max_aggregate_tool_calls: u64,
                allowed_model_classes: Set<String>,
                allowed_tool_classes: Set<String>,
                allowed_skill_identities: Set<String>,
                allowed_auth_binding_refs: Set<String>,
                deadline_ms: u64,
            },
            RecordPlanningDecision {
                adaptive_run_id: AdaptiveRunId,
                decision_kind: Enum<AdaptiveDecisionKind>,
            },
            RecordPlanRejected {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
            },
            ResolveLayerAdmission {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                plan_digest: String,
                child_mob_id: MobId,
                member_count: u64,
                token_reservation: u64,
                tool_call_reservation: u64,
                used_model_classes: Set<String>,
                used_tool_classes: Set<String>,
                used_skill_identities: Set<String>,
                used_auth_binding_refs: Set<String>,
                observed_at_ms: u64,
            },
            RecordLayerProvisioned {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
            },
            RecordLayerRunStarted {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                child_run_id: RunId,
            },
            IngestLayerTerminal {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                result_class: Enum<FlowRunPublicResultClassKind>,
                actual_tokens: u64,
                actual_tool_calls: u64,
            },
            RecordLayerSetupFault {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                fault: Enum<AdaptiveLayerSetupFaultKind>,
                spawned_members: u64,
                requested_members: u64,
            },
            RecordLayerResultValidated {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                result_digest: String,
            },
            RecordLayerResultInvalid {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
            },
            RecordLayerMobDestroyed {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
            },
            RecordLayerMobRetained {
                adaptive_run_id: AdaptiveRunId,
                layer_id: AdaptiveLayerId,
                attempt: u64,
                disposition: Enum<AdaptiveLayerDispositionKind>,
            },
            RecordCleanupResolved { adaptive_run_id: AdaptiveRunId },
            RecordBodyEvidenceMissing { adaptive_run_id: AdaptiveRunId, missing_digest: String },
            ResolveAdaptiveFinish {
                adaptive_run_id: AdaptiveRunId,
                final_result_digest: String,
            },
            RequestAdaptiveCancel { adaptive_run_id: AdaptiveRunId },
            RecordDeadlineObserved { adaptive_run_id: AdaptiveRunId, observed_at_ms: u64 },
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
            AdmitDestroyMemberRetire { mob_id: MobId, agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId> },
            ObserveRuntimeRetired { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            ObserveMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId> },
            ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            ObserveDestroyMemberRetirementArchived { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, session_id: Option<SessionId> },
            ResetMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool, session_id: SessionId },
            RespawnMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool, session_id: SessionId },
            ResolveRespawnTopologyRestore { agent_identity: AgentIdentity, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            DestroyMob { session_id: SessionId },
            ObserveRuntimeDestroyed { agent_runtime_id: AgentRuntimeId, fence_token: FenceToken },
            RecoverRosterMember { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, profile_name: String, runtime_mode: Enum<SpawnPolicyRuntimeMode>, external_addressable: bool },
            RecoverMemberSessionBinding { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, replacing: Option<SessionId> },
            RecoverSpawnedMemberPeerEndpoint { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation, peer_endpoint: MemberPeerEndpoint },
            RecoverMemberPeerEndpoint { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, bridge_session_id: SessionId, peer_endpoint: MemberPeerEndpoint },
            RecoverRosterMemberReset { agent_identity: AgentIdentity, previous_agent_runtime_id: AgentRuntimeId, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecoverRosterMemberRetirementStarted { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, generation: Generation, releasing: Option<SessionId>, session_id: Option<SessionId>, retiring_peer_endpoint: Option<MemberPeerEndpoint> },
            RecoverRemoteMemberRuntimeRetired { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecoverRemoteMemberSupervisorRevoked { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, fence_token: FenceToken, generation: Generation },
            RecoverRosterMemberRetired { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId },
            ConvergeRecoveredRosterTopology { edge: WiringEdge, a_identity: AgentIdentity, b_identity: AgentIdentity },
            RecoverMemberKickoff { member_id: AgentIdentity, phase: KickoffPhase, error: Option<String> },
            RecoverObjectiveBinding { member_id: AgentIdentity, objective_id: String },
            RecoverObjectiveConclusion { member_id: AgentIdentity, objective_id: String, outcome: String },
            RecoverRosterWiring { edge: WiringEdge },
            RecoverRosterUnwire { edge: WiringEdge },
            RecoverExternalPeerWiring { key: ExternalPeerKey, edge: ExternalPeerEdge },
            RecoverExternalPeerUnwire { key: ExternalPeerKey },
            RecoverSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String> },
            RecoverOwnerBridgeSession { bridge_session_id: SessionId, destroy_on_owner_archive: bool, implicit_delegation_mob: bool },
            RecoverMemberRestoreFailure { agent_identity: AgentIdentity, reason: String },
            ClassifyMemberLiveMaterialization { agent_identity: AgentIdentity, observation: Enum<MemberLiveMaterializationObservationKind>, reason: String },
            ResolveMemberRevivalSucceeded { agent_identity: AgentIdentity },
            ResolveMemberRevivalFailed { agent_identity: AgentIdentity, reason: String },
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
            RequestRuntimeRetire { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId },
            RequestRuntimeDestroy { session_id: SessionId },
            RuntimeBindingRefusalClassified { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String },
            RuntimeIngressRefusalClassified { agent_runtime_id: AgentRuntimeId, session_id: SessionId, work_id: WorkId, origin: Enum<WorkOrigin>, refusal_code: String, reason: String },
            RuntimeRetireRefusalClassified { agent_identity: AgentIdentity, agent_runtime_id: AgentRuntimeId, session_id: SessionId, refusal_code: String, reason: String },
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
            PersistKickoffUpdate { member_id: AgentIdentity, phase: KickoffPhase },
            PersistKickoffFailureUpdate { member_id: AgentIdentity, phase: KickoffPhase, error: String },
            EmitKickoffLifecycleNotice { member_id: AgentIdentity, intent: Enum<KickoffIntent> },
            PersistObjectiveOwnerBinding { owner_id: AgentIdentity, objective_id: String },
            PersistObjectiveConclusion { member_id: AgentIdentity, objective_id: String, outcome: String },
            // 0.7.2 L5 (rows 12/13): machine-owned drain obligation for the
            // autonomous-kickoff completion-waiter task. Emitted by every
            // member retire / destroy-retire admission. The shell discharges
            // it (abort + await the waiter task, resolve its waiters with a
            // typed terminal outcome) and closes it with the `KickoffQuiesced`
            // feedback input. Member retirement archival and mob destroy are
            // guarded on closure (kickoff not in flight), mirroring the
            // mob-destroy session-ingress-detach obligation shape.
            RequestKickoffQuiesce { member_id: AgentIdentity },
            // 0.7.2 L5 (row 14): machine-owned drain obligation for in-flight
            // spawn-provisioning waiter tasks at destroy admission. Emitted by
            // `AdmitDestroyCleanup`. The shell aborts + awaits every pending
            // spawn task, fails its waiters with a typed cancellation, and
            // closes each machine obligation with `CancelPendingSpawn`;
            // `Destroy`/`DestroyMob` are guarded on the pending-spawn table
            // being drained instead of silently wiping it.
            RequestPendingSpawnQuiesceForDestroy,
            // (Spawn-execution phase ladder emits NO request effects — the
            // machine only owns the phase order; the shell realizes each step.)
            // Row #14: machine-owned duplicate-member admission verdict. The
            // actor mirrors this (Admitted -> Ok; DuplicateRejected ->
            // MemberAlreadyExists) instead of probing the PendingSpawnLineage map.
            MemberAdmissionProbed { agent_identity: AgentIdentity, verdict: Enum<MemberAdmissionVerdictKind> },
            // Row #181: machine-owned next monotone respawn generation. The
            // actor consumes `next_generation` instead of `generation.next()`.
            RespawnGenerationComputed { agent_identity: AgentIdentity, next_generation: Generation },
            // Row #260: machine-owned step-output fault disposition. The flow
            // shell mirrors Retry (attempt+=1, continue) / Terminal (fail). The
            // typed `terminal_cause` flows into terminalization (row #126).
            StepOutputFaultClassified {
                run_id: RunId,
                step_id: StepId,
                target_retry_key: String,
                disposition: Enum<StepFaultDispositionKind>,
                terminal_cause: Enum<StepOutputFaultKind>,
            },
            // Row #261: machine-owned supervisor escalation request. The shell
            // runs the internal turn with the machine-supplied timeout and
            // realizes the escalation evidence.
            SupervisorEscalationRequested {
                run_id: RunId,
                step_id: StepId,
                supervisor_identity: AgentIdentity,
                turn_timeout_ms: u64,
            },
            // Row #261: machine-owned typed escalation failure. Fail-closed is a
            // typed terminal cause, not a free-form `SupervisorEscalation` string.
            SupervisorEscalationFailed {
                run_id: RunId,
                step_id: StepId,
                cause: Enum<SupervisorEscalationFailureCause>,
            },
            // Row #293: machine-owned topology-edge verdict. The FlowEngine reads
            // the verdict; the default policy is applied in-DSL for unmatched edges.
            TopologyEdgeVerdictResolved {
                from_role: String,
                to_role: String,
                verdict: Enum<PolicyDecision>,
            },
            // Row #320: machine-owned turn-timeout disposition. The shell mirrors
            // Detached (reconcile detached turn) / Canceled (abort) / Retryable.
            TurnTimeoutDispositionClassified {
                timed_out_run_id: RunId,
                disposition: Enum<TurnTimeoutDisposition>,
            },
            // Row #351: machine-owned membership-intent effects emitted by the
            // single-identity EnsureMember path. The handle mechanically realizes
            // spawn/retain/retire instead of computing the desired-vs-current diff.
            MemberSpawnRequired { agent_identity: AgentIdentity },
            MemberRetainRequired { agent_identity: AgentIdentity },
            MemberRetireRequired { agent_identity: AgentIdentity },
            SpawnPolicyResolutionRecorded { agent_identity: AgentIdentity, revision: u64, profile_name: Option<String>, runtime_mode: Option<Enum<SpawnPolicyRuntimeMode>> },
            OwnerBridgeSessionBound { bridge_session_id: SessionId, destroy_on_owner_archive: bool, implicit_delegation_mob: bool },
            RespawnTopologyRestoreResolved { agent_identity: AgentIdentity, result: Enum<RespawnTopologyRestoreResultKind>, failed_peer_ids: Seq<RespawnTopologyPeerId> },
            MemberLiveMaterializationClassified { agent_identity: AgentIdentity, observation: Enum<MemberLiveMaterializationObservationKind>, verdict: Enum<MemberRevivalVerdictKind>, reason: String },
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
            PersistSupervisorAuthority { peer_id: PeerId, signing_key: PeerSigningKey, epoch: u64, protocol_version: SupervisorProtocolVersion, pending_operation_id: Option<String>, pending_peer_id: Option<PeerId>, pending_signing_key: Option<PeerSigningKey>, pending_epoch: Option<u64>, pending_protocol_version: Option<SupervisorProtocolVersion>, pending_accepted_peer_ids: Set<PeerId>, pending_member_target_names: Map<PeerId, String>, pending_member_target_addresses: Map<PeerId, String> },
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
            AdaptiveRunInitialized { adaptive_run_id: AdaptiveRunId },
            AdaptivePlanningDecisionRecorded { adaptive_run_id: AdaptiveRunId, decision_kind: Enum<AdaptiveDecisionKind> },
            AdaptivePlanRejected { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId },
            AdaptiveLayerAdmissionResolved { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, admission: Enum<AdaptiveLayerAdmissionKind> },
            AdaptiveLayerProvisioned { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64 },
            AdaptiveLayerRunStarted { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, attempt: u64, child_run_id: RunId },
            AdaptiveLayerTerminalIngested { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, result_class: Enum<FlowRunPublicResultClassKind> },
            AdaptiveLayerSetupFaultRecorded { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, fault: Enum<AdaptiveLayerSetupFaultKind> },
            AdaptiveLayerResultValidated { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, result_digest: String },
            AdaptiveLayerResultInvalid { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId },
            AdaptiveLayerCleanupObserved { adaptive_run_id: AdaptiveRunId, layer_id: AdaptiveLayerId, disposition: Enum<AdaptiveLayerDispositionKind> },
            AdaptiveCleanupResolved { adaptive_run_id: AdaptiveRunId },
            AdaptiveBodyEvidenceMissing { adaptive_run_id: AdaptiveRunId, missing_digest: String },
            AdaptiveRunTerminalized { adaptive_run_id: AdaptiveRunId, reason: Enum<AdaptiveStopReason> },
        }

        disposition RequestRuntimeBinding => routed [MeerkatMachine] seam NoOwnerRealization,
        disposition SpawnProfileAuthorized => local seam NoOwnerRealization,
        disposition RequestRuntimeIngress => routed [MeerkatMachine] seam NoOwnerRealization,
        disposition RequestPeerRuntimeIngress => local seam SurfaceResultAlignment,
        disposition SubmitWorkRejected => local seam SurfaceResultAlignment,
        disposition CancelAllWorkRejected => local seam SurfaceResultAlignment,
        disposition RequestRuntimeRetire => routed [MeerkatMachine] seam NoOwnerRealization,
        disposition RequestRuntimeDestroy => routed [MeerkatMachine] seam NoOwnerRealization,
        disposition RuntimeBindingRefusalClassified => local seam SurfaceResultAlignment,
        disposition RuntimeIngressRefusalClassified => local seam SurfaceResultAlignment,
        disposition RuntimeRetireRefusalClassified => local seam SurfaceResultAlignment,
        disposition PendingSpawnOperationOwnerAuthorized => local seam NoOwnerRealization,
        disposition RequestSessionIngressDetachForMobDestroy => external handoff mob_destroying_session_ingress seam NoOwnerRealization,
        disposition AppendLifecycleJournal => local seam NoOwnerRealization,
        disposition AppendOperatorActionProvenance => local seam NoOwnerRealization,
        disposition EmitMemberLifecycleNotice => external seam SurfaceResultAlignment,
        disposition EmitRunLifecycleNotice => external seam SurfaceResultAlignment,
        disposition EmitFlowRunNotice => external seam SurfaceResultAlignment,
        disposition AppendFailureLedger => local seam NoOwnerRealization,
        disposition FlowTerminalized => external seam SurfaceResultAlignment,
        disposition FlowRunTerminal => local seam SurfaceResultAlignment,
        disposition FlowRunNonTerminal => local seam SurfaceResultAlignment,
        disposition FlowStepTerminal => local seam SurfaceResultAlignment,
        disposition FlowStepNonTerminal => local seam SurfaceResultAlignment,
        disposition FlowFrameTerminalStatusClassified => local seam SurfaceResultAlignment,
        disposition FlowFrameTerminalStatusUnavailable => local seam SurfaceResultAlignment,
        disposition FlowRunPublicResultClassified => local seam SurfaceResultAlignment,
        disposition EscalateSupervisor => external seam OwnerRealizationOnly,
        disposition NotifyCoordinator => external seam OwnerRealizationOnly,
        disposition ExposePendingSpawn => external seam OwnerRealizationOnly,
        disposition EmitMemberTerminalNotice => external seam SurfaceResultAlignment,
        disposition AdmitPeerInput => external seam OwnerRealizationOnly,
        disposition EmitProgressNote => external seam OwnerRealizationOnly,
        disposition PersistKickoffUpdate => local seam NoOwnerRealization,
        disposition PersistKickoffFailureUpdate => local seam NoOwnerRealization,
        disposition EmitKickoffLifecycleNotice => external seam OwnerRealizationOnly,
        disposition PersistObjectiveOwnerBinding => local seam NoOwnerRealization,
        disposition PersistObjectiveConclusion => local seam NoOwnerRealization,
        // 0.7.2 L5 drain obligations: realized by the owning mob actor itself
        // (same-machine effect→feedback shape; feedback inputs
        // `KickoffQuiesced` / `CancelPendingSpawn` close them and teardown
        // completion is guard-enforced on closure). Classified `local` like
        // the sibling kickoff persistence effects; cross-machine C-F3 pairing
        // is not required because no other machine participates.
        disposition RequestKickoffQuiesce => local seam NoOwnerRealization,
        disposition RequestPendingSpawnQuiesceForDestroy => local seam NoOwnerRealization,
        disposition MemberAdmissionProbed => local seam SurfaceResultAlignment,
        disposition RespawnGenerationComputed => local seam SurfaceResultAlignment,
        disposition StepOutputFaultClassified => local seam SurfaceResultAlignment,
        disposition SupervisorEscalationRequested => external seam OwnerRealizationOnly,
        disposition SupervisorEscalationFailed => local seam SurfaceResultAlignment,
        disposition TopologyEdgeVerdictResolved => local seam SurfaceResultAlignment,
        disposition TurnTimeoutDispositionClassified => local seam SurfaceResultAlignment,
        disposition MemberSpawnRequired => external seam OwnerRealizationOnly,
        disposition MemberRetainRequired => local seam SurfaceResultAlignment,
        disposition MemberRetireRequired => external seam OwnerRealizationOnly,
        disposition SpawnPolicyResolutionRecorded => local seam NoOwnerRealization,
        disposition OwnerBridgeSessionBound => local seam SurfaceResultAlignment,
        disposition RespawnTopologyRestoreResolved => local seam SurfaceResultAlignment,
        disposition MemberLiveMaterializationClassified => local seam SurfaceResultAlignment,
        disposition SpawnManyFailureClassified => local seam SurfaceResultAlignment,
        disposition MemberWaitClassified => local seam SurfaceResultAlignment,
        disposition FlowDelegationEdgeAdmissionResolved => local seam SurfaceResultAlignment,
        disposition RemoteMemberRuntimeTerminalityClassified => local seam SurfaceResultAlignment,
        disposition SpawnMemberAdmissionResolved => local seam SurfaceResultAlignment,
        disposition CurrentMobAdmissionResolved => local seam SurfaceResultAlignment,
        disposition SpawnToolAdmissionResolved => local seam SurfaceResultAlignment,
        disposition CreateMobAdmissionResolved => local seam SurfaceResultAlignment,
        disposition ProfileMutationAdmissionResolved => local seam SurfaceResultAlignment,
        disposition MemberOperationEligibilityResolved => local seam SurfaceResultAlignment,
        disposition BridgeRejectionRecoveryClassified => local seam SurfaceResultAlignment,
        disposition PendingSupervisorAcceptanceClassified => local seam SurfaceResultAlignment,
        disposition FrameSeedConfirmed => local seam SurfaceResultAlignment,
        disposition WiringGraphChanged => external seam SurfaceResultAlignment,
        disposition MemberSessionBindingChanged => external seam SurfaceResultAlignment,
        disposition SessionProvisionOperationOwnerAuthorized => local seam NoOwnerRealization,
        disposition MemberTrustWiringRequested => external handoff mob_member_trust_wiring seam OwnerRealizationOnly,
        disposition MemberTrustUnwiringRequested => external handoff mob_member_trust_unwiring seam OwnerRealizationOnly,
        disposition WiringTrustRepairRequested => local seam NoOwnerRealization,
        disposition ExternalPeerTrustWiringRequested => external handoff mob_external_peer_trust_wiring seam OwnerRealizationOnly,
        disposition ExternalPeerTrustUnwiringRequested => external handoff mob_external_peer_trust_unwiring seam OwnerRealizationOnly,
        disposition ExternalPeerTrustRepairRequested => external handoff mob_external_peer_trust_repair seam OwnerRealizationOnly,
        disposition MemberPeerRegistered => local seam NoOwnerRealization,
        disposition MemberPeerRebindAuthorized => local seam NoOwnerRealization,
        disposition MemberPeerOverlayAuthorized => external handoff mob_member_peer_overlay seam OwnerRealizationOnly,
        disposition ExternalPeerReciprocalTrustRequested => external handoff mob_external_peer_reciprocal_trust seam OwnerRealizationOnly,
        disposition PersistSupervisorAuthority => local seam SurfaceResultAlignment,
        disposition DeleteSupervisorAuthority => local seam SurfaceResultAlignment,
        disposition EmitWiringLifecycleNotice => external seam SurfaceResultAlignment,
        disposition EmitExternalPeerWiringLifecycleNotice => external seam SurfaceResultAlignment,
        disposition AuthorizeAgentEventSubscription => local seam SurfaceResultAlignment,
        disposition RejectAgentEventSubscription => local seam SurfaceResultAlignment,
        disposition AuthorizeAllAgentEventSubscription => local seam SurfaceResultAlignment,
        disposition RejectAllAgentEventSubscription => local seam SurfaceResultAlignment,
        disposition AuthorizeMobEventRouter => local seam SurfaceResultAlignment,
        disposition AuthorizeMobEventRouterMemberSubscription => local seam SurfaceResultAlignment,
        disposition AuthorizeMobEventRouterMemberRemoval => local seam SurfaceResultAlignment,
        disposition AuthorizeStructuralEventSubscription => local seam SurfaceResultAlignment,
        disposition RejectStructuralEventSubscription => local seam SurfaceResultAlignment,
        disposition AuthorizeStrictEventPoll => local seam SurfaceResultAlignment,
        disposition RejectStrictEventPoll => local seam SurfaceResultAlignment,
        disposition WorkIntentRecorded => local seam NoOwnerRealization,
        disposition ResourceClaimRecorded => local seam NoOwnerRealization,
        disposition WorkIntentStatusChanged => local seam NoOwnerRealization,
        disposition ResourceClaimStatusChanged => local seam NoOwnerRealization,
        disposition ResourceClaimOverlapObserved => local seam NoOwnerRealization,
        disposition AdaptiveRunInitialized => local seam NoOwnerRealization,
        disposition AdaptivePlanningDecisionRecorded => local seam NoOwnerRealization,
        disposition AdaptivePlanRejected => local seam NoOwnerRealization,
        disposition AdaptiveLayerAdmissionResolved => local seam SurfaceResultAlignment,
        disposition AdaptiveLayerProvisioned => local seam NoOwnerRealization,
        disposition AdaptiveLayerRunStarted => local seam NoOwnerRealization,
        disposition AdaptiveLayerTerminalIngested => local seam NoOwnerRealization,
        disposition AdaptiveLayerSetupFaultRecorded => local seam NoOwnerRealization,
        disposition AdaptiveLayerResultValidated => local seam NoOwnerRealization,
        disposition AdaptiveLayerResultInvalid => local seam NoOwnerRealization,
        disposition AdaptiveLayerCleanupObserved => local seam NoOwnerRealization,
        disposition AdaptiveCleanupResolved => local seam NoOwnerRealization,
        disposition AdaptiveBodyEvidenceMissing => local seam NoOwnerRealization,
        disposition AdaptiveRunTerminalized => local seam SurfaceResultAlignment,

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

        invariant member_peer_endpoint_material_is_coherent {
            for_all(id in self.member_peer_ids.keys(), self.member_peer_endpoints.contains_key(id))
            && for_all(id in self.member_peer_endpoints.keys(), self.member_peer_ids.contains_key(id))
            && for_all(id in self.member_peer_endpoints.keys(),
                self.member_peer_ids.get_cloned(id) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(id).get("value"))))
        }

        invariant member_prior_peer_endpoints_are_generation_scoped {
            for_all(id in self.member_prior_peer_endpoints.keys(),
                self.identity_to_runtime.contains_key(id)
                && self.member_peer_endpoints.contains_key(id)
                && self.member_prior_peer_endpoints.get_cloned(id).get("value") != EmptySet
                && for_all(prior_endpoint in self.member_prior_peer_endpoints.get_cloned(id).get("value"),
                    mob_machine_member_peer_endpoint_peer_id(prior_endpoint) != mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(id).get("value"))))
        }

        invariant member_peer_id_ownership_is_global_across_generations {
            for_all(id in self.member_peer_endpoints.keys(),
                mob_machine_member_peer_id_available_for_identity(
                    self.member_peer_endpoints,
                    self.member_prior_peer_endpoints,
                    id,
                    self.member_peer_endpoints.get_cloned(id).get("value")))
            && for_all(id in self.member_prior_peer_endpoints.keys(),
                for_all(prior_endpoint in self.member_prior_peer_endpoints.get_cloned(id).get("value"),
                    mob_machine_member_peer_id_available_for_identity(
                        self.member_peer_endpoints,
                        self.member_prior_peer_endpoints,
                        id,
                        prior_endpoint)))
        }

        invariant pending_session_ingress_detach_has_session_correlation {
            for_all(agent_runtime_id in self.pending_session_ingress_detach_runtime_ids,
                self.runtime_retire_pending_sessions.contains_key(agent_runtime_id))
        }

        invariant remote_runtime_retired_is_still_retiring {
            for_all(agent_runtime_id in self.remote_runtime_retired_ids,
                self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring))
        }

        invariant remote_supervisor_revoked_has_retired_runtime_anchor {
            for_all(agent_runtime_id in self.remote_supervisor_revoked_ids,
                self.remote_runtime_retired_ids.contains(agent_runtime_id) == true
                && self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring))
        }

        // Spawn-exec phase ladder: once a spawn-exec has committed membership
        // (phase == MembershipCommitted), the identity must already appear in
        // identity_to_runtime (the membership commit inserts it in the same
        // atomic step). The Opened phase precedes the membership commit, so it
        // is exempt. This documents + verifies the ladder's commit ordering; it
        // is NOT a duplicate of bindings_require_known_identity (which keys off
        // member_session_bindings, not the spawn_exec phase).
        invariant spawn_exec_membership_commit_requires_runtime {
            for_all(id in self.spawn_exec_phase.keys(),
                self.spawn_exec_phase.get_cloned(id) != Some(SpawnExecPhase::MembershipCommitted)
                || self.identity_to_runtime.contains_key(id))
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
                && self.supervisor_pending_authority_operation_id == None
                && self.supervisor_pending_authority_accepted_peer_ids == EmptySet
                && self.supervisor_pending_authority_member_target_names == EmptyMap
                && self.supervisor_pending_authority_member_target_addresses == EmptyMap
            )
            || (
                self.supervisor_pending_authority_peer_id != None
                && self.supervisor_pending_authority_signing_key != None
                && self.supervisor_pending_authority_epoch != None
                && self.supervisor_pending_authority_protocol_version != None
            )
        }

        invariant supervisor_pending_authority_member_targets_aligned {
            self.supervisor_pending_authority_member_target_names.keys()
                == self.supervisor_pending_authority_member_target_addresses.keys()
        }

        invariant supervisor_pending_authority_acceptance_has_target {
            self.supervisor_pending_authority_operation_id == None
            || for_all(peer_id in self.supervisor_pending_authority_accepted_peer_ids,
                self.supervisor_pending_authority_member_target_names.contains_key(peer_id)
                && self.supervisor_pending_authority_member_target_addresses.contains_key(peer_id))
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

        transition ObserveMemberProgressChangedOpen {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_changed" { self.member_progress_tokens.get_cloned(agent_identity) != Some(progress_token) }
            guard "run_open" { run_open == true }
            update {
                self.member_last_progress_at_ms.insert(agent_identity, observed_at_ms);
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::ExecutionAdvanced);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Healthy);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_progress_tokens.insert(agent_identity, progress_token);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressChangedIdle {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_changed" { self.member_progress_tokens.get_cloned(agent_identity) != Some(progress_token) }
            guard "run_idle" { run_open == false }
            update {
                self.member_last_progress_at_ms.insert(agent_identity, observed_at_ms);
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::BecameIdle);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Healthy);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_progress_tokens.insert(agent_identity, progress_token);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressUnchangedIdle {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_unchanged" { self.member_progress_tokens.get_cloned(agent_identity) == Some(progress_token) }
            guard "run_idle" { run_open == false }
            update {
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::Unchanged);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Healthy);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressUnchangedOpenHealthy {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_unchanged" { self.member_progress_tokens.get_cloned(agent_identity) == Some(progress_token) }
            guard "run_open" { run_open == true }
            guard "progress_recent" { observed_at_ms - self.member_last_progress_at_ms.get_cloned(agent_identity).get("value") < 60000 }
            update {
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::Unchanged);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Healthy);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressUnchangedOpenDegraded {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_unchanged" { self.member_progress_tokens.get_cloned(agent_identity) == Some(progress_token) }
            guard "run_open" { run_open == true }
            guard "progress_degraded" { observed_at_ms - self.member_last_progress_at_ms.get_cloned(agent_identity).get("value") >= 60000 && observed_at_ms - self.member_last_progress_at_ms.get_cloned(agent_identity).get("value") < 300000 }
            update {
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::Unchanged);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Degraded);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressUnchangedOpenWedged {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_monotonic" { self.member_last_observed_at_ms.get_copied(agent_identity) <= Some(observed_at_ms) }
            guard "member_progress_unchanged" { self.member_progress_tokens.get_cloned(agent_identity) == Some(progress_token) }
            guard "run_open" { run_open == true }
            guard "progress_wedged" { observed_at_ms - self.member_last_progress_at_ms.get_cloned(agent_identity).get("value") >= 300000 }
            update {
                self.member_last_progress_event.insert(agent_identity, MemberProgressEventKind::Unchanged);
                self.member_health_class.insert(agent_identity, MemberHealthClass::Wedged);
                self.member_run_open.insert(agent_identity, run_open);
                self.member_in_flight_work.insert(agent_identity, in_flight_work);
                self.member_last_observed_at_ms.insert(agent_identity, observed_at_ms);
            }
            to Running
        }

        transition ObserveMemberProgressStale {
            per_phase [Running]
            on input ObserveMemberProgress { agent_identity, run_open, in_flight_work, progress_token, observed_at_ms }
            guard "observation_stale" { self.member_last_observed_at_ms.get_copied(agent_identity) > Some(observed_at_ms) }
            update {}
            to Running
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
        // The tool surface extracts RAW, atomic observations (manage scope, raw
        // per-profile spawn-scope set membership, and the per-argument presence
        // of every privileged spawn argument) and feeds them here WITHOUT
        // pre-composing them; MobMachine owns the privileged-argument SET
        // membership policy (OR-ing the presence facts) and the
        // `manage_scope_present || profile_scope_contains` profile-scope
        // disjunction, composing the Allow/Deny verdict. Pure classification
        // across all phases.
        //
        // Verdict policy (composed entirely here):
        //   manage_scope_present                              -> Allowed
        //   !manage && any-privileged-arg                     -> Denied
        //   !manage && !privileged && profile_scope_contains  -> Allowed
        //   !manage && !privileged && !profile_scope_contains -> Denied

        transition ResolveSpawnMemberAdmissionManageScope {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission {
                manage_scope_present,
                profile_scope_contains,
                privileged_resume_bridge_session_present,
                privileged_resume_session_present,
                privileged_backend_present,
                privileged_runtime_mode_present,
                privileged_launch_mode_present,
                privileged_tool_access_policy_present,
                privileged_budget_split_policy_present,
                privileged_tooling_present,
                privileged_auth_binding_present
            }
            guard "manage_scope_allows" { manage_scope_present == true }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Allowed }
        }

        transition ResolveSpawnMemberAdmissionPrivilegedArgsDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission {
                manage_scope_present,
                profile_scope_contains,
                privileged_resume_bridge_session_present,
                privileged_resume_session_present,
                privileged_backend_present,
                privileged_runtime_mode_present,
                privileged_launch_mode_present,
                privileged_tool_access_policy_present,
                privileged_budget_split_policy_present,
                privileged_tooling_present,
                privileged_auth_binding_present
            }
            guard "privileged_args_without_manage_scope" {
                manage_scope_present == false
                && (
                    privileged_resume_bridge_session_present == true
                    || privileged_resume_session_present == true
                    || privileged_backend_present == true
                    || privileged_runtime_mode_present == true
                    || privileged_launch_mode_present == true
                    || privileged_tool_access_policy_present == true
                    || privileged_budget_split_policy_present == true
                    || privileged_tooling_present == true
                    || privileged_auth_binding_present == true
                )
            }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Denied }
        }

        transition ResolveSpawnMemberAdmissionProfileScope {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission {
                manage_scope_present,
                profile_scope_contains,
                privileged_resume_bridge_session_present,
                privileged_resume_session_present,
                privileged_backend_present,
                privileged_runtime_mode_present,
                privileged_launch_mode_present,
                privileged_tool_access_policy_present,
                privileged_budget_split_policy_present,
                privileged_tooling_present,
                privileged_auth_binding_present
            }
            guard "profile_scope_allows" {
                manage_scope_present == false
                && privileged_resume_bridge_session_present == false
                && privileged_resume_session_present == false
                && privileged_backend_present == false
                && privileged_runtime_mode_present == false
                && privileged_launch_mode_present == false
                && privileged_tool_access_policy_present == false
                && privileged_budget_split_policy_present == false
                && privileged_tooling_present == false
                && privileged_auth_binding_present == false
                && profile_scope_contains == true
            }
            update {}
            to Running
            emit SpawnMemberAdmissionResolved { admission: MobSpawnMemberAdmissionKind::Allowed }
        }

        transition ResolveSpawnMemberAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnMemberAdmission {
                manage_scope_present,
                profile_scope_contains,
                privileged_resume_bridge_session_present,
                privileged_resume_session_present,
                privileged_backend_present,
                privileged_runtime_mode_present,
                privileged_launch_mode_present,
                privileged_tool_access_policy_present,
                privileged_budget_split_policy_present,
                privileged_tooling_present,
                privileged_auth_binding_present
            }
            guard "no_scope_denies" {
                manage_scope_present == false
                && privileged_resume_bridge_session_present == false
                && privileged_resume_session_present == false
                && privileged_backend_present == false
                && privileged_runtime_mode_present == false
                && privileged_launch_mode_present == false
                && privileged_tool_access_policy_present == false
                && privileged_budget_split_policy_present == false
                && privileged_tooling_present == false
                && privileged_auth_binding_present == false
                && profile_scope_contains == false
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
            on input ResolveSpawnToolAdmission { can_manage_mob, spawn_profile_scope_present }
            guard "spawn_scope_allows" {
                can_manage_mob == true || spawn_profile_scope_present == true
            }
            update {}
            to Running
            emit SpawnToolAdmissionResolved { admission: MobSpawnToolAdmissionKind::Allowed }
        }

        transition ResolveSpawnToolAdmissionDenied {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolveSpawnToolAdmission { can_manage_mob, spawn_profile_scope_present }
            guard "no_spawn_scope_denies" {
                can_manage_mob == false && spawn_profile_scope_present == false
            }
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

        transition InitializeAdaptiveRunRunning {
            per_phase [Running]
            on input InitializeAdaptiveRun {
                adaptive_run_id,
                max_depth,
                max_total_decisions,
                max_repair_attempts,
                max_layer_failures,
                max_attempts_per_layer,
                max_members_per_layer,
                max_total_spawned_members,
                max_active_members,
                max_retained_layer_mobs,
                max_aggregate_tokens,
                max_aggregate_tool_calls,
                allowed_model_classes,
                allowed_tool_classes,
                allowed_skill_identities,
                allowed_auth_binding_refs,
                deadline_ms
            }
            guard "no_active_adaptive_run" { self.adaptive_active_run == None }
            guard "limits_complete" {
                max_depth > 0
                && max_total_decisions > 0
                && max_repair_attempts > 0
                && max_layer_failures > 0
                && max_attempts_per_layer > 0
                && max_members_per_layer > 0
                && max_total_spawned_members > 0
                && max_active_members > 0
                && max_retained_layer_mobs > 0
                && max_aggregate_tokens > 0
                && max_aggregate_tool_calls > 0
                && deadline_ms > 0
            }
            update {
                self.adaptive_active_run = Some(adaptive_run_id);
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Active);
                self.adaptive_limit_max_depth.insert(adaptive_run_id, max_depth);
                self.adaptive_limit_max_total_decisions.insert(adaptive_run_id, max_total_decisions);
                self.adaptive_limit_max_repair_attempts.insert(adaptive_run_id, max_repair_attempts);
                self.adaptive_limit_max_layer_failures.insert(adaptive_run_id, max_layer_failures);
                self.adaptive_limit_max_attempts_per_layer.insert(adaptive_run_id, max_attempts_per_layer);
                self.adaptive_limit_max_members_per_layer.insert(adaptive_run_id, max_members_per_layer);
                self.adaptive_limit_max_total_spawned_members.insert(adaptive_run_id, max_total_spawned_members);
                self.adaptive_limit_max_active_members.insert(adaptive_run_id, max_active_members);
                self.adaptive_limit_max_retained_layer_mobs.insert(adaptive_run_id, max_retained_layer_mobs);
                self.adaptive_limit_max_aggregate_tokens.insert(adaptive_run_id, max_aggregate_tokens);
                self.adaptive_limit_max_aggregate_tool_calls.insert(adaptive_run_id, max_aggregate_tool_calls);
                self.adaptive_limit_allowed_model_classes.insert(adaptive_run_id, allowed_model_classes);
                self.adaptive_limit_allowed_tool_classes.insert(adaptive_run_id, allowed_tool_classes);
                self.adaptive_limit_allowed_skill_identities.insert(adaptive_run_id, allowed_skill_identities);
                self.adaptive_limit_allowed_auth_binding_refs.insert(adaptive_run_id, allowed_auth_binding_refs);
                self.adaptive_deadline_ms.insert(adaptive_run_id, deadline_ms);
                self.adaptive_depth.insert(adaptive_run_id, 0);
                self.adaptive_total_decisions.insert(adaptive_run_id, 0);
                self.adaptive_repair_attempts.insert(adaptive_run_id, 0);
                self.adaptive_layer_failures.insert(adaptive_run_id, 0);
                self.adaptive_total_spawned_members.insert(adaptive_run_id, 0);
                self.adaptive_active_members.insert(adaptive_run_id, 0);
                self.adaptive_retained_layer_mobs.insert(adaptive_run_id, 0);
                self.adaptive_aggregate_token_reserved.insert(adaptive_run_id, 0);
                self.adaptive_aggregate_token_actual.insert(adaptive_run_id, 0);
                self.adaptive_aggregate_tool_call_reserved.insert(adaptive_run_id, 0);
                self.adaptive_aggregate_tool_call_actual.insert(adaptive_run_id, 0);
            }
            to Running
            emit AdaptiveRunInitialized { adaptive_run_id: adaptive_run_id }
        }

        transition RecordAdaptivePlanningDecisionActive {
            per_phase [Running]
            on input RecordPlanningDecision { adaptive_run_id, decision_kind }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "decision_limit_available" {
                self.adaptive_total_decisions.get_cloned(adaptive_run_id).get("value")
                < self.adaptive_limit_max_total_decisions.get_cloned(adaptive_run_id).get("value")
            }
            update {
                self.adaptive_total_decisions.insert(
                    adaptive_run_id,
                    self.adaptive_total_decisions.get_cloned(adaptive_run_id).get("value") + 1
                );
            }
            to Running
            emit AdaptivePlanningDecisionRecorded { adaptive_run_id: adaptive_run_id, decision_kind: decision_kind }
        }

        transition RecordAdaptivePlanningDecisionPlanLimit {
            per_phase [Running]
            on input RecordPlanningDecision { adaptive_run_id, decision_kind }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "decision_limit_exhausted" {
                self.adaptive_total_decisions.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_limit_max_total_decisions.get_cloned(adaptive_run_id).get("value")
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Failed);
                self.adaptive_stop_reason.insert(adaptive_run_id, AdaptiveStopReason::PlanLimit);
                self.adaptive_active_run = None;
            }
            to Running
            emit AdaptiveRunTerminalized { adaptive_run_id: adaptive_run_id, reason: AdaptiveStopReason::PlanLimit }
        }

        transition RecordAdaptivePlanRejectedActive {
            per_phase [Running]
            on input RecordPlanRejected { adaptive_run_id, layer_id }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "repair_limit_available" {
                self.adaptive_repair_attempts.get_cloned(adaptive_run_id).get("value")
                < self.adaptive_limit_max_repair_attempts.get_cloned(adaptive_run_id).get("value")
            }
            update {
                self.adaptive_repair_attempts.insert(
                    adaptive_run_id,
                    self.adaptive_repair_attempts.get_cloned(adaptive_run_id).get("value") + 1
                );
            }
            to Running
            emit AdaptivePlanRejected { adaptive_run_id: adaptive_run_id, layer_id: layer_id }
        }

        transition RecordAdaptivePlanRejectedRepairLimit {
            per_phase [Running]
            on input RecordPlanRejected { adaptive_run_id, layer_id }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "repair_limit_exhausted" {
                self.adaptive_repair_attempts.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_limit_max_repair_attempts.get_cloned(adaptive_run_id).get("value")
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Failed);
                self.adaptive_stop_reason.insert(adaptive_run_id, AdaptiveStopReason::RepairLimit);
                self.adaptive_active_run = None;
            }
            to Running
            emit AdaptiveRunTerminalized { adaptive_run_id: adaptive_run_id, reason: AdaptiveStopReason::RepairLimit }
        }

        transition ResolveAdaptiveLayerAdmissionAllowed {
            per_phase [Running]
            on input ResolveLayerAdmission {
                adaptive_run_id,
                layer_id,
                attempt,
                plan_digest,
                child_mob_id,
                member_count,
                token_reservation,
                tool_call_reservation,
                used_model_classes,
                used_tool_classes,
                used_skill_identities,
                used_auth_binding_refs,
                observed_at_ms
            }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "deadline_available" {
                observed_at_ms <= self.adaptive_deadline_ms.get_cloned(adaptive_run_id).get("value")
            }
            guard "depth_available" {
                self.adaptive_depth.get_cloned(adaptive_run_id).get("value")
                < self.adaptive_limit_max_depth.get_cloned(adaptive_run_id).get("value")
            }
            guard "failure_limit_available" {
                self.adaptive_layer_failures.get_cloned(adaptive_run_id).get("value")
                < self.adaptive_limit_max_layer_failures.get_cloned(adaptive_run_id).get("value")
            }
            guard "member_count_available" {
                member_count <= self.adaptive_limit_max_members_per_layer.get_cloned(adaptive_run_id).get("value")
            }
            guard "total_spawn_available" {
                self.adaptive_total_spawned_members.get_cloned(adaptive_run_id).get("value") + member_count
                <= self.adaptive_limit_max_total_spawned_members.get_cloned(adaptive_run_id).get("value")
            }
            guard "active_member_capacity_available" {
                self.adaptive_active_members.get_cloned(adaptive_run_id).get("value") + member_count
                <= self.adaptive_limit_max_active_members.get_cloned(adaptive_run_id).get("value")
            }
            guard "token_reservation_available" {
                self.adaptive_aggregate_token_reserved.get_cloned(adaptive_run_id).get("value") + token_reservation
                <= self.adaptive_limit_max_aggregate_tokens.get_cloned(adaptive_run_id).get("value")
            }
            guard "tool_call_reservation_available" {
                self.adaptive_aggregate_tool_call_reserved.get_cloned(adaptive_run_id).get("value") + tool_call_reservation
                <= self.adaptive_limit_max_aggregate_tool_calls.get_cloned(adaptive_run_id).get("value")
            }
            guard "model_classes_allowed" {
                for_all(class in used_model_classes, self.adaptive_limit_allowed_model_classes.get_cloned(adaptive_run_id).get("value").contains(class))
            }
            guard "tool_classes_allowed" {
                for_all(class in used_tool_classes, self.adaptive_limit_allowed_tool_classes.get_cloned(adaptive_run_id).get("value").contains(class))
            }
            guard "skill_identities_allowed" {
                for_all(skill in used_skill_identities, self.adaptive_limit_allowed_skill_identities.get_cloned(adaptive_run_id).get("value").contains(skill))
            }
            guard "auth_bindings_allowed" {
                for_all(auth in used_auth_binding_refs, self.adaptive_limit_allowed_auth_binding_refs.get_cloned(adaptive_run_id).get("value").contains(auth))
            }
            guard "no_active_layer" {
                self.adaptive_active_layer.get_cloned(adaptive_run_id) == None
            }
            guard "layer_not_previously_admitted" {
                self.adaptive_layer_phase.get_cloned(layer_id) == None
            }
            update {
                self.adaptive_depth.insert(
                    adaptive_run_id,
                    self.adaptive_depth.get_cloned(adaptive_run_id).get("value") + 1
                );
                self.adaptive_total_spawned_members.insert(
                    adaptive_run_id,
                    self.adaptive_total_spawned_members.get_cloned(adaptive_run_id).get("value") + member_count
                );
                self.adaptive_active_members.insert(
                    adaptive_run_id,
                    self.adaptive_active_members.get_cloned(adaptive_run_id).get("value") + member_count
                );
                self.adaptive_aggregate_token_reserved.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_token_reserved.get_cloned(adaptive_run_id).get("value") + token_reservation
                );
                self.adaptive_aggregate_tool_call_reserved.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_tool_call_reserved.get_cloned(adaptive_run_id).get("value") + tool_call_reservation
                );
                self.adaptive_active_layer.insert(adaptive_run_id, layer_id);
                self.adaptive_layer_adaptive_run.insert(layer_id, adaptive_run_id);
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::Admitted);
                self.adaptive_layer_attempt.insert(layer_id, attempt);
                self.adaptive_layer_member_count.insert(layer_id, member_count);
                self.adaptive_layer_plan_digest.insert(layer_id, plan_digest);
                self.adaptive_layer_child_mob_id.insert(layer_id, child_mob_id);
                self.adaptive_layer_token_reservation.insert(layer_id, token_reservation);
                self.adaptive_layer_tool_call_reservation.insert(layer_id, tool_call_reservation);
            }
            to Running
            emit AdaptiveLayerAdmissionResolved { adaptive_run_id: adaptive_run_id, layer_id: layer_id, admission: AdaptiveLayerAdmissionKind::Allowed }
        }

        transition ResolveAdaptiveLayerAdmissionRejected {
            per_phase [Running]
            on input ResolveLayerAdmission {
                adaptive_run_id,
                layer_id,
                attempt,
                plan_digest,
                child_mob_id,
                member_count,
                token_reservation,
                tool_call_reservation,
                used_model_classes,
                used_tool_classes,
                used_skill_identities,
                used_auth_binding_refs,
                observed_at_ms
            }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "limit_exceeded" {
                observed_at_ms > self.adaptive_deadline_ms.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_depth.get_cloned(adaptive_run_id).get("value") >= self.adaptive_limit_max_depth.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_layer_failures.get_cloned(adaptive_run_id).get("value") >= self.adaptive_limit_max_layer_failures.get_cloned(adaptive_run_id).get("value")
                || member_count > self.adaptive_limit_max_members_per_layer.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_total_spawned_members.get_cloned(adaptive_run_id).get("value") + member_count > self.adaptive_limit_max_total_spawned_members.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_active_members.get_cloned(adaptive_run_id).get("value") + member_count > self.adaptive_limit_max_active_members.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_aggregate_token_reserved.get_cloned(adaptive_run_id).get("value") + token_reservation > self.adaptive_limit_max_aggregate_tokens.get_cloned(adaptive_run_id).get("value")
                || self.adaptive_aggregate_tool_call_reserved.get_cloned(adaptive_run_id).get("value") + tool_call_reservation > self.adaptive_limit_max_aggregate_tool_calls.get_cloned(adaptive_run_id).get("value")
                || for_all(class in used_model_classes, self.adaptive_limit_allowed_model_classes.get_cloned(adaptive_run_id).get("value").contains(class)) == false
                || for_all(class in used_tool_classes, self.adaptive_limit_allowed_tool_classes.get_cloned(adaptive_run_id).get("value").contains(class)) == false
                || for_all(skill in used_skill_identities, self.adaptive_limit_allowed_skill_identities.get_cloned(adaptive_run_id).get("value").contains(skill)) == false
                || for_all(auth in used_auth_binding_refs, self.adaptive_limit_allowed_auth_binding_refs.get_cloned(adaptive_run_id).get("value").contains(auth)) == false
                || self.adaptive_active_layer.get_cloned(adaptive_run_id) != None
            }
            update {
                self.adaptive_repair_attempts.insert(
                    adaptive_run_id,
                    self.adaptive_repair_attempts.get_cloned(adaptive_run_id).get("value") + 1
                );
            }
            to Running
            emit AdaptiveLayerAdmissionResolved { adaptive_run_id: adaptive_run_id, layer_id: layer_id, admission: AdaptiveLayerAdmissionKind::Denied }
        }

        transition RecordAdaptiveLayerProvisioned {
            per_phase [Running]
            on input RecordLayerProvisioned { adaptive_run_id, layer_id, attempt }
            guard "layer_admitted" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Admitted)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            update {
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::Provisioning);
            }
            to Running
            emit AdaptiveLayerProvisioned { adaptive_run_id: adaptive_run_id, layer_id: layer_id, attempt: attempt }
        }

        transition RecordAdaptiveLayerRunStarted {
            per_phase [Running]
            on input RecordLayerRunStarted { adaptive_run_id, layer_id, attempt, child_run_id }
            guard "layer_provisioning" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Provisioning)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            update {
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::Running);
                self.adaptive_layer_run_id.insert(layer_id, child_run_id);
            }
            to Running
            emit AdaptiveLayerRunStarted { adaptive_run_id: adaptive_run_id, layer_id: layer_id, attempt: attempt, child_run_id: child_run_id }
        }

        transition IngestAdaptiveLayerTerminalSuccess {
            per_phase [Running]
            on input IngestLayerTerminal { adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls }
            guard "layer_running" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Running)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "result_success" { result_class == FlowRunPublicResultClassKind::Success }
            update {
                self.adaptive_aggregate_token_actual.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_token_actual.get_cloned(adaptive_run_id).get("value") + actual_tokens
                );
                self.adaptive_aggregate_tool_call_actual.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_tool_call_actual.get_cloned(adaptive_run_id).get("value") + actual_tool_calls
                );
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::Collecting);
            }
            to Running
            emit AdaptiveLayerTerminalIngested { adaptive_run_id: adaptive_run_id, layer_id: layer_id, result_class: result_class }
        }

        transition IngestAdaptiveLayerTerminalFailure {
            per_phase [Running]
            on input IngestLayerTerminal { adaptive_run_id, layer_id, attempt, result_class, actual_tokens, actual_tool_calls }
            guard "layer_running" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Running)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "result_failure" { result_class == FlowRunPublicResultClassKind::Failure }
            update {
                self.adaptive_aggregate_token_actual.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_token_actual.get_cloned(adaptive_run_id).get("value") + actual_tokens
                );
                self.adaptive_aggregate_tool_call_actual.insert(
                    adaptive_run_id,
                    self.adaptive_aggregate_tool_call_actual.get_cloned(adaptive_run_id).get("value") + actual_tool_calls
                );
                self.adaptive_layer_failures.insert(
                    adaptive_run_id,
                    self.adaptive_layer_failures.get_cloned(adaptive_run_id).get("value") + 1
                );
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::RunFailed);
                self.adaptive_active_layer.remove(adaptive_run_id);
            }
            to Running
            emit AdaptiveLayerTerminalIngested { adaptive_run_id: adaptive_run_id, layer_id: layer_id, result_class: result_class }
        }

        transition RecordAdaptiveLayerSetupFault {
            per_phase [Running]
            on input RecordLayerSetupFault { adaptive_run_id, layer_id, attempt, fault, spawned_members, requested_members }
            guard "layer_admitted_or_provisioning" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Admitted)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Provisioning)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "setup_counts_valid" {
                requested_members == self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
                && spawned_members <= requested_members
            }
            update {
                self.adaptive_layer_failures.insert(
                    adaptive_run_id,
                    self.adaptive_layer_failures.get_cloned(adaptive_run_id).get("value") + 1
                );
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::SetupFailed);
                self.adaptive_layer_fault.insert(layer_id, fault);
                self.adaptive_active_layer.remove(adaptive_run_id);
            }
            to Running
            emit AdaptiveLayerSetupFaultRecorded { adaptive_run_id: adaptive_run_id, layer_id: layer_id, fault: fault }
        }

        transition RecordAdaptiveLayerResultValidated {
            per_phase [Running]
            on input RecordLayerResultValidated { adaptive_run_id, layer_id, attempt, result_digest }
            guard "layer_collecting" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Collecting)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            update {
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::Completed);
                self.adaptive_layer_result_digest.insert(layer_id, result_digest);
                self.adaptive_active_layer.remove(adaptive_run_id);
            }
            to Running
            emit AdaptiveLayerResultValidated { adaptive_run_id: adaptive_run_id, layer_id: layer_id, result_digest: result_digest }
        }

        transition RecordAdaptiveLayerResultInvalid {
            per_phase [Running]
            on input RecordLayerResultInvalid { adaptive_run_id, layer_id, attempt }
            guard "layer_collecting" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Collecting)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            update {
                self.adaptive_layer_failures.insert(
                    adaptive_run_id,
                    self.adaptive_layer_failures.get_cloned(adaptive_run_id).get("value") + 1
                );
                self.adaptive_layer_phase.insert(layer_id, AdaptiveLayerPhase::ResultInvalid);
                self.adaptive_active_layer.remove(adaptive_run_id);
            }
            to Running
            emit AdaptiveLayerResultInvalid { adaptive_run_id: adaptive_run_id, layer_id: layer_id }
        }

        transition RecordAdaptiveLayerMobDestroyed {
            per_phase [Running]
            on input RecordLayerMobDestroyed { adaptive_run_id, layer_id, attempt }
            guard "layer_terminal" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Completed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::SetupFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::RunFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::ResultInvalid)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Canceled)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "active_member_debit_present" {
                self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
            }
            update {
                self.adaptive_active_members.insert(
                    adaptive_run_id,
                    self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                    - self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
                );
                self.adaptive_layer_disposition.insert(layer_id, AdaptiveLayerDispositionKind::Destroyed);
            }
            to Running
            emit AdaptiveLayerCleanupObserved { adaptive_run_id: adaptive_run_id, layer_id: layer_id, disposition: AdaptiveLayerDispositionKind::Destroyed }
        }

        transition RecordAdaptiveLayerMobRetained {
            per_phase [Running]
            on input RecordLayerMobRetained { adaptive_run_id, layer_id, attempt, disposition }
            guard "layer_terminal" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Completed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::SetupFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::RunFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::ResultInvalid)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Canceled)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "retention_capacity_available" {
                self.adaptive_retained_layer_mobs.get_cloned(adaptive_run_id).get("value")
                < self.adaptive_limit_max_retained_layer_mobs.get_cloned(adaptive_run_id).get("value")
            }
            guard "active_member_debit_present" {
                self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
            }
            update {
                self.adaptive_active_members.insert(
                    adaptive_run_id,
                    self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                    - self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
                );
                self.adaptive_retained_layer_mobs.insert(
                    adaptive_run_id,
                    self.adaptive_retained_layer_mobs.get_cloned(adaptive_run_id).get("value") + 1
                );
                self.adaptive_layer_disposition.insert(layer_id, disposition);
            }
            to Running
            emit AdaptiveLayerCleanupObserved { adaptive_run_id: adaptive_run_id, layer_id: layer_id, disposition: disposition }
        }

        transition RecordAdaptiveLayerMobRetainedCleanupRequired {
            per_phase [Running]
            on input RecordLayerMobRetained { adaptive_run_id, layer_id, attempt, disposition }
            guard "layer_terminal" {
                self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Completed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::SetupFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::RunFailed)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::ResultInvalid)
                || self.adaptive_layer_phase.get_cloned(layer_id) == Some(AdaptiveLayerPhase::Canceled)
            }
            guard "attempt_matches" {
                self.adaptive_layer_attempt.get_cloned(layer_id).get("value") == attempt
            }
            guard "retention_capacity_exhausted" {
                self.adaptive_retained_layer_mobs.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_limit_max_retained_layer_mobs.get_cloned(adaptive_run_id).get("value")
            }
            guard "active_member_debit_present" {
                self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                >= self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
            }
            update {
                self.adaptive_active_members.insert(
                    adaptive_run_id,
                    self.adaptive_active_members.get_cloned(adaptive_run_id).get("value")
                    - self.adaptive_layer_member_count.get_cloned(layer_id).get("value")
                );
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::CleanupRequired);
                self.adaptive_layer_disposition.insert(layer_id, disposition);
            }
            to Running
            emit AdaptiveLayerCleanupObserved { adaptive_run_id: adaptive_run_id, layer_id: layer_id, disposition: disposition }
        }

        transition RecordAdaptiveCleanupResolved {
            per_phase [Running]
            on input RecordCleanupResolved { adaptive_run_id }
            guard "cleanup_pause_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::CleanupRequired)
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Active);
            }
            to Running
            emit AdaptiveCleanupResolved { adaptive_run_id: adaptive_run_id }
        }

        transition RecordAdaptiveBodyEvidenceMissing {
            per_phase [Running]
            on input RecordBodyEvidenceMissing { adaptive_run_id, missing_digest }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::EvidenceMissing);
                self.adaptive_missing_body_digest.insert(adaptive_run_id, missing_digest);
            }
            to Running
            emit AdaptiveBodyEvidenceMissing { adaptive_run_id: adaptive_run_id, missing_digest: missing_digest }
        }

        transition ResolveAdaptiveFinishRunning {
            per_phase [Running]
            on input ResolveAdaptiveFinish { adaptive_run_id, final_result_digest }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Finished);
                self.adaptive_stop_reason.insert(adaptive_run_id, AdaptiveStopReason::FinishDecision);
                self.adaptive_active_run = None;
            }
            to Running
            emit AdaptiveRunTerminalized { adaptive_run_id: adaptive_run_id, reason: AdaptiveStopReason::FinishDecision }
        }

        transition RequestAdaptiveCancelRunning {
            per_phase [Running]
            on input RequestAdaptiveCancel { adaptive_run_id }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Canceled);
                self.adaptive_stop_reason.insert(adaptive_run_id, AdaptiveStopReason::HostCancel);
                self.adaptive_active_run = None;
            }
            to Running
            emit AdaptiveRunTerminalized { adaptive_run_id: adaptive_run_id, reason: AdaptiveStopReason::HostCancel }
        }

        transition RecordAdaptiveDeadlineObservedExpired {
            per_phase [Running]
            on input RecordDeadlineObserved { adaptive_run_id, observed_at_ms }
            guard "run_active" {
                self.adaptive_run_phase.get_cloned(adaptive_run_id) == Some(AdaptiveRunPhase::Active)
            }
            guard "deadline_expired" {
                observed_at_ms > self.adaptive_deadline_ms.get_cloned(adaptive_run_id).get("value")
            }
            update {
                self.adaptive_run_phase.insert(adaptive_run_id, AdaptiveRunPhase::Failed);
                self.adaptive_stop_reason.insert(adaptive_run_id, AdaptiveStopReason::DeadlineExceeded);
                self.adaptive_active_run = None;
            }
            to Running
            emit AdaptiveRunTerminalized { adaptive_run_id: adaptive_run_id, reason: AdaptiveStopReason::DeadlineExceeded }
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
        // CommitSpawnMembership{Fresh,FreshPeerOnly,Replacing} (renamed from the
        // old SpawnRunning* transitions): the membership-commit rung of the
        // phase ladder. The original authority guards (coordinator_bound,
        // addressability/digest authorization, identity_not_external_peer) moved
        // to the BeginSpawnExec opener; these commit transitions guard on the
        // per-identity phase == Opened plus the branch discriminator
        // (no_prior_session_binding/replacing/bridge present|absent). The
        // membership update + 4 emits are preserved verbatim from the originals,
        // so the membership facts still mutate atomically in one transition (no
        // torn state, bindings_require_known_identity invariant preserved). The
        // update additionally flips spawn_exec_phase -> MembershipCommitted.
        transition CommitSpawnMembershipFresh {
            on input CommitSpawnMembership { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "spawn_exec_opened" { self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::Opened) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
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
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
                // Phase ladder: Opened -> MembershipCommitted.
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::MembershipCommitted);
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

        transition CommitSpawnMembershipFreshPeerOnly {
            on input CommitSpawnMembership { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "spawn_exec_opened" { self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::Opened) }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
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
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
                // Phase ladder: Opened -> MembershipCommitted.
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::MembershipCommitted);
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

        transition CommitSpawnMembershipReplacing {
            on input CommitSpawnMembership { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "spawn_exec_opened" { self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::Opened) }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            guard "replacing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(replacing.get("value")) }
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
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_profile_names.insert(agent_identity, self.spawn_profile_authority_profile_names.get_cloned(agent_identity).get("value"));
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_session_bindings.insert(agent_identity, bridge_session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.spawn_profile_authority_profile_names.remove(agent_identity);
                self.spawn_profile_authority_models.remove(agent_identity);
                self.spawn_profile_authority_material_digests.remove(agent_identity);
                self.spawn_profile_authority_tool_config_digests.remove(agent_identity);
                self.spawn_profile_authority_skills_digests.remove(agent_identity);
                self.spawn_profile_authority_provider_params_digests.remove(agent_identity);
                self.spawn_profile_authority_output_schema_digests.remove(agent_identity);
                self.spawn_profile_authority_external_addressable.remove(agent_identity);
                self.topology_epoch += 1;
                // Phase ladder: Opened -> MembershipCommitted.
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::MembershipCommitted);
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

        // =====================================================================
        // Spawn-execution phase ladder: opener + final activation commit + abort.
        // A lightweight 3-rung ladder (Begin -> Commit -> Activate). The machine
        // OWNS + VALIDATES the phase ORDER; the shell still runs each individual
        // finalization step. NO per-obligation tracking and NO per-step request
        // effects (those exploded the mob x meerkat composition state space). The
        // opener carries the original Spawn authority guards; activation/abort are
        // total over an absent (Settled) phase entry via LateArrival/Destroyed
        // no-op variants, mirroring KickoffQuiescedIdle/Destroyed.
        // =====================================================================

        // 1. Opener. Three guarded variants carrying the ORIGINAL Spawn authority
        // guards (the guards the renamed CommitSpawnMembership* no longer carry) +
        // a `spawn_exec_settled` guard (no window already open for this id). Sets
        // ONLY spawn_exec_phase[id] = Opened; emits nothing. Does NOT mutate
        // membership and does NOT consume the single-shot spawn_profile_authority_*
        // material (that happens at CommitSpawnMembership). Self-loops to Running.
        transition BeginSpawnExecFresh {
            on input BeginSpawnExec { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "spawn_exec_settled" { self.spawn_exec_phase.contains_key(agent_identity) == false }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_present" { bridge_session_id != None }
            update {
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::Opened);
            }
            to Running
        }

        transition BeginSpawnExecFreshPeerOnly {
            on input BeginSpawnExec { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "spawn_exec_settled" { self.spawn_exec_phase.contains_key(agent_identity) == false }
            guard "no_prior_session_binding" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "replacing_absent" { replacing == None }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_absent" { bridge_session_id == None }
            update {
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::Opened);
            }
            to Running
        }

        transition BeginSpawnExecReplacing {
            on input BeginSpawnExec { agent_identity, agent_runtime_id, fence_token, generation, profile_material_digest, external_addressable, runtime_mode, bridge_session_id, replacing }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "spawn_exec_settled" { self.spawn_exec_phase.contains_key(agent_identity) == false }
            guard "prior_session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "replacing_present" { replacing != None }
            guard "replacing_matches_current" { self.member_session_bindings.get_cloned(agent_identity) == Some(replacing.get("value")) }
            guard "identity_not_external_peer" { mob_machine_external_peer_identity_absent(self.external_peer_edges, agent_identity) }
            guard "spawn_profile_name_authorized" { self.spawn_profile_authority_profile_names.contains_key(agent_identity) == true }
            guard "spawn_profile_authorized" { self.spawn_profile_authority_material_digests.get_cloned(agent_identity) == Some(profile_material_digest) }
            guard "spawn_profile_addressability_authorized" { self.spawn_profile_authority_external_addressable.get_cloned(agent_identity) == Some(external_addressable) }
            guard "bridge_session_present" { bridge_session_id != None }
            update {
                self.spawn_exec_phase.insert(agent_identity, SpawnExecPhase::Opened);
            }
            to Running
        }

        // (removed) Per-step obligation feedback closers
        // (SpawnTrustInstalled/SpawnOverlayUpserted/SpawnMemberEventAppended/
        // SpawnPeerRegistered/SpawnKickoffMarked/SpawnWired/SpawnBindingDrained/
        // SpawnActivated and their *Closed/*LateArrival/*Destroyed variants):
        // the phase ladder tracks no per-step obligations, so the shell runs each
        // step without a per-step machine handshake.

        // 2. Final commit (Opened-membership -> Settled). Guarded ONLY on phase ==
        // MembershipCommitted; removes the spawn_exec_phase entry (-> Settled),
        // leaving no spawn-exec residue (membership facts persist independently).
        // LateArrival/Destroyed keep the input total over an absent entry.
        transition CommitSpawnActivationFinal {
            per_phase [Running, Stopped, Completed]
            on input CommitSpawnActivation { agent_identity }
            guard "spawn_exec_membership_committed" { self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::MembershipCommitted) }
            update {
                self.spawn_exec_phase.remove(agent_identity);
            }
            to Running
        }
        transition CommitSpawnActivationLateArrival {
            per_phase [Running, Stopped, Completed]
            on input CommitSpawnActivation { agent_identity }
            // Exact complement of the Final guard: total over Settled (absent)
            // AND a premature arrival while still Opened (membership not yet
            // committed) — both are accepted no-ops.
            guard "not_committed" { self.spawn_exec_phase.get_cloned(agent_identity) != Some(SpawnExecPhase::MembershipCommitted) }
            update {}
            to Running
        }
        transition CommitSpawnActivationDestroyed {
            on input CommitSpawnActivation { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // 3. Abort (any non-Destroyed phase). Clears the spawn_exec_phase entry
        // (-> Settled); the shell decides which compensator to run from its own
        // step position. No machine-owned compensation routing or request effects.
        // LateArrival (already Settled) + Destroyed keep the input total.
        transition AbortSpawnExecActive {
            per_phase [Running, Stopped, Completed]
            on input AbortSpawnExec { agent_identity }
            guard "spawn_exec_in_flight" { self.spawn_exec_phase.contains_key(agent_identity) == true }
            update {
                self.spawn_exec_phase.remove(agent_identity);
            }
            to Running
        }
        transition AbortSpawnExecLateArrival {
            per_phase [Running, Stopped, Completed]
            on input AbortSpawnExec { agent_identity }
            guard "spawn_exec_settled" { self.spawn_exec_phase.contains_key(agent_identity) == false }
            update {}
            to Running
        }
        transition AbortSpawnExecDestroyed {
            on input AbortSpawnExec { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
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
            update {
                self.desired_members.insert(agent_identity);
                self.members_to_spawn.remove(agent_identity);
                self.members_to_retire.remove(agent_identity);
            }
            to Running
            emit MemberRetainRequired { agent_identity: agent_identity }
        }

        transition EnsureMemberRunningMissing {
            on input EnsureMember { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "coordinator_bound" { self.coordinator_bound == true }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {
                self.desired_members.insert(agent_identity);
                self.members_to_spawn.insert(agent_identity);
                self.members_to_retire.remove(agent_identity);
            }
            to Running
            emit MemberSpawnRequired { agent_identity: agent_identity }
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
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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

        // Replay-only endpoint recovery for the exact durable runtime/session
        // binding. A legacy journal may not have registered an endpoint at
        // spawn, so the fresh arm establishes the first endpoint without
        // claiming a rotation. Replaying the exact same descriptor is a typed
        // no-op. Only an actual descriptor change retains the previous
        // generation endpoint, replaces current authority, and advances the
        // topology epoch so every previously minted trust authority is stale.
        transition RecoverSpawnedMemberPeerEndpointFreshRunning {
            on signal RecoverSpawnedMemberPeerEndpoint { agent_identity, agent_runtime_id, fence_token, generation, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_peer_not_registered" { self.member_peer_ids.contains_key(agent_identity) == false }
            guard "member_endpoint_not_registered" { self.member_peer_endpoints.contains_key(agent_identity) == false }
            guard "prior_endpoint_history_absent" { self.member_prior_peer_endpoints.contains_key(agent_identity) == false }
            guard "peer_id_available_for_identity" {
                mob_machine_member_peer_id_available_for_identity(
                    self.member_peer_endpoints,
                    self.member_prior_peer_endpoints,
                    agent_identity,
                    peer_endpoint)
            }
            update {
                self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(peer_endpoint));
                self.member_peer_endpoints.insert(agent_identity, peer_endpoint);
            }
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RecoverSpawnedMemberPeerEndpointAlreadyCurrentRunning {
            on signal RecoverSpawnedMemberPeerEndpoint { agent_identity, agent_runtime_id, fence_token, generation, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_endpoint_matches" { self.member_peer_endpoints.get_cloned(agent_identity) == Some(peer_endpoint) }
            guard "current_peer_id_matches" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(peer_endpoint)) }
            update {}
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RecoverMemberPeerEndpointFreshRunning {
            on signal RecoverMemberPeerEndpoint { agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == Some(bridge_session_id) }
            guard "member_peer_not_registered" { self.member_peer_ids.contains_key(agent_identity) == false }
            guard "member_endpoint_not_registered" { self.member_peer_endpoints.contains_key(agent_identity) == false }
            guard "prior_endpoint_history_absent" { self.member_prior_peer_endpoints.contains_key(agent_identity) == false }
            // With no durable baseline endpoint there is no exact old PeerId
            // authority to remove from already-connected peers. New journals
            // register before wiring; legacy journals with retained topology
            // must fail closed instead of treating an arbitrary live
            // observation as the historical baseline.
            guard "no_member_wiring_edges" {
                for_all(edge in self.wiring_edges,
                    mob_machine_wiring_edge_contains_identity(edge, agent_identity) == false)
            }
            guard "no_external_peer_edges" {
                mob_machine_member_has_no_external_peer_edges(self.external_peer_edges, agent_identity)
            }
            guard "peer_id_available_for_identity" {
                mob_machine_member_peer_id_available_for_identity(
                    self.member_peer_endpoints,
                    self.member_prior_peer_endpoints,
                    agent_identity,
                    peer_endpoint)
            }
            update {
                self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(peer_endpoint));
                self.member_peer_endpoints.insert(agent_identity, peer_endpoint);
            }
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RecoverMemberPeerEndpointAlreadyCurrentRunning {
            on signal RecoverMemberPeerEndpoint { agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == Some(bridge_session_id) }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_endpoint_matches_recovered" { self.member_peer_endpoints.get_cloned(agent_identity) == Some(peer_endpoint) }
            guard "current_peer_id_matches_recovered" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(peer_endpoint)) }
            update {}
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RecoverMemberPeerEndpointChangedRunning {
            on signal RecoverMemberPeerEndpoint { agent_identity, agent_runtime_id, bridge_session_id, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == Some(bridge_session_id) }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_peer_id_matches_endpoint" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value"))) }
            guard "recovered_endpoint_changed" { self.member_peer_endpoints.get_cloned(agent_identity) != Some(peer_endpoint) }
            // Trust stores are keyed by PeerId and generated mutation
            // preflight deliberately forbids rewriting descriptor material in
            // place. Treat same-PeerId descriptor drift as corruption, not a
            // migration; only a new cryptographic peer identity may rotate.
            guard "recovered_peer_id_changed" {
                mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value"))
                    != mob_machine_member_peer_endpoint_peer_id(peer_endpoint)
            }
            guard "recovered_peer_id_available_for_identity" {
                mob_machine_member_peer_id_available_for_identity(
                    self.member_peer_endpoints,
                    self.member_prior_peer_endpoints,
                    agent_identity,
                    peer_endpoint)
            }
            // External reciprocal trust does not yet have a generated
            // historical-endpoint cleanup handoff. Refuse the migration rather
            // than strand an old local PeerId in an external peer's trust
            // store. Member-to-member edges are covered below.
            guard "no_external_peer_edges" { mob_machine_member_has_no_external_peer_edges(self.external_peer_edges, agent_identity) }
            update {
                self.member_prior_peer_endpoints = mob_machine_member_prior_peer_endpoints_after_migration(
                    self.member_prior_peer_endpoints,
                    agent_identity,
                    self.member_peer_endpoints.get_cloned(agent_identity).get("value"),
                    peer_endpoint
                );
                self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(peer_endpoint));
                self.member_peer_endpoints.insert(agent_identity, peer_endpoint);
                self.topology_epoch += 1;
            }
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RecoverRosterMemberResetRunning {
            on signal RecoverRosterMemberReset { agent_identity, previous_agent_runtime_id, agent_runtime_id, fence_token, generation }
            guard { self.lifecycle_phase == Phase::Running }
            guard "previous_runtime_recovered" { self.live_runtime_ids.contains(previous_agent_runtime_id) == true }
            guard "identity_recovered" { self.identity_to_runtime.get_cloned(agent_identity) == Some(previous_agent_runtime_id) }
            guard "previous_runtime_session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(previous_agent_runtime_id) == false }
            guard "previous_runtime_not_retiring" { self.member_state_markers.get_cloned(previous_agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "previous_runtime_retirement_closed" { self.runtime_retire_pending_sessions.contains_key(previous_agent_runtime_id) == false }
            guard "previous_runtime_retire_refusal_closed" {
                self.runtime_retire_refusal_codes.contains_key(previous_agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(previous_agent_runtime_id) == false
            }
            update {
                self.live_runtime_ids.remove(previous_agent_runtime_id);
                self.runtime_fence_tokens.remove(previous_agent_runtime_id);
                self.member_startup_binding_requested.remove(previous_agent_runtime_id);
                self.member_startup_runtime_ready.remove(previous_agent_runtime_id);
                self.member_startup_ready.remove(previous_agent_runtime_id);
                self.member_state_markers.remove(previous_agent_runtime_id);
                self.remote_runtime_retired_ids.remove(previous_agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(previous_agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(previous_agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(previous_agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(previous_agent_runtime_id);
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
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        // A retirement-start journal is a durable in-flight obligation, not a
        // completed roster removal. Replay keeps the member live-but-Retiring
        // and restores the exact session correlation needed for a routed
        // retire retry. The three arms mirror the releasing, preserving, and
        // peer-only admission shapes without emitting live side effects during
        // recovery.
        transition RecoverRosterMemberRetirementStartedReleasing {
            on signal RecoverRosterMemberRetirementStarted { agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "releasing_present" { releasing != None }
            guard "session_matches_releasing" { session_id == releasing }
            guard "binding_matches_releasing" { self.member_session_bindings.get_cloned(agent_identity) == releasing }
            guard "retiring_peer_endpoint_consistent" { retiring_peer_endpoint == None || self.member_peer_endpoints.contains_key(agent_identity) == false || self.member_peer_endpoints.get_cloned(agent_identity) == retiring_peer_endpoint }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_session_bindings.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                if retiring_peer_endpoint != None {
                    self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(retiring_peer_endpoint.get("value")));
                    self.member_peer_endpoints.insert(agent_identity, retiring_peer_endpoint.get("value"));
                }
                self.topology_epoch += 1;
            }
            to Running
        }

        // Event replay may legitimately encounter the same start marker more
        // than once (for example after an at-least-once journal copy). The
        // releasing arm removed the binding on its first application, so make
        // that already-applied shape explicitly idempotent instead of letting
        // duplicate recovery fail closed as an invalid transition.
        transition RecoverRosterMemberRetirementStartedReleasingAlreadyApplied {
            on signal RecoverRosterMemberRetirementStarted { agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "releasing_present" { releasing != None }
            guard "session_matches_releasing" { session_id == releasing }
            guard "binding_already_released" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "member_already_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_already_pending" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "ingress_detach_still_pending" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == true }
            guard "retiring_peer_endpoint_consistent" { retiring_peer_endpoint == None || self.member_peer_endpoints.contains_key(agent_identity) == false || self.member_peer_endpoints.get_cloned(agent_identity) == retiring_peer_endpoint }
            update {
                if retiring_peer_endpoint != None {
                    self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(retiring_peer_endpoint.get("value")));
                    self.member_peer_endpoints.insert(agent_identity, retiring_peer_endpoint.get("value"));
                }
            }
            to Running
        }

        transition RecoverRosterMemberRetirementStartedPreservingBinding {
            on signal RecoverRosterMemberRetirementStarted { agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "releasing_absent" { releasing == None }
            guard "session_present" { session_id != None }
            guard "binding_matches_session" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "retiring_peer_endpoint_consistent" { retiring_peer_endpoint == None || self.member_peer_endpoints.contains_key(agent_identity) == false || self.member_peer_endpoints.get_cloned(agent_identity) == retiring_peer_endpoint }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                if retiring_peer_endpoint != None {
                    self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(retiring_peer_endpoint.get("value")));
                    self.member_peer_endpoints.insert(agent_identity, retiring_peer_endpoint.get("value"));
                }
            }
            to Running
        }

        transition RecoverRosterMemberRetirementStartedPeerOnly {
            on signal RecoverRosterMemberRetirementStarted { agent_identity, agent_runtime_id, generation, releasing, session_id, retiring_peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "releasing_absent" { releasing == None }
            guard "session_absent" { session_id == None }
            guard "binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retiring_peer_endpoint_consistent" { retiring_peer_endpoint == None || self.member_peer_endpoints.contains_key(agent_identity) == false || self.member_peer_endpoints.get_cloned(agent_identity) == retiring_peer_endpoint }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                if retiring_peer_endpoint != None {
                    self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(retiring_peer_endpoint.get("value")));
                    self.member_peer_endpoints.insert(agent_identity, retiring_peer_endpoint.get("value"));
                }
            }
            to Running
        }

        transition RecoverRemoteMemberRuntimeRetiredFresh {
            per_phase [Running, Stopped]
            on signal RecoverRemoteMemberRuntimeRetired { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_not_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == false }
            update {
                self.remote_runtime_retired_ids.insert(agent_runtime_id);
            }
            to Running
        }

        transition RecoverRemoteMemberRuntimeRetiredAlreadyRecorded {
            per_phase [Running, Stopped]
            on signal RecoverRemoteMemberRuntimeRetired { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
        }

        transition RecoverRemoteMemberSupervisorRevokedFresh {
            per_phase [Running, Stopped]
            on signal RecoverRemoteMemberSupervisorRevoked { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            guard "remote_supervisor_not_recorded" { self.remote_supervisor_revoked_ids.contains(agent_runtime_id) == false }
            update {
                self.remote_supervisor_revoked_ids.insert(agent_runtime_id);
            }
            to Running
        }

        transition RecoverRemoteMemberSupervisorRevokedAlreadyRecorded {
            per_phase [Running, Stopped]
            on signal RecoverRemoteMemberSupervisorRevoked { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            guard "remote_supervisor_recorded" { self.remote_supervisor_revoked_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
        }

        transition RecoverRosterMemberRetiredRunning {
            on signal RecoverRosterMemberRetired { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_recovered" { self.live_runtime_ids.contains(agent_runtime_id) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                // Preserve a replacement spawn's pre-membership Opened phase;
                // only the retired membership's committed phase is terminal.
                if self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::MembershipCommitted) {
                    self.spawn_exec_phase.remove(agent_identity);
                }
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                // A durable MemberRetired event can only have been appended
                // after the live detach handoff closed. Replay therefore
                // consumes the transient obligation reopened by the earlier
                // retirement-start event instead of attempting the external
                // side effect again after terminal completion.
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
                self.identity_to_runtime.remove(agent_identity);
                self.identity_runtime_generations.remove(agent_identity);
                self.identity_runtime_fence_tokens.remove(agent_identity);
                self.member_profile_names.remove(agent_identity);
                self.member_runtime_modes.remove(agent_identity);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.member_kickoff_pending.remove(agent_identity);
                self.member_kickoff_starting.remove(agent_identity);
                self.member_kickoff_callback_pending.remove(agent_identity);
                self.member_kickoff_started.remove(agent_identity);
                self.member_kickoff_failed.remove(agent_identity);
                self.member_kickoff_cancelled.remove(agent_identity);
                self.member_kickoff_error.remove(agent_identity);
                self.external_member_rebind_capability.remove(agent_identity);
                self.members_to_retire.remove(agent_identity);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition RecoverRosterMemberRetiredAlreadyAbsent {
            on signal RecoverRosterMemberRetired { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_absent" { self.identity_to_runtime.contains_key(agent_identity) == false }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
                self.identity_to_runtime.remove(agent_identity);
                self.identity_runtime_generations.remove(agent_identity);
                self.identity_runtime_fence_tokens.remove(agent_identity);
                self.member_profile_names.remove(agent_identity);
                self.member_runtime_modes.remove(agent_identity);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.member_kickoff_pending.remove(agent_identity);
                self.member_kickoff_starting.remove(agent_identity);
                self.member_kickoff_callback_pending.remove(agent_identity);
                self.member_kickoff_started.remove(agent_identity);
                self.member_kickoff_failed.remove(agent_identity);
                self.member_kickoff_cancelled.remove(agent_identity);
                self.member_kickoff_error.remove(agent_identity);
                self.external_member_rebind_capability.remove(agent_identity);
                self.members_to_retire.remove(agent_identity);
            }
            to Running
        }

        transition RecoverRosterMemberRetiredStaleGeneration {
            on signal RecoverRosterMemberRetired { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_remapped" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id)
            }
            update {
                // The identity belongs to a newer incarnation, so retain its
                // identity-scoped facts while converging every runtime-scoped
                // residue owned by the terminal event's older runtime.
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.pending_session_ingress_detach_runtime_ids.remove(agent_runtime_id);
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

        transition RecoverObjectiveBinding {
            on signal RecoverObjectiveBinding { member_id, objective_id }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.member_kickoff_objective_ids.insert(member_id, objective_id);
                if !self.objective_owner_ids.contains_key(objective_id) {
                    self.objective_owner_ids.insert(objective_id, member_id);
                }
            }
            to Running
        }

        transition BindObjectiveOwner {
            per_phase [Running]
            on input BindObjectiveOwner { owner_id, objective_id }
            guard "objective_owner_unbound" { self.objective_owner_ids.contains_key(objective_id) == false }
            update {
                self.objective_owner_ids.insert(objective_id, owner_id);
            }
            to Running
            emit PersistObjectiveOwnerBinding { owner_id: owner_id, objective_id: objective_id }
        }

        transition BindObjectiveOwnerIdempotent {
            per_phase [Running]
            on input BindObjectiveOwner { owner_id, objective_id }
            guard "objective_owner_matches" { self.objective_owner_ids.get_cloned(objective_id) == Some(owner_id) }
            update {}
            to Running
        }

        transition RecoverObjectiveConclusion {
            on signal RecoverObjectiveConclusion { member_id, objective_id, outcome }
            guard { self.lifecycle_phase == Phase::Running }
            guard "objective_belongs_to_lead" { self.objective_owner_ids.get_cloned(objective_id) == Some(member_id) }
            update {
                self.objective_outcomes.insert(objective_id, outcome);
                self.concluded_objective_ids.insert(objective_id);
            }
            to Running
        }

        // Row #351: machine-owned membership reconciliation. The batch diff
        // (desired-vs-current) is recomputed in-DSL from `identity_to_runtime`
        // and stored into the membership-intent sets. The handle reads these
        // sets from the returned snapshot and executes spawn/retire mechanically
        // rather than computing the difference itself. Each ReconcileRunning
        // fully recomputes the sets so successive reconciles stay self-fresh.
        transition ReconcileRunning {
            on input Reconcile { desired, retire_stale }
            guard { self.lifecycle_phase == Phase::Running }
            update {
                self.members_to_spawn = mob_machine_members_to_spawn(self.identity_to_runtime, desired);
                self.members_to_retire = mob_machine_members_to_retire(self.identity_to_runtime, desired, retire_stale);
                self.desired_members = desired;
            }
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

        transition ReconcileDestroyed {
            on input Reconcile { desired, retire_stale }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
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
            on input KickoffMarkPending { member_id, objective_id }
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
                self.member_kickoff_objective_ids.insert(member_id, objective_id);
                if !self.objective_owner_ids.contains_key(objective_id) {
                    self.objective_owner_ids.insert(objective_id, member_id);
                }
            }
            to Running
            emit PersistKickoffUpdate { member_id: member_id, phase: KickoffPhase::Pending }
            emit EmitKickoffLifecycleNotice { member_id: member_id, intent: KickoffIntent::Pending }
        }

        transition ConcludeObjective {
            per_phase [Running]
            on input ConcludeObjective { member_id, objective_id, outcome }
            guard "objective_belongs_to_lead" { self.objective_owner_ids.get_cloned(objective_id) == Some(member_id) }
            guard "objective_not_concluded" { self.concluded_objective_ids.contains(objective_id) == false }
            update {
                self.objective_outcomes.insert(objective_id, outcome);
                self.concluded_objective_ids.insert(objective_id);
            }
            to Running
            emit PersistObjectiveConclusion { member_id: member_id, objective_id: objective_id, outcome: outcome }
        }

        transition ConcludeObjectiveIdempotent {
            per_phase [Running]
            on input ConcludeObjective { member_id, objective_id, outcome }
            guard "objective_belongs_to_lead" { self.objective_owner_ids.get_cloned(objective_id) == Some(member_id) }
            guard "objective_already_concluded" { self.concluded_objective_ids.contains(objective_id) == true }
            guard "objective_outcome_matches" { self.objective_outcomes.get_cloned(objective_id) == Some(outcome) }
            update {}
            to Running
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

        // =====================================================================
        // 0.7.2 disciplined shell inputs — L5 (rows 12/13).
        //
        // `KickoffQuiesced` closes the kickoff-waiter drain obligation opened
        // by the member retire / destroy-retire transitions (the
        // `RequestKickoffQuiesce` effect). It is TOTAL over kickoff state and
        // lifecycle phase: quiescing an in-flight kickoff is a machine-recorded
        // cancellation; quiescing an idle or already-terminal kickoff is an
        // accepted no-op; a post-destroy arrival is an accepted no-op. The
        // shell aborts + awaits the waiter task and fires this input without
        // probing machine state first.
        // =====================================================================

        transition KickoffQuiescedInFlight {
            per_phase [Running, Stopped, Completed]
            on input KickoffQuiesced { member_id }
            guard "kickoff_in_flight" {
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

        transition KickoffQuiescedIdle {
            per_phase [Running, Stopped, Completed]
            on input KickoffQuiesced { member_id }
            guard "kickoff_not_in_flight" {
                !self.member_kickoff_pending.contains(member_id)
                && !self.member_kickoff_starting.contains(member_id)
                && !self.member_kickoff_callback_pending.contains(member_id)
            }
            update {}
            to Running
        }

        transition KickoffQuiescedDestroyed {
            on input KickoffQuiesced { member_id }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // 0.7.2 L5 D2a — late kickoff resolutions are total typed no-ops.
        //
        // The autonomous-kickoff completion waiter is an in-process producer
        // that can legitimately deliver its outcome after teardown quiesced
        // the kickoff (or after the outcome was already in flight when
        // teardown committed). A late `KickoffResolve*` / `KickoffCancel*`
        // arrival must commit as an explicit no-op self-loop instead of a
        // guard rejection the shell has to launder (`Err(_) => Ok(false)`).
        // Precedent: `ObserveCredentialFreshnessValid` no-op self-loop
        // (auth_machine.rs). The Destroyed-phase variants keep the inputs
        // total even when the mob reached Destroyed without per-member
        // quiesce (e.g. `ObserveRuntimeDestroyed` recovery).
        // =====================================================================

        transition KickoffResolveStartedLateArrival {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveStarted { member_id }
            guard "kickoff_not_starting" { !self.member_kickoff_starting.contains(member_id) }
            update {}
            to Running
        }

        transition KickoffResolveStartedDestroyed {
            on input KickoffResolveStarted { member_id }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        transition KickoffResolveCallbackPendingLateArrival {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveCallbackPending { member_id }
            guard "kickoff_not_starting" { !self.member_kickoff_starting.contains(member_id) }
            update {}
            to Running
        }

        transition KickoffResolveCallbackPendingDestroyed {
            on input KickoffResolveCallbackPending { member_id }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        transition KickoffResolveFailedLateArrival {
            per_phase [Running, Stopped, Completed]
            on input KickoffResolveFailed { member_id, error }
            guard "kickoff_not_active" {
                !self.member_kickoff_pending.contains(member_id)
                && !self.member_kickoff_starting.contains(member_id)
                && !self.member_kickoff_callback_pending.contains(member_id)
            }
            update {}
            to Running
        }

        transition KickoffResolveFailedDestroyed {
            on input KickoffResolveFailed { member_id, error }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        transition KickoffCancelRequestedLateArrival {
            per_phase [Running, Stopped, Completed]
            on input KickoffCancelRequested { member_id }
            guard "kickoff_not_active" {
                !self.member_kickoff_pending.contains(member_id)
                && !self.member_kickoff_starting.contains(member_id)
                && !self.member_kickoff_callback_pending.contains(member_id)
            }
            update {}
            to Running
        }

        transition KickoffCancelRequestedDestroyed {
            on input KickoffCancelRequested { member_id }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // Wave-G1 catalog folds: classification / probe / escalation transitions
        // =====================================================================

        // Row #14: duplicate-member admission probe. The verdict is read from
        // machine-owned binding state, not the PendingSpawnLineage side-map.
        // CRITICAL FIX (G1 regression): the Duplicate guard must also reject an
        // in-flight pending spawn (pending_spawn_sessions) so a concurrent
        // second spawn probing before the first records its binding still sees a
        // duplicate. Admitted is the exact negation of all three presence facts.
        transition ProbeMemberAdmissionDuplicate {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ProbeMemberAdmission { agent_identity }
            guard "identity_already_bound_or_pending" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                || self.member_session_bindings.contains_key(agent_identity) == true
                || self.pending_spawn_sessions.contains_key(agent_identity) == true
            }
            update {}
            to Running
            emit MemberAdmissionProbed { agent_identity: agent_identity, verdict: MemberAdmissionVerdictKind::DuplicateRejected }
        }

        transition ProbeMemberAdmissionAdmitted {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ProbeMemberAdmission { agent_identity }
            guard "identity_not_bound_and_not_pending" {
                self.identity_to_runtime.contains_key(agent_identity) == false
                && self.member_session_bindings.contains_key(agent_identity) == false
                && self.pending_spawn_sessions.contains_key(agent_identity) == false
            }
            update {}
            to Running
            emit MemberAdmissionProbed { agent_identity: agent_identity, verdict: MemberAdmissionVerdictKind::Admitted }
        }

        // Row #181: compute the next monotone respawn generation. Reads the
        // machine-owned current Generation for the identity and emits
        // current.saturating_add(1) (starting at 1 when absent).
        transition ComputeRespawnGeneration {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ComputeRespawnGeneration { agent_identity }
            update {}
            to Running
            emit RespawnGenerationComputed {
                agent_identity: agent_identity,
                next_generation: mob_machine_next_respawn_generation(self.identity_runtime_generations, agent_identity)
            }
        }

        // Row #260: classify a typed step-output fault into retry vs terminal.
        // The retry-vs-fail decision is machine-owned; the typed terminal cause
        // flows into terminalization. Fault detail strings stay shell-side.
        transition ClassifyStepOutputFaultRetry {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyStepOutputFault { run_id, step_id, target_retry_key, fault, attempt, max_retries }
            guard "attempts_remaining" { attempt < max_retries }
            update {}
            to Running
            emit StepOutputFaultClassified {
                run_id: run_id,
                step_id: step_id,
                target_retry_key: target_retry_key,
                disposition: StepFaultDispositionKind::Retry,
                terminal_cause: fault
            }
        }

        transition ClassifyStepOutputFaultTerminal {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyStepOutputFault { run_id, step_id, target_retry_key, fault, attempt, max_retries }
            guard "attempts_exhausted" { attempt >= max_retries }
            update {}
            to Running
            emit StepOutputFaultClassified {
                run_id: run_id,
                step_id: step_id,
                target_retry_key: target_retry_key,
                disposition: StepFaultDispositionKind::Terminal,
                terminal_cause: fault
            }
        }

        // Row #261: supervisor escalation. Presence of an eligible target is
        // structural — two inputs, not an Option<AgentIdentity>. Target
        // selection, timeout policy, and escalation evidence are machine-owned;
        // fail-closed is a typed terminal cause, not a free-form string.
        transition EscalateToSupervisorTargetFound {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EscalateToSupervisor { run_id, step_id, supervisor_identity, turn_timeout_ms }
            update {}
            to Running
            emit SupervisorEscalationRequested {
                run_id: run_id,
                step_id: step_id,
                supervisor_identity: supervisor_identity,
                turn_timeout_ms: turn_timeout_ms
            }
        }

        transition EscalateToSupervisorTargetMissing {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EscalateToSupervisorNoEligibleTarget { run_id, step_id }
            update {}
            to Running
            emit SupervisorEscalationFailed {
                run_id: run_id,
                step_id: step_id,
                cause: SupervisorEscalationFailureCause::NoEligibleSupervisor
            }
        }

        // Row #293: topology-edge admission verdict. A matched declarative rule
        // resolves directly; an unmatched edge applies the declared default
        // policy field. Default policy is an explicit machine field, not a
        // shell map_or(Allow, ..).
        transition EvaluateTopologyEdgeRuleAllow {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EvaluateTopologyEdge { from_role, to_role, rule_match }
            guard "rule_allows_edge" { rule_match == Some(PolicyDecision::Allow) }
            update {}
            to Running
            emit TopologyEdgeVerdictResolved { from_role: from_role, to_role: to_role, verdict: PolicyDecision::Allow }
        }

        transition EvaluateTopologyEdgeRuleDeny {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EvaluateTopologyEdge { from_role, to_role, rule_match }
            guard "rule_denies_edge" { rule_match == Some(PolicyDecision::Deny) }
            update {}
            to Running
            emit TopologyEdgeVerdictResolved { from_role: from_role, to_role: to_role, verdict: PolicyDecision::Deny }
        }

        transition EvaluateTopologyEdgeDefaultAllow {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EvaluateTopologyEdge { from_role, to_role, rule_match }
            guard "no_rule_and_default_allow" {
                rule_match == None
                && self.topology_default_policy == PolicyDecision::Allow
            }
            update {}
            to Running
            emit TopologyEdgeVerdictResolved { from_role: from_role, to_role: to_role, verdict: PolicyDecision::Allow }
        }

        transition EvaluateTopologyEdgeDefaultDeny {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input EvaluateTopologyEdge { from_role, to_role, rule_match }
            guard "no_rule_and_default_deny" {
                rule_match == None
                && self.topology_default_policy == PolicyDecision::Deny
            }
            update {}
            to Running
            emit TopologyEdgeVerdictResolved { from_role: from_role, to_role: to_role, verdict: PolicyDecision::Deny }
        }

        // Row #314: external-member rebind capability. The bridge supplies the
        // typed capability bit; the projection reads the machine map directly.
        transition SetExternalMemberRebindCapability {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input SetExternalMemberRebindCapability { agent_identity, capability }
            update {
                self.external_member_rebind_capability.insert(agent_identity, capability);
            }
            to Running
        }

        // Row #320: one-time orphan-budget seed. Dispatched once at actor build
        // from definition.limits.max_orphaned_turns (default 8). Without this,
        // every non-retryable timeout would classify Canceled.
        transition SeedOrphanBudget {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input SeedOrphanBudget { budget }
            update {
                self.orphan_budget = budget;
            }
            to Running
        }

        // Row #320: turn-timeout disposition. A retryable timeout is Retryable;
        // a non-retryable timeout detaches while orphan_budget > 0 (decrementing
        // it), otherwise cancels. The detach/cancel verdict and the orphan
        // budget are machine-owned.
        transition ClassifyTurnTimeoutDispositionRetryable {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyTurnTimeoutDisposition { timed_out_run_id, retryable }
            guard "timeout_is_retryable" { retryable == true }
            update {}
            to Running
            emit TurnTimeoutDispositionClassified {
                timed_out_run_id: timed_out_run_id,
                disposition: TurnTimeoutDisposition::Retryable
            }
        }

        transition ClassifyTurnTimeoutDispositionDetached {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyTurnTimeoutDisposition { timed_out_run_id, retryable }
            guard "non_retryable_with_budget" {
                retryable == false
                && self.orphan_budget > 0
            }
            update {
                self.orphan_budget -= 1;
            }
            to Running
            emit TurnTimeoutDispositionClassified {
                timed_out_run_id: timed_out_run_id,
                disposition: TurnTimeoutDisposition::Detached
            }
        }

        transition ClassifyTurnTimeoutDispositionCanceled {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ClassifyTurnTimeoutDisposition { timed_out_run_id, retryable }
            guard "non_retryable_without_budget" {
                retryable == false
                && self.orphan_budget == 0
            }
            update {}
            to Running
            emit TurnTimeoutDispositionClassified {
                timed_out_run_id: timed_out_run_id,
                disposition: TurnTimeoutDisposition::Canceled
            }
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

        // Generated routed-effect refusal closure. The composition binds each
        // consumer refusal back to one of these typed inputs. The shell never
        // chooses terminality from a generic dispatch error:
        //
        // * binding refusal is the only arm that records Broken/restore
        //   terminality for the affected identity;
        // * ingress refusal terminates only that work delivery;
        // * retire refusal preserves a retry anchor while the member remains
        //   Retiring.
        transition ResolveRuntimeBindingRefusalRunning {
            on input ResolveRuntimeBindingRefusal { agent_identity, agent_runtime_id, session_id, refusal_code, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == Some(session_id) }
            update {
                self.member_restore_failures.insert(agent_identity, reason);
                self.member_restore_failure_codes.insert(agent_identity, refusal_code);
            }
            to Running
            emit RuntimeBindingRefusalClassified {
                agent_identity: agent_identity,
                agent_runtime_id: agent_runtime_id,
                session_id: session_id,
                refusal_code: refusal_code,
                reason: reason
            }
        }

        transition ResolveRuntimeIngressRefusalRunning {
            on input ResolveRuntimeIngressRefusal { agent_runtime_id, fence_token, session_id, work_id, origin, refusal_code, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {}
            to Running
            emit RuntimeIngressRefusalClassified {
                agent_runtime_id: agent_runtime_id,
                session_id: session_id,
                work_id: work_id,
                origin: origin,
                refusal_code: refusal_code,
                reason: reason
            }
        }

        transition ResolveRuntimeRetireRefusalRunning {
            on input ResolveRuntimeRetireRefusal { agent_identity, agent_runtime_id, session_id, refusal_code, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.runtime_retire_refusal_codes.insert(agent_runtime_id, refusal_code);
                self.runtime_retire_refusal_reasons.insert(agent_runtime_id, reason);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id);
            }
            to Running
            emit RuntimeRetireRefusalClassified {
                agent_identity: agent_identity,
                agent_runtime_id: agent_runtime_id,
                session_id: session_id,
                refusal_code: refusal_code,
                reason: reason
            }
        }

        transition ResolveRuntimeRetireRefusalStopped {
            on input ResolveRuntimeRetireRefusal { agent_identity, agent_runtime_id, session_id, refusal_code, reason }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.runtime_retire_refusal_codes.insert(agent_runtime_id, refusal_code);
                self.runtime_retire_refusal_reasons.insert(agent_runtime_id, reason);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id);
            }
            to Stopped
            emit RuntimeRetireRefusalClassified {
                agent_identity: agent_identity,
                agent_runtime_id: agent_runtime_id,
                session_id: session_id,
                refusal_code: refusal_code,
                reason: reason
            }
        }

        transition RetryRuntimeRetireRunning {
            on input RetryRuntimeRetire { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_pending" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == true }
            update {}
            to Running
            emit RequestRuntimeRetire {
                agent_identity: agent_identity,
                agent_runtime_id: agent_runtime_id,
                session_id: self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id).get("value")
            }
        }

        transition RetryRuntimeRetireStopped {
            on input RetryRuntimeRetire { agent_identity, agent_runtime_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_pending" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == true }
            update {}
            to Stopped
            emit RequestRuntimeRetire {
                agent_identity: agent_identity,
                agent_runtime_id: agent_runtime_id,
                session_id: self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id).get("value")
            }
        }

        transition RecordRemoteMemberRuntimeRetiredFresh {
            per_phase [Running, Stopped]
            on input RecordRemoteMemberRuntimeRetired { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_not_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == false }
            update {
                self.remote_runtime_retired_ids.insert(agent_runtime_id);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RemoteMemberRuntimeRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: None
            }
        }

        transition RecordRemoteMemberRuntimeRetiredAlreadyRecorded {
            per_phase [Running, Stopped]
            on input RecordRemoteMemberRuntimeRetired { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RemoteMemberRuntimeRetired,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: None
            }
        }

        transition RecordRemoteMemberSupervisorRevokedFresh {
            per_phase [Running, Stopped]
            on input RecordRemoteMemberSupervisorRevoked { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            guard "remote_supervisor_not_recorded" { self.remote_supervisor_revoked_ids.contains(agent_runtime_id) == false }
            update {
                self.remote_supervisor_revoked_ids.insert(agent_runtime_id);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RemoteMemberSupervisorRevoked,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: None
            }
        }

        transition RecordRemoteMemberSupervisorRevokedAlreadyRecorded {
            per_phase [Running, Stopped]
            on input RecordRemoteMemberSupervisorRevoked { agent_identity, agent_runtime_id, fence_token, generation }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_runtime_recorded" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            guard "remote_supervisor_recorded" { self.remote_supervisor_revoked_ids.contains(agent_runtime_id) == true }
            update {}
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::RemoteMemberSupervisorRevoked,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: Some(fence_token),
                generation: Some(generation),
                session_id: None
            }
        }

        transition AdmitDestroyMemberRetireLiveRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireLiveStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetirePeerOnlyRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPeerOnly,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: None
            }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetirePeerOnlyStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            update {
                self.member_state_markers.insert(agent_runtime_id, MobMemberState::Retiring);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPeerOnly,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: None
            }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringSessionRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            update {
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Running
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringSessionStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_present" { session_id != None }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            update {
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Stopped
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        // Ordinary releasing retirement consumes the live session binding as
        // soon as detach authority opens. A later destroy takeover therefore
        // proves session ownership through the still-durable pending retire
        // correlation, not through the already-released binding.
        transition AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_present" { session_id != None }
            guard "session_binding_released" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            update {
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Running
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_present" { session_id != None }
            guard "session_binding_released" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            update {
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
            }
            to Stopped
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringPeerOnlyRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            update {}
            to Running
            // Idempotent re-admission still re-requests the kickoff quiesce so
            // a destroy retry can re-discharge a previously dropped obligation.
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyRetiringPeerOnlyStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_absent" { session_id == None }
            guard "session_binding_absent" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "session_ingress_detach_absent" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "retire_refusal_absent" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            update {}
            to Stopped
            emit RequestKickoffQuiesce { member_id: agent_identity }
        }

        transition AdmitDestroyMemberRetireAlreadyArchivedRunning {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_cleared" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retry_observes_cleared_session" { session_id == None }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "runtime_retirement_closed" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "runtime_retire_refusal_closed" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
            }
            update {}
            to Running
        }

        transition AdmitDestroyMemberRetireAlreadyArchivedStopped {
            on signal AdmitDestroyMemberRetire { mob_id, agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_cleared" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retry_observes_cleared_session" { session_id == None }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "runtime_retirement_closed" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "runtime_retire_refusal_closed" {
                self.runtime_retire_refusal_codes.contains_key(agent_runtime_id) == false
                && self.runtime_retire_refusal_reasons.contains_key(agent_runtime_id) == false
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

        transition ObserveRuntimeRetiredStopped {
            on signal ObserveRuntimeRetired { agent_runtime_id, fence_token }
            guard { self.lifecycle_phase == Phase::Stopped }
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
            to Stopped
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        // Peer-only terminality is a distinct exact proof path. The shell may
        // emit this signal only after the remote retirement checkpoint is
        // durable and direct supervisor revocation has acknowledged (or
        // returned the typed idempotent NotBound result). Generic session
        // archive observations cannot terminalize this sessionless shape.
        transition ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked {
            per_phase [Running, Stopped]
            on signal ObserveRemoteMemberRetirementArchivedAndSupervisorRevoked { agent_identity, agent_runtime_id, fence_token, generation }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "remote_runtime_retired" { self.remote_runtime_retired_ids.contains(agent_runtime_id) == true }
            guard "remote_supervisor_revoked" { self.remote_supervisor_revoked_ids.contains(agent_runtime_id) == true }
            guard "remote_member_session_absent" { self.member_session_bindings.contains_key(agent_identity) == false }
            guard "remote_retirement_session_absent" { self.runtime_retire_pending_sessions.contains_key(agent_runtime_id) == false }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.active_run_count = 0;
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
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedLive {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            // 0.7.2 L5: retirement archival is the member-level teardown
            // completion; it may only commit after the kickoff-waiter drain
            // obligation closed (`KickoffQuiesced` / a natural `KickoffResolve*`
            // moved the kickoff out of the in-flight states).
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.active_run_count = 0;
            }
            to Running
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedLiveStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.active_run_count = 0;
            }
            to Stopped
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Retired }
        }

        transition ObserveMemberRetirementArchivedRetired {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
            }
            to Running
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedRetiredStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
            }
            to Stopped
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedStaleRuntime {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_remapped" { self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
            }
            to Running
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedStaleRuntimeStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_remapped" { self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "session_present" { session_id != None }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
            }
            to Stopped
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedAlreadyCleared {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Running
            // Re-emit durable-write authority for a retry that landed after
            // the first transition committed but before the shell released
            // its retained roster anchor. The event store remains the
            // idempotency owner and skips an already-present exact event.
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedAlreadyClearedStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Stopped
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedStaleRuntimeAlreadyCleared {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_remapped" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id)
            }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Running
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveMemberRetirementArchivedStaleRuntimeAlreadyClearedStopped {
            on signal ObserveMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_remapped" {
                self.identity_to_runtime.contains_key(agent_identity) == true
                && self.identity_to_runtime.get_cloned(agent_identity) != Some(agent_runtime_id)
            }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            update {}
            to Stopped
            emit AppendLifecycleJournal { kind: MobLifecycleJournalKind::MemberRetired, agent_identity: Some(agent_identity), agent_runtime_id: Some(agent_runtime_id), fence_token: None, generation: Some(generation), session_id: session_id }
        }

        transition ObserveDestroyMemberRetirementArchivedLiveRunning {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "session_present" { session_id != None }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "session_present" { session_id != None }
            guard "runtime_live" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.live_runtime_ids.remove(agent_runtime_id);
                self.externally_addressable_runtime_ids.remove(agent_runtime_id);
                self.runtime_fence_tokens.remove(agent_runtime_id);
                self.member_startup_binding_requested.remove(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "session_present" { session_id != None }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "session_binding_matches" { self.member_session_bindings.get_cloned(agent_identity) == session_id }
            guard "session_present" { session_id != None }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retirement_session_matches" { self.runtime_retire_pending_sessions.get_cloned(agent_runtime_id) == session_id }
            guard "kickoff_quiesced" {
                !self.member_kickoff_pending.contains(agent_identity)
                && !self.member_kickoff_starting.contains(agent_identity)
                && !self.member_kickoff_callback_pending.contains(agent_identity)
            }
            update {
                self.member_state_markers.remove(agent_runtime_id);
                self.remote_runtime_retired_ids.remove(agent_runtime_id);
                self.remote_supervisor_revoked_ids.remove(agent_runtime_id);
                self.runtime_retire_refusal_codes.remove(agent_runtime_id);
                self.runtime_retire_refusal_reasons.remove(agent_runtime_id);
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_session_bindings.remove(agent_identity);
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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

        transition ObserveDestroyMemberRetirementArchivedAlreadyClearedRunning {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_cleared" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retry_observes_cleared_session" { session_id == None }
            update {}
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

        transition ObserveDestroyMemberRetirementArchivedAlreadyClearedStopped {
            on signal ObserveDestroyMemberRetirementArchived { agent_identity, agent_runtime_id, fence_token, generation, session_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "destroy_admitted" { self.destroy_admitted == true }
            guard "session_ingress_detach_closed" { self.pending_session_ingress_detach_runtime_ids.contains(agent_runtime_id) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "runtime_not_live" { self.live_runtime_ids.contains(agent_runtime_id) == false }
            guard "member_not_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring) }
            guard "session_binding_cleared" { self.member_session_bindings.get_cloned(agent_identity) == None }
            guard "retry_observes_cleared_session" { session_id == None }
            update {}
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

        transition ResetMember {
            on signal ResetMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id }
            guard {
                self.lifecycle_phase == Phase::Running
                || self.lifecycle_phase == Phase::Stopped
            }
            guard "identity_binding_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "previous_runtime_session_ingress_detach_closed" {
                self.pending_session_ingress_detach_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            guard "previous_runtime_not_retiring" {
                self.member_state_markers.get_cloned(self.identity_to_runtime.get_cloned(agent_identity).get("value")) != Some(MobMemberState::Retiring)
            }
            guard "previous_runtime_retirement_closed" {
                self.runtime_retire_pending_sessions.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            guard "previous_runtime_retire_refusal_closed" {
                self.runtime_retire_refusal_codes.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
                && self.runtime_retire_refusal_reasons.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.live_runtime_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.externally_addressable_runtime_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_fence_tokens.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_binding_requested.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_runtime_ready.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_ready.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_state_markers.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.remote_runtime_retired_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.remote_supervisor_revoked_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_refusal_codes.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_refusal_reasons.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_pending_sessions.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
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
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
            }
            to Running
            emit RequestRuntimeBinding { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, fence_token: fence_token, generation: Some(generation), session_id: session_id }
            emit EmitMemberLifecycleNotice { kind: MemberLifecycleKind::Reset }
        }

        transition RespawnMember {
            on signal RespawnMember { agent_identity, agent_runtime_id, fence_token, generation, profile_name, runtime_mode, external_addressable, session_id }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_binding_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "previous_runtime_session_ingress_detach_closed" {
                self.pending_session_ingress_detach_runtime_ids.contains(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            guard "previous_runtime_not_retiring" {
                self.member_state_markers.get_cloned(self.identity_to_runtime.get_cloned(agent_identity).get("value")) != Some(MobMemberState::Retiring)
            }
            guard "previous_runtime_retirement_closed" {
                self.runtime_retire_pending_sessions.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            guard "previous_runtime_retire_refusal_closed" {
                self.runtime_retire_refusal_codes.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
                && self.runtime_retire_refusal_reasons.contains_key(self.identity_to_runtime.get_cloned(agent_identity).get("value")) == false
            }
            update {
                self.active_run_count = 0;
                self.pending_spawn_count = 0;
                self.pending_spawn_sessions = EmptyMap;
                self.live_runtime_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.externally_addressable_runtime_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_fence_tokens.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_binding_requested.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_runtime_ready.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_startup_ready.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.member_state_markers.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.remote_runtime_retired_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.remote_supervisor_revoked_ids.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_refusal_codes.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_refusal_reasons.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
                self.runtime_retire_pending_sessions.remove(self.identity_to_runtime.get_cloned(agent_identity).get("value"));
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
                self.member_peer_ids.remove(agent_identity);
                self.member_peer_endpoints.remove(agent_identity);
                self.member_prior_peer_endpoints.remove(agent_identity);
                self.member_profile_names.insert(agent_identity, profile_name);
                self.member_runtime_modes.insert(agent_identity, runtime_mode);
                self.member_startup_binding_requested.insert(agent_runtime_id);
                self.member_startup_runtime_ready.remove(agent_runtime_id);
                self.member_startup_ready.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
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
                self.member_restore_failure_codes.remove(agent_identity);
            }
            to Running
        }

        // Post-discard member revival seam (#37). The shell observes "the
        // member's current bridge session has no live materialization" at the
        // dispatch boundary and feeds the raw observation (durable snapshot
        // present or missing) here; MobMachine — never the session-registry
        // cache — owns the verdict. A durable snapshot makes the member
        // revivable: the machine records the revival obligation and authorizes
        // exactly one shell materialization attempt via the emitted
        // ReviveAuthorized verdict. A missing durable snapshot is a terminal
        // restore failure (existing Broken classification). Resolution comes
        // back as a typed signal: success clears the obligation; failure
        // converts it into the Broken classification, whose `not_broken`
        // guard below refuses any further revival authorization — fail-closed,
        // no retry loop.
        transition ClassifyMemberLiveMaterializationRevivable {
            on signal ClassifyMemberLiveMaterialization { agent_identity, observation, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "not_broken" { self.member_restore_failures.contains_key(agent_identity) == false }
            guard "durable_snapshot_present" { observation == MemberLiveMaterializationObservationKind::DurableSnapshotPresent }
            update {
                self.member_revival_pending.insert(agent_identity);
            }
            to Running
            emit MemberLiveMaterializationClassified {
                agent_identity: agent_identity,
                observation: observation,
                verdict: MemberRevivalVerdictKind::ReviveAuthorized,
                reason: reason
            }
        }

        transition ClassifyMemberLiveMaterializationTerminal {
            on signal ClassifyMemberLiveMaterialization { agent_identity, observation, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "session_binding_present" { self.member_session_bindings.contains_key(agent_identity) == true }
            guard "not_broken" { self.member_restore_failures.contains_key(agent_identity) == false }
            guard "durable_snapshot_missing" { observation == MemberLiveMaterializationObservationKind::DurableSnapshotMissing }
            update {
                self.member_revival_pending.remove(agent_identity);
                self.member_restore_failures.insert(agent_identity, reason);
                self.member_restore_failure_codes.remove(agent_identity);
            }
            to Running
            emit MemberLiveMaterializationClassified {
                agent_identity: agent_identity,
                observation: observation,
                verdict: MemberRevivalVerdictKind::BrokenRecorded,
                reason: reason
            }
        }

        transition ResolveMemberRevivalSucceededRunning {
            on signal ResolveMemberRevivalSucceeded { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "revival_pending" { self.member_revival_pending.contains(agent_identity) == true }
            update {
                self.member_revival_pending.remove(agent_identity);
            }
            to Running
        }

        transition ResolveMemberRevivalFailedRunning {
            on signal ResolveMemberRevivalFailed { agent_identity, reason }
            guard { self.lifecycle_phase == Phase::Running }
            guard "revival_pending" { self.member_revival_pending.contains(agent_identity) == true }
            update {
                self.member_revival_pending.remove(agent_identity);
                self.member_restore_failures.insert(agent_identity, reason);
                self.member_restore_failure_codes.remove(agent_identity);
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
            // 0.7.2 L5 (row 14): destroy admission opens the pending-spawn
            // drain obligation. The shell aborts + awaits every in-flight
            // spawn-provisioning task, fails its waiters with a typed
            // cancellation, and closes each machine obligation with
            // `CancelPendingSpawn` before the `Destroy` transition (guarded on
            // the drained pending-spawn table) can commit.
            emit RequestPendingSpawnQuiesceForDestroy
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
            // 0.7.2 L5: destroy is the final teardown transition; it may only
            // commit after every kickoff-waiter drain obligation closed (no
            // member kickoff left in flight) and the pending-spawn table was
            // drained via typed `CancelPendingSpawn` closures (instead of the
            // former silent wipe, which erased open obligations).
            guard "kickoff_waiters_quiesced" {
                self.member_kickoff_pending == EmptySet
                && self.member_kickoff_starting == EmptySet
                && self.member_kickoff_callback_pending == EmptySet
            }
            guard "pending_spawns_drained" {
                self.pending_spawn_count == 0
                && self.pending_spawn_sessions == EmptyMap
            }
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
                self.member_prior_peer_endpoints = EmptyMap;
                self.member_startup_binding_requested = EmptySet;
                self.member_startup_runtime_ready = EmptySet;
                self.member_startup_ready = EmptySet;
                self.member_state_markers = EmptyMap;
                self.member_restore_failures = EmptyMap;
                self.member_restore_failure_codes = EmptyMap;
                self.runtime_retire_refusal_codes = EmptyMap;
                self.runtime_retire_refusal_reasons = EmptyMap;
                self.runtime_retire_pending_sessions = EmptyMap;
                self.remote_runtime_retired_ids = EmptySet;
                self.remote_supervisor_revoked_ids = EmptySet;
                self.member_revival_pending = EmptySet;
                // Spawn-exec phase ladder: total teardown — clear the whole phase
                // map so no in-flight spawn-exec survives into Destroyed (every
                // late CommitSpawnActivation/AbortSpawnExec becomes a Settled no-op).
                self.spawn_exec_phase = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
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
            guard "session_ingress_detaches_closed" { self.pending_session_ingress_detach_runtime_ids == EmptySet }
            guard "current_binding_matches" { self.live_runtime_ids.contains(agent_runtime_id) }
            guard "fence_token_matches" { self.runtime_fence_tokens.get_copied(agent_runtime_id) == Some(fence_token) }
            update {
                self.live_runtime_ids = EmptySet;
                self.runtime_fence_tokens = EmptyMap;
                self.member_state_markers = EmptyMap;
                self.member_restore_failures = EmptyMap;
                self.member_restore_failure_codes = EmptyMap;
                self.runtime_retire_refusal_codes = EmptyMap;
                self.runtime_retire_refusal_reasons = EmptyMap;
                self.runtime_retire_pending_sessions = EmptyMap;
                self.remote_runtime_retired_ids = EmptySet;
                self.remote_supervisor_revoked_ids = EmptySet;
                self.member_revival_pending = EmptySet;
                // Spawn-exec phase ladder: total teardown (see DestroyMob).
                self.spawn_exec_phase = EmptyMap;
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

        // Durable wiring events describe historical intent, while terminal
        // member events decide which identity incarnations survived replay.
        // Prune only after the full event stream so a later respawn of the
        // same identity retains its topology and a finally-absent identity
        // cannot resurrect stale trust on the next fresh spawn.
        transition ConvergeRecoveredRosterTopologyPrune {
            per_phase [Running, Stopped, Completed]
            on signal ConvergeRecoveredRosterTopology { edge, a_identity, b_identity }
            guard "edge_recovered" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "edge_has_absent_endpoint" {
                self.identity_to_runtime.contains_key(a_identity) == false
                || self.identity_to_runtime.contains_key(b_identity) == false
            }
            update {
                self.wiring_edges.remove(edge);
                self.topology_epoch += 1;
            }
            to Running
        }

        transition ConvergeRecoveredRosterTopologyRetain {
            per_phase [Running, Stopped, Completed]
            on signal ConvergeRecoveredRosterTopology { edge, a_identity, b_identity }
            guard "edge_recovered" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "edge_endpoints_present" {
                self.identity_to_runtime.contains_key(a_identity) == true
                && self.identity_to_runtime.contains_key(b_identity) == true
            }
            update {}
            to Running
        }

        transition ConvergeRecoveredRosterTopologyDestroyed {
            on signal ConvergeRecoveredRosterTopology { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
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

        transition CleanupRetiringMemberWiring {
            per_phase [Running, Stopped]
            on input CleanupRetiringMemberWiring { edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "retiring_identity_is_endpoint" { agent_identity == a_identity || agent_identity == b_identity }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.wiring_edges.remove(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
        }

        transition CleanupRetiringMemberWiringAlreadyAbsent {
            per_phase [Running, Stopped]
            on input CleanupRetiringMemberWiring { edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation }
            guard "edge_absent" { self.wiring_edges.contains(edge) == false }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "retiring_identity_is_endpoint" { agent_identity == a_identity || agent_identity == b_identity }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {}
            to Running
        }

        transition RestoreRetiringMemberWiring {
            per_phase [Running, Stopped]
            on input RestoreRetiringMemberWiring { edge, a_identity, b_identity, agent_identity, agent_runtime_id, fence_token, generation }
            guard "edge_absent" { self.wiring_edges.contains(edge) == false }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "retiring_identity_is_endpoint" { agent_identity == a_identity || agent_identity == b_identity }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            update {
                self.wiring_edges.insert(edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
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
            guard "member_peer_not_registered" { self.member_peer_ids.contains_key(agent_identity) == false }
            guard "member_endpoint_not_registered" { self.member_peer_endpoints.contains_key(agent_identity) == false }
            guard "fresh_spawn_or_no_retained_topology" {
                self.spawn_exec_phase.get_cloned(agent_identity) == Some(SpawnExecPhase::MembershipCommitted)
                || (
                    for_all(edge in self.wiring_edges,
                        mob_machine_wiring_edge_contains_identity(edge, agent_identity) == false)
                    && mob_machine_member_has_no_external_peer_edges(self.external_peer_edges, agent_identity)
                )
            }
            guard "peer_id_available_for_identity" {
                mob_machine_member_peer_id_available_for_identity(
                    self.member_peer_endpoints,
                    self.member_prior_peer_endpoints,
                    agent_identity,
                    peer_endpoint)
            }
            update {
                self.member_peer_ids.insert(agent_identity, mob_machine_member_peer_endpoint_peer_id(peer_endpoint));
                self.member_peer_endpoints.insert(agent_identity, peer_endpoint);
            }
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        transition RegisterMemberPeerAlreadyCurrentRunning {
            on input RegisterMemberPeer { agent_identity, peer_endpoint }
            guard { self.lifecycle_phase == Phase::Running }
            guard "identity_present" { self.identity_to_runtime.contains_key(agent_identity) == true }
            guard "member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "member_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "current_endpoint_matches" { self.member_peer_endpoints.get_cloned(agent_identity) == Some(peer_endpoint) }
            guard "current_peer_id_matches" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(peer_endpoint)) }
            update {}
            to Running
            emit MemberPeerRegistered { agent_identity: agent_identity, peer_id: mob_machine_member_peer_endpoint_peer_id(peer_endpoint) }
        }

        // A durable endpoint recovery may leave stale reciprocal trust on
        // each still-wired peer. This authorization is deliberately separate
        // from ordinary cleanup: the exact current runtime, exact retained
        // descriptor, live edge, and the other endpoint's current descriptor
        // all come from MobMachine state. The effect carries the historical
        // peer id only on the migrated side and the current peer id on the
        // other side; the existing generated member-unwiring handoff then
        // mints the reciprocal PublicRemove authority without removing the
        // topology edge.
        transition AuthorizeMemberEndpointMigrationTrustCleanupRunning {
            per_phase [Running, Stopped]
            on input AuthorizeMemberEndpointMigrationTrustCleanup { edge, agent_identity, agent_runtime_id, retained_peer_endpoint }
            guard "identity_runtime_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            // Normal resume cleans history while the runtime is live. A
            // retirement retry may already have observed the runtime as gone,
            // but its exact Retiring tuple remains the durable cleanup anchor
            // until MemberRetired clears the generation.
            guard "runtime_live_or_retiring" {
                self.live_runtime_ids.contains(agent_runtime_id) == true
                || self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring)
            }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_contains_migrated_member" { mob_machine_wiring_edge_contains_identity(edge, agent_identity) }
            guard "retained_endpoint_history_present" {
                self.member_prior_peer_endpoints.contains_key(agent_identity) == true
                && self.member_prior_peer_endpoints.get_cloned(agent_identity).get("value").contains(retained_peer_endpoint) == true
            }
            guard "migrated_current_endpoint_registered" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "migrated_current_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            guard "migrated_current_endpoint_changed" { self.member_peer_endpoints.get_cloned(agent_identity) != Some(retained_peer_endpoint) }
            guard "migrated_current_peer_id_changed" {
                mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value"))
                    != mob_machine_member_peer_endpoint_peer_id(retained_peer_endpoint)
            }
            guard "migrated_current_peer_id_matches_endpoint" {
                self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value")))
            }
            guard "edge_a_current_peer_registered" { self.member_peer_ids.contains_key(mob_machine_wiring_edge_a(edge)) == true }
            guard "edge_b_current_peer_registered" { self.member_peer_ids.contains_key(mob_machine_wiring_edge_b(edge)) == true }
            guard "edge_a_current_endpoint_registered" { self.member_peer_endpoints.contains_key(mob_machine_wiring_edge_a(edge)) == true }
            guard "edge_b_current_endpoint_registered" { self.member_peer_endpoints.contains_key(mob_machine_wiring_edge_b(edge)) == true }
            guard "edge_a_current_peer_id_matches_endpoint" {
                self.member_peer_ids.get_cloned(mob_machine_wiring_edge_a(edge)) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(mob_machine_wiring_edge_a(edge)).get("value")))
            }
            guard "edge_b_current_peer_id_matches_endpoint" {
                self.member_peer_ids.get_cloned(mob_machine_wiring_edge_b(edge)) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(mob_machine_wiring_edge_b(edge)).get("value")))
            }
            update {}
            to Running
            emit MemberTrustUnwiringRequested {
                edge: edge,
                a_peer_id: mob_machine_member_endpoint_migration_cleanup_peer_id(mob_machine_wiring_edge_a(edge), agent_identity, retained_peer_endpoint, self.member_peer_ids),
                b_peer_id: mob_machine_member_endpoint_migration_cleanup_peer_id(mob_machine_wiring_edge_b(edge), agent_identity, retained_peer_endpoint, self.member_peer_ids),
                epoch: self.topology_epoch
            }
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

        transition AuthorizeMemberTrustUnwiringStopped {
            on input AuthorizeMemberTrustUnwiring { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            update {}
            to Stopped
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

        transition AuthorizeMemberTrustCleanupStopped {
            on input AuthorizeMemberTrustCleanup { edge, a_identity, b_identity }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "a_member_peer_registered" { self.member_peer_ids.contains_key(a_identity) == true }
            guard "b_member_peer_registered" { self.member_peer_ids.contains_key(b_identity) == true }
            update {}
            to Stopped
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

        transition AuthorizeMemberTrustCleanupObservedStopped {
            on input AuthorizeMemberTrustCleanupObserved { edge, a_identity, a_peer_id, b_identity, b_peer_id }
            guard { self.lifecycle_phase == Phase::Stopped }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "cleanup_has_restore_failure" { self.member_restore_failures.contains_key(a_identity) == true || self.member_restore_failures.contains_key(b_identity) == true }
            update {}
            to Stopped
            emit MemberTrustUnwiringRequested {
                edge: edge,
                a_peer_id: a_peer_id,
                b_peer_id: b_peer_id,
                epoch: self.topology_epoch
            }
        }

        // Cold retirement retry may have only observed trust rows left: the
        // retirement-start journal deliberately clears restore diagnostics,
        // while peer endpoint registration can be unavailable after the
        // crashed runtime vanished. The exact current runtime binding, fence,
        // generation, and Retiring marker are the machine-owned authority for
        // accepting those observed peer ids. This path never authorizes an
        // active or stale runtime to remove trust.
        transition AuthorizeRetiringMemberTrustCleanupObserved {
            per_phase [Running, Stopped]
            on input AuthorizeRetiringMemberTrustCleanupObserved { edge, a_identity, a_peer_id, b_identity, b_peer_id, agent_identity, agent_runtime_id, fence_token, generation }
            guard "edge_matches_members" { mob_machine_wiring_edge_matches_members(edge, a_identity, b_identity) }
            guard "edge_currently_wired" { self.wiring_edges.contains(edge) == true }
            guard "retiring_identity_is_endpoint" { agent_identity == a_identity || agent_identity == b_identity }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
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

        transition CleanupRetiringExternalPeer {
            per_phase [Running, Stopped]
            on input CleanupRetiringExternalPeer { key, edge, agent_identity, agent_runtime_id, fence_token, generation }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_matches_member" { mob_machine_external_peer_key_matches_local(key, agent_identity) }
            guard "external_peer_key_currently_wired" { self.external_peer_edges_by_key.get_cloned(key) == Some(edge) }
            guard "external_peer_edge_currently_wired" { self.external_peer_edges.contains(edge) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "local_member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            update {
                self.external_peer_edges.remove(edge);
                self.external_peer_edges_by_key.remove(key);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit ExternalPeerTrustUnwiringRequested {
                edge: edge,
                local_peer_id: self.member_peer_ids.get_cloned(agent_identity).get("value"),
                peer_id: mob_machine_external_peer_edge_peer_id(edge),
                epoch: self.topology_epoch
            }
        }

        transition RestoreRetiringExternalPeer {
            per_phase [Running, Stopped]
            on input RestoreRetiringExternalPeer { key, edge, agent_identity, agent_runtime_id, fence_token, generation }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_matches_member" { mob_machine_external_peer_key_matches_local(key, agent_identity) }
            guard "external_peer_key_absent" { self.external_peer_edges_by_key.contains_key(key) == false }
            guard "external_peer_edge_absent" { self.external_peer_edges.contains(edge) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "local_member_peer_registered" { self.member_peer_ids.contains_key(agent_identity) == true }
            update {
                self.external_peer_edges.insert(edge);
                self.external_peer_edges_by_key.insert(key, edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
            emit ExternalPeerTrustWiringRequested {
                edge: edge,
                local_peer_id: self.member_peer_ids.get_cloned(agent_identity).get("value"),
                peer_id: mob_machine_external_peer_edge_peer_id(edge),
                epoch: self.topology_epoch
            }
        }

        // A cold retirement retry can prove that the retiring runtime no
        // longer has a live comms owner. In that shape there is no trust store
        // on which to realize an ExternalPeerTrustUnwiringRequested effect.
        // The durable retiring endpoint plus exact runtime/fence/generation
        // authorizes topology convergence without publishing an unrealizable
        // trust obligation. A failed event append uses the symmetric restore
        // transition, which likewise emits no live trust effect.
        transition CleanupRetiringExternalPeerObservedAbsent {
            per_phase [Running, Stopped]
            on input CleanupRetiringExternalPeerObservedAbsent { key, edge, agent_identity, agent_runtime_id, fence_token, generation }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_matches_member" { mob_machine_external_peer_key_matches_local(key, agent_identity) }
            guard "external_peer_key_currently_wired" { self.external_peer_edges_by_key.get_cloned(key) == Some(edge) }
            guard "external_peer_edge_currently_wired" { self.external_peer_edges.contains(edge) == true }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retiring_peer_endpoint_retained" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "retiring_peer_id_retained" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value"))) }
            update {
                self.external_peer_edges.remove(edge);
                self.external_peer_edges_by_key.remove(key);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
        }

        transition RestoreRetiringExternalPeerObservedAbsent {
            per_phase [Running, Stopped]
            on input RestoreRetiringExternalPeerObservedAbsent { key, edge, agent_identity, agent_runtime_id, fence_token, generation }
            guard "external_peer_key_matches_edge" { mob_machine_external_peer_key_matches_edge(key, edge) }
            guard "external_peer_key_matches_member" { mob_machine_external_peer_key_matches_local(key, agent_identity) }
            guard "external_peer_key_absent" { self.external_peer_edges_by_key.contains_key(key) == false }
            guard "external_peer_edge_absent" { self.external_peer_edges.contains(edge) == false }
            guard "identity_binding_matches" { self.identity_to_runtime.get_cloned(agent_identity) == Some(agent_runtime_id) }
            guard "generation_matches" { self.identity_runtime_generations.get_copied(agent_identity) == Some(generation) }
            guard "fence_token_matches" { self.identity_runtime_fence_tokens.get_copied(agent_identity) == Some(fence_token) }
            guard "member_retiring" { self.member_state_markers.get_cloned(agent_runtime_id) == Some(MobMemberState::Retiring) }
            guard "retiring_peer_endpoint_retained" { self.member_peer_endpoints.contains_key(agent_identity) == true }
            guard "retiring_peer_id_retained" { self.member_peer_ids.get_cloned(agent_identity) == Some(mob_machine_member_peer_endpoint_peer_id(self.member_peer_endpoints.get_cloned(agent_identity).get("value"))) }
            update {
                self.external_peer_edges.insert(edge);
                self.external_peer_edges_by_key.insert(key, edge);
                self.topology_epoch += 1;
            }
            to Running
            emit WiringGraphChanged { epoch: self.topology_epoch }
        }


        // Mob-wide supervisor authority. The shell may generate key material,
        // but MobMachine owns whether the current/pending authority tuple is
        // admitted and which exact tuple may be persisted or used by the
        // bridge.

        transition AdmitSupervisorRotation {
            per_phase [Running, Stopped]
            on input AdmitSupervisorRotation
            guard "destroy_not_admitted" { self.destroy_admitted == false }
            guard "no_member_retiring" {
                for_all(agent_runtime_id in self.member_state_markers.keys(),
                    self.member_state_markers.get_cloned(agent_runtime_id) != Some(MobMemberState::Retiring))
            }
            update {}
            to Running
        }

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
                self.supervisor_pending_authority_operation_id = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
                self.supervisor_pending_authority_member_target_names = EmptyMap;
                self.supervisor_pending_authority_member_target_addresses = EmptyMap;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: peer_id,
                signing_key: signing_key,
                epoch: epoch,
                protocol_version: protocol_version,
                pending_operation_id: None,
                pending_peer_id: None,
                pending_signing_key: None,
                pending_epoch: None,
                pending_protocol_version: None,
                pending_accepted_peer_ids: EmptySet,
                pending_member_target_names: EmptyMap,
                pending_member_target_addresses: EmptyMap
            }
        }

        transition RecoverSupervisorAuthority {
            per_phase [Running, Stopped, Completed, Destroyed]
            on signal RecoverSupervisorAuthority {
                peer_id,
                signing_key,
                epoch,
                protocol_version,
                pending_operation_id,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids,
                pending_member_target_names,
                pending_member_target_addresses
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
                    && pending_operation_id == None
                    && pending_accepted_peer_ids == EmptySet
                    && pending_member_target_names == EmptyMap
                    && pending_member_target_addresses == EmptyMap
                )
                || (
                    pending_peer_id != None
                    && pending_signing_key != None
                    && pending_epoch != None
                    && pending_protocol_version != None
                )
            }
            guard "pending_member_targets_aligned" {
                pending_member_target_names.keys() == pending_member_target_addresses.keys()
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
                self.supervisor_pending_authority_operation_id = pending_operation_id;
                self.supervisor_pending_authority_accepted_peer_ids = pending_accepted_peer_ids;
                self.supervisor_pending_authority_member_target_names = pending_member_target_names;
                self.supervisor_pending_authority_member_target_addresses = pending_member_target_addresses;
            }
            to Running
        }

        transition RecordSupervisorPendingRotation {
            per_phase [Running, Stopped, Completed]
            on input RecordSupervisorPendingRotation {
                current_peer_id,
                current_epoch,
                current_protocol_version,
                operation_id,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                accepted_peer_ids,
                active_peer_ids,
                member_target_names,
                member_target_addresses
            }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(current_protocol_version)
            }
            guard "pending_authority_changes_peer" { pending_peer_id != current_peer_id }
            guard "pending_epoch_is_successor" { pending_epoch == current_epoch + 1 }
            guard "operation_id_not_empty" { operation_id != "" }
            guard "member_targets_aligned" {
                member_target_names.keys() == member_target_addresses.keys()
            }
            guard "accepted_peers_have_targets" {
                for_all(peer_id in accepted_peer_ids,
                    member_target_names.contains_key(peer_id)
                    && member_target_addresses.contains_key(peer_id))
            }
            guard "pending_slot_empty_legacy_reconciled_or_exact_retry" {
                (
                    self.supervisor_pending_authority_operation_id == None
                    && self.supervisor_pending_authority_peer_id == None
                    && accepted_peer_ids == EmptySet
                )
                || (
                    self.supervisor_pending_authority_operation_id == None
                    && self.supervisor_pending_authority_peer_id == Some(pending_peer_id)
                    && self.supervisor_pending_authority_signing_key == Some(pending_signing_key)
                    && self.supervisor_pending_authority_epoch == Some(pending_epoch)
                    // Legacy V2/V3 records predate operation ids and exact
                    // targets. The shell supplies the current active-peer set;
                    // the machine admits exactly old-accepted intersect active,
                    // deterministically dropping retired/missing peer ids before
                    // installing the first operation id.
                    && for_all(peer_id in accepted_peer_ids,
                        self.supervisor_pending_authority_accepted_peer_ids.contains(peer_id)
                        && active_peer_ids.contains(peer_id))
                    && for_all(peer_id in self.supervisor_pending_authority_accepted_peer_ids,
                        accepted_peer_ids.contains(peer_id) == active_peer_ids.contains(peer_id))
                )
                || (
                    self.supervisor_pending_authority_operation_id == Some(operation_id)
                    && self.supervisor_pending_authority_peer_id == Some(pending_peer_id)
                    && self.supervisor_pending_authority_signing_key == Some(pending_signing_key)
                    && self.supervisor_pending_authority_epoch == Some(pending_epoch)
                    && self.supervisor_pending_authority_protocol_version == Some(pending_protocol_version)
                    && for_all(peer_id in self.supervisor_pending_authority_accepted_peer_ids,
                        accepted_peer_ids.contains(peer_id))
                    && for_all(peer_id in self.supervisor_pending_authority_member_target_names.keys(),
                        member_target_names.get_cloned(peer_id)
                            == self.supervisor_pending_authority_member_target_names.get_cloned(peer_id))
                    && for_all(peer_id in self.supervisor_pending_authority_member_target_addresses.keys(),
                        member_target_addresses.get_cloned(peer_id)
                            == self.supervisor_pending_authority_member_target_addresses.get_cloned(peer_id))
                )
            }
            update {
                self.supervisor_pending_authority_peer_id = Some(pending_peer_id);
                self.supervisor_pending_authority_signing_key = Some(pending_signing_key);
                self.supervisor_pending_authority_epoch = Some(pending_epoch);
                self.supervisor_pending_authority_protocol_version = Some(pending_protocol_version);
                self.supervisor_pending_authority_operation_id = Some(operation_id);
                self.supervisor_pending_authority_accepted_peer_ids = accepted_peer_ids;
                self.supervisor_pending_authority_member_target_names = member_target_names;
                self.supervisor_pending_authority_member_target_addresses = member_target_addresses;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: current_peer_id,
                signing_key: self.supervisor_authority_signing_key.get("value"),
                epoch: current_epoch,
                protocol_version: current_protocol_version,
                pending_operation_id: Some(operation_id),
                pending_peer_id: Some(pending_peer_id),
                pending_signing_key: Some(pending_signing_key),
                pending_epoch: Some(pending_epoch),
                pending_protocol_version: Some(pending_protocol_version),
                pending_accepted_peer_ids: accepted_peer_ids,
                pending_member_target_names: member_target_names,
                pending_member_target_addresses: member_target_addresses
            }
        }

        // --- Recipient-trust obligation window (dogma row R044) ---
        //
        // The comms router refuses to route to untrusted peers, so the shell
        // must install recipient trust BEFORE the AuthorizeSupervisor /
        // BindMember terminality verdict. That trust-install-to-terminality
        // window is a machine fact, mirroring the pending-rotation metadata
        // pattern: Record before trust install, Resolve on confirmed-accept
        // terminality, Rollback after the shell removes the installed trust
        // on a failure path (send error, terminal rejection, decode error).
        // No guards beyond phase: set insert/remove are idempotent so nested
        // authorize-then-bind windows for the same peer compose. The state is
        // volatile exactly like the trust it tracks (recovery seeds empty).

        transition RecordPendingRecipientTrust {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input RecordPendingRecipientTrust { peer_id }
            update {
                self.pending_recipient_trust.insert(peer_id);
            }
            to Running
        }

        transition ResolvePendingRecipientTrust {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input ResolvePendingRecipientTrust { peer_id }
            update {
                self.pending_recipient_trust.remove(peer_id);
            }
            to Running
        }

        transition RollbackPendingRecipientTrust {
            per_phase [Running, Stopped, Completed, Destroyed]
            on input RollbackPendingRecipientTrust { peer_id }
            update {
                self.pending_recipient_trust.remove(peer_id);
            }
            to Running
        }

        transition CommitSupervisorRotation {
            per_phase [Running, Stopped, Completed]
            on input CommitSupervisorRotation { current_peer_id, current_epoch, current_protocol_version, operation_id, next_peer_id, next_signing_key, next_epoch, next_protocol_version }
            guard "current_supervisor_matches" {
                self.supervisor_authority_peer_id == Some(current_peer_id)
                && self.supervisor_authority_signing_key != None
                && self.supervisor_authority_epoch == Some(current_epoch)
                && self.supervisor_authority_protocol_version == Some(current_protocol_version)
            }
            guard "next_epoch_is_successor" { next_epoch == current_epoch + 1 }
            guard "next_authority_changes_peer" { next_peer_id != current_peer_id }
            guard "pending_operation_matches" {
                self.supervisor_pending_authority_operation_id == Some(operation_id)
            }
            guard "pending_authority_matches_next" {
                self.supervisor_pending_authority_peer_id == Some(next_peer_id)
                && self.supervisor_pending_authority_signing_key == Some(next_signing_key)
                && self.supervisor_pending_authority_epoch == Some(next_epoch)
                && self.supervisor_pending_authority_protocol_version == Some(next_protocol_version)
            }
            update {
                self.supervisor_authority_peer_id = Some(next_peer_id);
                self.supervisor_authority_signing_key = Some(next_signing_key);
                self.supervisor_authority_epoch = Some(next_epoch);
                self.supervisor_authority_protocol_version = Some(next_protocol_version);
                self.supervisor_pending_authority_peer_id = None;
                self.supervisor_pending_authority_signing_key = None;
                self.supervisor_pending_authority_epoch = None;
                self.supervisor_pending_authority_protocol_version = None;
                self.supervisor_pending_authority_operation_id = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
                self.supervisor_pending_authority_member_target_names = EmptyMap;
                self.supervisor_pending_authority_member_target_addresses = EmptyMap;
            }
            to Running
            emit PersistSupervisorAuthority {
                peer_id: next_peer_id,
                signing_key: next_signing_key,
                epoch: next_epoch,
                protocol_version: next_protocol_version,
                pending_operation_id: None,
                pending_peer_id: None,
                pending_signing_key: None,
                pending_epoch: None,
                pending_protocol_version: None,
                pending_accepted_peer_ids: EmptySet,
                pending_member_target_names: EmptyMap,
                pending_member_target_addresses: EmptyMap
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
                self.supervisor_pending_authority_operation_id = None;
                self.supervisor_pending_authority_accepted_peer_ids = EmptySet;
                self.supervisor_pending_authority_member_target_names = EmptyMap;
                self.supervisor_pending_authority_member_target_addresses = EmptyMap;
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
                pending_operation_id,
                pending_peer_id,
                pending_signing_key,
                pending_epoch,
                pending_protocol_version,
                pending_accepted_peer_ids,
                pending_member_target_names,
                pending_member_target_addresses
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
                    && pending_operation_id == None
                    && pending_accepted_peer_ids == EmptySet
                    && pending_member_target_names == EmptyMap
                    && pending_member_target_addresses == EmptyMap
                )
                || (
                    pending_peer_id != None
                    && pending_signing_key != None
                    && pending_epoch != None
                    && pending_protocol_version != None
                )
            }
            guard "pending_member_targets_aligned" {
                pending_member_target_names.keys() == pending_member_target_addresses.keys()
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
                self.supervisor_pending_authority_operation_id = pending_operation_id;
                self.supervisor_pending_authority_accepted_peer_ids = pending_accepted_peer_ids;
                self.supervisor_pending_authority_member_target_names = pending_member_target_names;
                self.supervisor_pending_authority_member_target_addresses = pending_member_target_addresses;
            }
            to Destroyed
            emit PersistSupervisorAuthority {
                peer_id: peer_id,
                signing_key: signing_key,
                epoch: epoch,
                protocol_version: protocol_version,
                pending_operation_id: pending_operation_id,
                pending_peer_id: pending_peer_id,
                pending_signing_key: pending_signing_key,
                pending_epoch: pending_epoch,
                pending_protocol_version: pending_protocol_version,
                pending_accepted_peer_ids: pending_accepted_peer_ids,
                pending_member_target_names: pending_member_target_names,
                pending_member_target_addresses: pending_member_target_addresses
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
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_session_bindings.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedReleasing,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
            // 0.7.2 L5: member retire opens the kickoff-waiter drain
            // obligation; archival is guarded on its closure.
            emit RequestKickoffQuiesce { member_id: agent_identity }
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
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestKickoffQuiesce { member_id: agent_identity }
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
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
            }
            to Running
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPeerOnly,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: None
            }
            emit RequestKickoffQuiesce { member_id: agent_identity }
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
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_session_bindings.remove(agent_identity);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
                self.pending_session_ingress_detach_runtime_ids.insert(agent_runtime_id);
                self.topology_epoch += 1;
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedReleasing,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestSessionIngressDetachForMobDestroy { mob_id: mob_id, agent_runtime_id: agent_runtime_id }
            emit MemberSessionBindingChanged { epoch: self.topology_epoch, agent_identity: agent_identity, old_session_id: Some(releasing.get("value")), new_session_id: None }
            emit RequestKickoffQuiesce { member_id: agent_identity }
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
                self.runtime_retire_pending_sessions.insert(agent_runtime_id, session_id.get("value"));
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: session_id
            }
            emit RequestRuntimeRetire { agent_identity: agent_identity, agent_runtime_id: agent_runtime_id, session_id: session_id.get("value") }
            emit RequestKickoffQuiesce { member_id: agent_identity }
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
                self.runtime_retire_pending_sessions.remove(agent_runtime_id);
                self.member_restore_failures.remove(agent_identity);
                self.member_restore_failure_codes.remove(agent_identity);
                self.member_revival_pending.remove(agent_identity);
            }
            to Stopped
            emit AppendLifecycleJournal {
                kind: MobLifecycleJournalKind::MemberRetirementStartedPeerOnly,
                agent_identity: Some(agent_identity),
                agent_runtime_id: Some(agent_runtime_id),
                fence_token: None,
                generation: Some(generation),
                session_id: None
            }
            emit RequestKickoffQuiesce { member_id: agent_identity }
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

        // 0.7.2 L5 D2a (row 14): a spawn completion that arrives after its
        // pending slot was already closed (teardown drained it, or a destroy
        // retry re-observes a completion) is a legitimate late arrival, not a
        // shell error. Accepted as an explicit typed no-op.
        transition CompleteSpawnLateArrival {
            per_phase [Running, Stopped, Completed]
            on signal CompleteSpawn { agent_identity }
            guard "pending_identity_absent" { self.pending_spawn_sessions.contains_key(agent_identity) == false }
            update {}
            to Running
        }

        transition CompleteSpawnDestroyed {
            on signal CompleteSpawn { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
        }

        // =====================================================================
        // CancelPendingSpawn — 0.7.2 L5 (row 14)
        //
        // Typed teardown closure for an in-flight pending spawn. Fired by the
        // shell while discharging `RequestPendingSpawnQuiesceForDestroy` (and
        // by member-scoped pending-spawn cancellation), after the provisioning
        // waiter task was aborted + awaited and its waiters failed with a
        // typed cancellation. Replaces the former laundering of teardown
        // cancellation through the `CompleteSpawn` success signal (which
        // emitted a `Spawned` lifecycle notice for a spawn that never landed).
        // Total: absent identities and the Destroyed phase are accepted
        // no-ops so re-drains are idempotent.
        // =====================================================================

        transition CancelPendingSpawnPresent {
            per_phase [Running, Stopped, Completed]
            on input CancelPendingSpawn { agent_identity }
            guard "pending_spawns_present" { self.pending_spawn_count > 0 }
            guard "pending_identity_present" { self.pending_spawn_sessions.contains_key(agent_identity) == true }
            update {
                self.pending_spawn_count -= 1;
                self.pending_spawn_sessions.remove(agent_identity);
            }
            to Running
        }

        transition CancelPendingSpawnAbsent {
            per_phase [Running, Stopped, Completed]
            on input CancelPendingSpawn { agent_identity }
            guard "pending_identity_absent" { self.pending_spawn_sessions.contains_key(agent_identity) == false }
            update {}
            to Running
        }

        transition CancelPendingSpawnDestroyed {
            on input CancelPendingSpawn { agent_identity }
            guard { self.lifecycle_phase == Phase::Destroyed }
            update {}
            to Destroyed
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
            // 0.7.2 L5: same teardown-completion gating as `DestroyMob` — no
            // in-flight kickoff waiters, pending-spawn obligations closed via
            // typed `CancelPendingSpawn` instead of a silent wipe.
            guard "kickoff_waiters_quiesced" {
                self.member_kickoff_pending == EmptySet
                && self.member_kickoff_starting == EmptySet
                && self.member_kickoff_callback_pending == EmptySet
            }
            guard "pending_spawns_drained" {
                self.pending_spawn_count == 0
                && self.pending_spawn_sessions == EmptyMap
            }
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
                self.member_prior_peer_endpoints = EmptyMap;
                self.runtime_retire_refusal_codes = EmptyMap;
                self.runtime_retire_refusal_reasons = EmptyMap;
                self.runtime_retire_pending_sessions = EmptyMap;
                self.pending_session_ingress_detach_runtime_ids = EmptySet;
                self.active_run_count = 0;
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

        fn mob_machine_member_peer_id_available_for_identity(
            current_endpoints: &std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,
            prior_endpoints: &std::collections::BTreeMap<AgentIdentity, std::collections::BTreeSet<MemberPeerEndpoint>>,
            agent_identity: &AgentIdentity,
            candidate_endpoint: &MemberPeerEndpoint,
        ) -> bool {
            current_endpoints.iter().all(|(other_identity, endpoint)| {
                other_identity == agent_identity
                    || endpoint.peer_id != candidate_endpoint.peer_id
            }) && prior_endpoints.iter().all(|(other_identity, endpoints)| {
                other_identity == agent_identity
                    || endpoints
                        .iter()
                        .all(|endpoint| endpoint.peer_id != candidate_endpoint.peer_id)
            })
        }

        fn mob_machine_member_has_no_external_peer_edges(
            external_peer_edges: &std::collections::BTreeSet<ExternalPeerEdge>,
            agent_identity: &AgentIdentity,
        ) -> bool {
            external_peer_edges
                .iter()
                .all(|edge| edge.local != *agent_identity)
        }

        fn mob_machine_member_prior_peer_endpoints_after_migration(
            all_prior_endpoints: &std::collections::BTreeMap<AgentIdentity, std::collections::BTreeSet<MemberPeerEndpoint>>,
            agent_identity: &AgentIdentity,
            current_endpoint: &MemberPeerEndpoint,
            next_endpoint: &MemberPeerEndpoint,
        ) -> std::collections::BTreeMap<AgentIdentity, std::collections::BTreeSet<MemberPeerEndpoint>> {
            let mut all_prior_endpoints = all_prior_endpoints.clone();
            {
                let prior_endpoints = all_prior_endpoints
                    .entry(agent_identity.clone())
                    .or_default();
                prior_endpoints.insert(current_endpoint.clone());
                // Rotating back to a previously used endpoint makes it
                // current again. Trust stores are keyed by PeerId, so remove
                // every descriptor for the next PeerId rather than only an
                // exactly equal descriptor; cleanup must never remove the live
                // trust row through an older metadata shape of the same key.
                prior_endpoints.retain(|endpoint| endpoint.peer_id != next_endpoint.peer_id);
            }
            if all_prior_endpoints
                .get(agent_identity)
                .is_some_and(std::collections::BTreeSet::is_empty)
            {
                all_prior_endpoints.remove(agent_identity);
            }
            all_prior_endpoints
        }

        fn mob_machine_wiring_edge_a(edge: &WiringEdge) -> AgentIdentity {
            edge.a.clone()
        }

        fn mob_machine_wiring_edge_b(edge: &WiringEdge) -> AgentIdentity {
            edge.b.clone()
        }

        fn mob_machine_wiring_edge_contains_identity(
            edge: &WiringEdge,
            agent_identity: &AgentIdentity,
        ) -> bool {
            edge.a == *agent_identity || edge.b == *agent_identity
        }

        fn mob_machine_member_endpoint_migration_cleanup_peer_id(
            endpoint_identity: &AgentIdentity,
            migrated_identity: &AgentIdentity,
            retained_peer_endpoint: &MemberPeerEndpoint,
            current_peer_ids: &std::collections::BTreeMap<AgentIdentity, PeerId>,
        ) -> PeerId {
            if endpoint_identity == migrated_identity {
                retained_peer_endpoint.peer_id.clone()
            } else {
                current_peer_ids
                    .get(endpoint_identity)
                    .expect("migration cleanup guards require the other member's current peer id")
                    .clone()
            }
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

        // -----------------------------------------------------------------
        // Wave-G1 catalog fold helpers.
        // -----------------------------------------------------------------

        // Row #181: compute the next monotone respawn generation for an
        // identity. Reads the machine-owned current Generation and returns
        // current.saturating_add(1), starting at 1 when the identity has no
        // recorded generation yet.
        fn mob_machine_next_respawn_generation(
            identity_runtime_generations: &std::collections::BTreeMap<AgentIdentity, Generation>,
            agent_identity: &AgentIdentity,
        ) -> Generation {
            let current = identity_runtime_generations
                .get(agent_identity)
                .map(|generation| generation.0)
                .unwrap_or(0);
            Generation(current.saturating_add(1))
        }


        // Row #351: members that must be spawned — desired identities that are
        // not currently bound to a runtime.
        fn mob_machine_members_to_spawn(
            identity_to_runtime: &std::collections::BTreeMap<AgentIdentity, AgentRuntimeId>,
            desired: &std::collections::BTreeSet<AgentIdentity>,
        ) -> std::collections::BTreeSet<AgentIdentity> {
            desired
                .iter()
                .filter(|identity| !identity_to_runtime.contains_key(*identity))
                .cloned()
                .collect()
        }

        // Row #351: members that must be retired — currently bound identities
        // that are no longer desired. Only computed when `retire_stale` is set;
        // otherwise no member is retired (the set is empty).
        fn mob_machine_members_to_retire(
            identity_to_runtime: &std::collections::BTreeMap<AgentIdentity, AgentRuntimeId>,
            desired: &std::collections::BTreeSet<AgentIdentity>,
            retire_stale: &bool,
        ) -> std::collections::BTreeSet<AgentIdentity> {
            if !*retire_stale {
                return std::collections::BTreeSet::new();
            }
            identity_to_runtime
                .keys()
                .filter(|identity| !desired.contains(*identity))
                .cloned()
                .collect()
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

/// Bridging type for adaptive run identity.
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
pub struct AdaptiveRunId(pub String);

impl<T: Into<String>> From<T> for AdaptiveRunId {
    fn from(s: T) -> Self {
        Self(s.into())
    }
}

/// Bridging type for adaptive layer identity.
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
pub struct AdaptiveLayerId(pub String);

impl<T: Into<String>> From<T> for AdaptiveLayerId {
    fn from(s: T) -> Self {
        Self(s.into())
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

/// Per-identity spawn-execution phase ladder. Absence of an entry in
/// `spawn_exec_phase` == Settled (no spawn-exec in flight for that identity),
/// which makes every late/post-teardown CommitSpawnActivation/AbortSpawnExec
/// input a total no-op. `Opened`: `BeginSpawnExec` committed, membership not yet
/// committed. `MembershipCommitted`: `CommitSpawnMembership` committed the
/// membership facts atomically. `Activated`: terminal rung — reserved for an
/// explicit "activated" marker; the current ladder removes the entry directly at
/// `CommitSpawnActivation` (Settled) rather than parking in `Activated`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SpawnExecPhase {
    Opened,
    MembershipCommitted,
    Activated,
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
pub enum AdaptiveRunPhase {
    #[default]
    Active,
    CleanupRequired,
    EvidenceMissing,
    Finished,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveStopReason {
    #[default]
    FinishDecision,
    DepthLimit,
    PlanLimit,
    RepairLimit,
    FailureLimit,
    BudgetExhausted,
    DeadlineExceeded,
    HostCancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveDecisionKind {
    #[default]
    RunLayer,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveLayerPhase {
    #[default]
    Validating,
    Admitted,
    Provisioning,
    Running,
    Collecting,
    Completed,
    SetupFailed,
    RunFailed,
    ResultInvalid,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveLayerAdmissionKind {
    #[default]
    Denied,
    Allowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveLayerSetupFaultKind {
    #[default]
    MobCreateFailed,
    SpawnFailed,
    WiringFailed,
    CanceledDuringSetup,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum AdaptiveLayerDispositionKind {
    #[default]
    Destroyed,
    Retained,
    RetainedAsEvidence,
}

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

/// Machine-owned execution-health class for one member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberHealthClass {
    Healthy,
    Degraded,
    Wedged,
    Unknown,
}

/// Typed description of the last progress observation committed by MobMachine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemberProgressEventKind {
    ExecutionAdvanced,
    BecameIdle,
    Unchanged,
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

/// Typed shell observation of a member's live materialization at the dispatch
/// boundary: the member's current bridge session has no live runtime, and the
/// durable session snapshot is either still present (revivable) or gone
/// (terminal). The shell observes; MobMachine owns the verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberLiveMaterializationObservationKind {
    #[default]
    DurableSnapshotPresent,
    DurableSnapshotMissing,
}

/// Machine-owned verdict for a member live-materialization observation:
/// authorize exactly one shell revival attempt, or record the terminal Broken
/// classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberRevivalVerdictKind {
    #[default]
    ReviveAuthorized,
    BrokenRecorded,
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
    MemberRetirementStartedReleasing,
    MemberRetirementStartedPreservingBinding,
    MemberRetirementStartedPeerOnly,
    RemoteMemberRuntimeRetired,
    RemoteMemberSupervisorRevoked,
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

// ---------------------------------------------------------------------------
// Wave-G1 catalog fold enums. All are unit-variant closed mirrors carried as
// the typed verdict/disposition/capability fields on the G1 inputs/effects.
// ---------------------------------------------------------------------------

/// Row #14: machine-owned duplicate-member admission verdict for
/// [`MobMachineInput::ProbeMemberAdmission`]. The actor mirrors `Admitted ->
/// Ok` / `DuplicateRejected -> MemberAlreadyExists` instead of probing the
/// PendingSpawnLineage side-map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MemberAdmissionVerdictKind {
    #[default]
    Admitted,
    DuplicateRejected,
}

/// Row #260: typed step-output fault carried by
/// [`MobMachineInput::ClassifyStepOutputFault`] /
/// [`MobMachineEffect::StepOutputFaultClassified`]. Fault detail strings stay
/// shell-side; this enum is the guard-visible, terminalizable cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StepOutputFaultKind {
    #[default]
    MalformedJson,
}

/// Row #260: machine-owned step-fault disposition. The flow shell mirrors
/// `Retry` (attempt += 1, continue) / `Terminal` (fail).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum StepFaultDispositionKind {
    #[default]
    Retry,
    Terminal,
}

/// Row #261: typed supervisor-escalation failure cause for
/// [`MobMachineEffect::SupervisorEscalationFailed`]. Fail-closed is a typed
/// terminal, not a free-form `SupervisorEscalation` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SupervisorEscalationFailureCause {
    #[default]
    NoEligibleSupervisor,
    TimeoutExceeded,
}

/// Row #293: topology-edge policy decision. Carried by
/// [`MobMachineInput::EvaluateTopologyEdge`] (as `Option<PolicyDecision>` rule
/// match), the declared `topology_default_policy` state field, and the
/// [`MobMachineEffect::TopologyEdgeVerdictResolved`] verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum PolicyDecision {
    #[default]
    Allow,
    Deny,
}

/// Row #314: external-member rebind capability. Minted by the bridge/spawn
/// chokepoints into `external_member_rebind_capability`; read by
/// `project_external_member_observation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum ExternalMemberRebindCapability {
    #[default]
    Unavailable,
    Available,
}

/// Row #320: machine-owned turn-timeout disposition for
/// [`MobMachineEffect::TurnTimeoutDispositionClassified`]. The shell mirrors
/// `Detached` (reconcile detached turn) / `Canceled` (abort) / `Retryable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum TurnTimeoutDisposition {
    #[default]
    Detached,
    Canceled,
    Retryable,
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
