// Generated mob wire types for @rkat/web
// Source: artifacts/schemas/wire-types.json

export const MOB_SPAWN_MANY_FAILURE_CAUSES = [
  "profile_not_found",
  "member_not_found",
  "member_already_exists",
  "not_externally_addressable",
  "invalid_transition",
  "wiring_error",
  "bridge_command_rejected",
  "member_restore_failed",
  "kickoff_wait_timed_out",
  "ready_wait_timed_out",
  "definition_error",
  "flow_not_found",
  "flow_failed",
  "run_not_found",
  "run_canceled",
  "flow_turn_timed_out",
  "frame_depth_limit_exceeded",
  "frame_atomic_persistence_unavailable",
  "spec_revision_conflict",
  "schema_validation",
  "insufficient_targets",
  "topology_violation",
  "bridge_delivery_rejected",
  "supervisor_escalation",
  "unsupported_for_mode",
  "missing_member_capability",
  "reset_barrier",
  "storage_error",
  "session_error",
  "comms_error",
  "callback_pending",
  "stale_fence_token",
  "stale_event_cursor",
  "work_not_found",
  "internal",
] as const;
export type MobSpawnManyFailureCause = typeof MOB_SPAWN_MANY_FAILURE_CAUSES[number];

export type WireMobMemberStatus = "active" | "retiring" | "broken" | "completed" | "unknown";

export type WireMemberRef = string;

export type WireMemberProgressEvent = "execution_advanced" | "became_idle" | "unchanged";

export type WireMemberRunState = "idle" | "run_open" | "unknown";

export type WireMemberHealthClass = "healthy" | "degraded" | "wedged" | "unknown";

export interface WireMemberProgressSnapshot {
  health: WireMemberHealthClass;
  in_flight_work: number;
  last_progress_at_ms: number;
  last_progress_event: WireMemberProgressEvent;
  run_state: WireMemberRunState;
}

export type WireHostRef = string;

export type WireReachability = "reachable" | "stale" | "unreachable" | "unknown";

export interface WireUnreachablePeer {
  peer: string;
  reason?: string | null;
}

export interface WirePeerConnectivitySnapshot {
  reachable_peer_count: number;
  unknown_peer_count: number;
  unreachable_peers?: WireUnreachablePeer[];
}

export interface WirePeerConnectivityNotApplicable {
  status: "not_applicable";
}

export interface WirePeerConnectivityProbeTimedOut {
  status: "probe_timed_out";
}

export interface WirePeerConnectivityKnown {
  snapshot: WirePeerConnectivitySnapshot;
  status: "known";
}

export type WirePeerConnectivity = WirePeerConnectivityNotApplicable | WirePeerConnectivityProbeTimedOut | WirePeerConnectivityKnown;

export type WireNonPortableResourceKind = "rust_bundles" | "per_spawn_external_tools" | "mob_default_external_tools" | "default_llm_client_override" | "host_surface_mcp_allowlist" | "workgraph_tools";

export interface WireMemberLifecycleCapabilities {
  resume_after_restart: boolean;
  revisions: boolean;
  transcript_edits: boolean;
}

export interface MobStatusResult {
  mob_id: string;
  status: "Creating" | "Running" | "Stopped" | "Completed" | "Destroyed";
}

export interface MobListResult {
  mobs: MobStatusResult[];
}

export interface MobRespawnResult {
  failed_peer_ids?: string[];
  receipt: Record<string, unknown>;
  status: "completed" | "topology_restore_failed";
}

export interface MobEventsResult {
  events: unknown[];
}

export type WireHandlingMode = "queue" | "steer";

export interface MobMemberSendResult {
  agent_identity: string;
  handling_mode: WireHandlingMode;
  member_ref: WireMemberRef;
  mob_id: string;
}

export type WireWorkExecutionLifecyclePhase = "absent" | "launch_requested" | "launch_uncertain" | "launch_quarantined" | "running" | "evidence_projection_requested" | "failure_evidence_projection_requested" | "cancellation_evidence_projection_requested" | "launch_failure_evidence_projection_requested" | "work_closure_requested" | "flow_failed" | "flow_canceled" | "evidence_projected" | "work_closed" | "launch_failed";

export interface WireWorkGraphFlowWorkRef {
  item_id: string;
  namespace: string;
  realm_id: string;
}

export interface WireWorkGraphFlowExecutionBinding {
  binding_id: string;
  binding_revision: number;
  created_at: string;
  evidence_id: string;
  flow_config_digest: string;
  flow_id: string;
  lifecycle_phase: WireWorkExecutionLifecyclePhase;
  mob_id: string;
  run_id: string;
  supersedes?: string | null;
  work_ref: WireWorkGraphFlowWorkRef;
}

export interface MobFlowStatusResult {
  execution_binding?: WireWorkGraphFlowExecutionBinding | null;
  run?: Record<string, unknown> | null;
}

export interface MobRunResult {
  run?: Record<string, unknown> | null;
}

export type BindingId = string;

export type ProfileId = string;

export type RealmId = string;

export interface WireAuthBindingRef {
  binding: BindingId;
  profile?: ProfileId | null;
  realm: RealmId;
}

export type WireMobBackendKind = "session" | "external";

export type WireMobRuntimeMode = "autonomous_host" | "turn_driven";

export interface MobSpawnHelperParams {
  agent_identity?: string | null;
  auth_binding?: WireAuthBindingRef | null;
  backend?: WireMobBackendKind | null;
  max_text_bytes: number;
  mob_id: string;
  model_override?: string | null;
  prompt: string;
  result_label: string;
  role_name?: string | null;
  runtime_mode?: WireMobRuntimeMode | null;
}

export interface MobForkHelperParams {
  agent_identity?: string | null;
  auth_binding?: WireAuthBindingRef | null;
  backend?: WireMobBackendKind | null;
  fork_context?: unknown;
  max_text_bytes: number;
  mob_id: string;
  model_override?: string | null;
  prompt: string;
  result_label: string;
  role_name?: string | null;
  runtime_mode?: WireMobRuntimeMode | null;
  source_member_id: string;
}

export type MobBoundedHelperResultStatus = "completed" | "completed_truncated" | "failed" | "failed_truncated" | "in_progress" | "in_progress_truncated" | "unavailable" | "unavailable_truncated";

export interface MobBoundedHelperResult {
  label: string;
  status: MobBoundedHelperResultStatus;
  text: string;
}

export interface Usage {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  provider_accounting?: Record<string, unknown> | null;
}

export interface MobHelperResult {
  agent_identity: string;
  bounded_result: MobBoundedHelperResult;
  member_ref: WireMemberRef;
  output: string;
  retirement_error?: string | null;
  session_id: string;
  tokens_used: number;
  tool_calls: number;
  turns: number;
  usage: Usage;
}

export interface WireResolvedModelCapabilities {
  image_generation?: boolean;
  image_input?: boolean;
  image_tool_results?: boolean;
  inline_video?: boolean;
  mid_conversation_system_messages?: boolean;
  realtime?: boolean;
  vision?: boolean;
  web_search?: boolean;
}

export interface MobMemberStatusResult {
  activity?: Record<string, unknown> | null;
  comms_reachability?: WireReachability | null;
  control_reachability?: WireReachability | null;
  current_session_id?: string | null;
  detached_jobs?: Record<string, unknown> | null;
  error?: string | null;
  external_member?: unknown;
  freshness_reason?: string | null;
  is_final: boolean;
  kickoff?: unknown;
  last_seen_ms?: number | null;
  lifecycle_capabilities?: WireMemberLifecycleCapabilities | null;
  member_ref: WireMemberRef;
  non_portable_disabled?: WireNonPortableResourceKind[] | null;
  output_preview?: string | null;
  peer_connectivity?: WirePeerConnectivity | null;
  placement?: WireHostRef | null;
  progress?: WireMemberProgressSnapshot | null;
  resolved_capabilities?: WireResolvedModelCapabilities | null;
  status: WireMobMemberStatus;
  tokens_used: number;
}

export interface MobAppendSystemContextResult {
  agent_identity: string;
  mob_id: string;
  status: "applied" | "duplicate";
}

export interface MobLifecycleResult {
  action: "stop" | "resume" | "complete" | "reset" | "destroy";
  destroy_report?: unknown;
  mob_id: string;
  ok: boolean;
}

export interface ApplicationToolPolicyBindingUnmanaged {
  kind: "unmanaged";
}

export interface ApplicationToolPolicyBindingInherit {
  kind: "inherit";
}

export interface ApplicationToolPolicyBindingProvider {
  kind: "provider";
  policy_id: string;
  provider_id: string;
}

export type ApplicationToolPolicyBinding = ApplicationToolPolicyBindingUnmanaged | ApplicationToolPolicyBindingInherit | ApplicationToolPolicyBindingProvider;

export type ToolCategoryOverride = "inherit" | "enable" | "disable";

export interface ToolCategoryOverrides {
  builtins?: ToolCategoryOverride;
  comms?: ToolCategoryOverride;
  image_generation?: ToolCategoryOverride;
  memory?: ToolCategoryOverride;
  mob?: ToolCategoryOverride;
  schedule?: ToolCategoryOverride;
  shell?: ToolCategoryOverride;
  web_search?: ToolCategoryOverride;
  workgraph?: ToolCategoryOverride;
}

export interface WireDesiredLocalCallbackTool {
  description: string;
  input_schema: unknown;
  name: string;
}

export interface WireCallbackToolSetDeclarationInherit {
  kind: "inherit";
}

export interface WireCallbackToolSetDeclarationSet {
  kind: "set";
  tools: WireDesiredLocalCallbackTool[];
}

export type WireCallbackToolSetDeclaration = WireCallbackToolSetDeclarationInherit | WireCallbackToolSetDeclarationSet;

export interface WireMemberToolAccessConstraintAllowNames {
  kind: "allow_names";
  names: string[];
}

export interface WireMemberToolAccessConstraintDenyNames {
  kind: "deny_names";
  names: string[];
}

export interface WireMemberToolAccessConstraintReadOnly {
  kind: "read_only";
}

export type WireMemberToolAccessConstraint = WireMemberToolAccessConstraintAllowNames | WireMemberToolAccessConstraintDenyNames | WireMemberToolAccessConstraintReadOnly;

export interface WireMemberToolAccessDeclarationInherit {
  kind: "inherit";
}

export interface WireMemberToolAccessDeclarationUnrestricted {
  kind: "unrestricted";
}

export interface WireMemberToolAccessDeclarationConstraints {
  constraints: WireMemberToolAccessConstraint[];
  kind: "constraints";
}

export type WireMemberToolAccessDeclaration = WireMemberToolAccessDeclarationInherit | WireMemberToolAccessDeclarationUnrestricted | WireMemberToolAccessDeclarationConstraints;

export interface WireMemberToolDeclaration {
  application_policy: ApplicationToolPolicyBinding;
  callback_tools: WireCallbackToolSetDeclaration;
  category_overrides: ToolCategoryOverrides;
  execution: WireMemberToolAccessDeclaration;
}

export type WireIdentityAdoptionPrecondition = "expected_absent";

export type WireDesiredSessionAuthorityPolicy = "require_existing";

export interface WireDesiredSessionTarget {
  authority_policy: WireDesiredSessionAuthorityPolicy;
  lineage_generation: number;
  lineage_id: string;
  session_id: string;
}

export interface WireTrustedPeerIdentityEd25519PublicKey {
  kind: "ed25519_public_key";
  public_key: string;
}

export type WireTrustedPeerIdentity = WireTrustedPeerIdentityEd25519PublicKey;

export interface WireDesiredExecutionControllingSession {
  execution: "controlling_session";
}

export interface WireDesiredExecutionAnyBoundHostSession {
  execution: "any_bound_host_session";
}

export interface WireDesiredExecutionPlacedSession {
  execution: "placed_session";
  host_id: string;
}

export interface WireDesiredExecutionExternal {
  address: string;
  execution: "external";
  identity: WireTrustedPeerIdentity;
}

export type WireDesiredExecution = WireDesiredExecutionControllingSession | WireDesiredExecutionAnyBoundHostSession | WireDesiredExecutionPlacedSession | WireDesiredExecutionExternal;

export interface WireIdentityProfileMemberDeclaration {
  profile_name: string;
  profile_override?: Record<string, unknown> | null;
  model_override?: string | null;
  external_addressable_override?: boolean | null;
  context?: string | null;
  labels?: Record<string, string> | null;
  additional_instructions?: string[] | null;
  system_prompt_override?: Record<string, unknown> | null;
  tool_access_policy?: Record<string, unknown> | null;
  auth_binding?: WireAuthBindingRef | null;
  budget_limits?: Record<string, unknown> | null;
  runtime_mode?: WireMobRuntimeMode | null;
  required_env_keys?: string[];
  required_local_callback_tools?: WireDesiredLocalCallbackTool[];
  execution: WireDesiredExecution;
}

export interface WireDesiredIdentityEdge {
  a: string;
  b: string;
}

export interface WireIdentityAdoptionOutcomeAdopted {
  desired_revision: number;
  outcome: "adopted";
}

export interface WireIdentityAdoptionOutcomePreconditionConflict {
  actual_revision: number;
  outcome: "precondition_conflict";
}

export interface WireIdentityAdoptionOutcomeRequestConflict {
  outcome: "request_conflict";
  request_id: string;
}

export type WireIdentityAdoptionOutcome = WireIdentityAdoptionOutcomeAdopted | WireIdentityAdoptionOutcomePreconditionConflict | WireIdentityAdoptionOutcomeRequestConflict;

export interface WireIdentityConvergenceModeDrain {
  kind: "drain";
  max_wait_ms: number;
}

export interface WireIdentityConvergenceModeCancelActive {
  kind: "cancel_active";
}

export type WireIdentityConvergenceMode = WireIdentityConvergenceModeDrain | WireIdentityConvergenceModeCancelActive;

export type WireIdentityConvergenceCondition = "pending" | "reconciling" | "converged" | "backoff" | "repair_blocked" | "quarantined" | "tombstoned" | "suspended" | "drain_blocked";

export type WireIdentityReconcileDecision = "backoff" | "repair_blocked" | "acquire_lease" | "await_lease" | "close_member_admission" | "await_member_drain" | "drain_blocked" | "cancel_active_member" | "seal_retirement_proven" | "seal_session_creation_consumed" | "ensure_session_authority" | "ensure_runtime_registration" | "await_external_binding_ceremony" | "ensure_external_binding_receipt" | "ensure_external_binding" | "ensure_member_materialization" | "ensure_initial_delivery_receipt" | "ensure_initial_delivery" | "await_initial_delivery" | "reconcile_wiring" | "retire_member_materialization" | "retire_runtime_registration" | "release_session_authority" | "converged" | "tombstoned" | "quarantined";

export interface WireIdentityConvergenceStatus {
  active_intent_revision?: number | null;
  agent_identity: string;
  condition: WireIdentityConvergenceCondition;
  decision?: WireIdentityReconcileDecision | null;
  desired_intent_revision?: number | null;
  detail?: string | null;
  observed_at_ms: number;
}

export interface WireMemberToolCommitOutcomeCommitted {
  desired_revision: number;
  outcome: "committed";
}

export interface WireMemberToolCommitOutcomeNoChange {
  desired_revision: number;
  outcome: "no_change";
}

export interface WireMemberToolCommitOutcomeRevisionConflict {
  actual: number;
  expected: number;
  outcome: "revision_conflict";
}

export interface WireMemberToolCommitOutcomeRequestConflict {
  outcome: "request_conflict";
  request_id: string;
}

export interface WireMemberToolCommitOutcomeMemberAbsent {
  outcome: "member_absent";
}

export interface WireMemberToolCommitOutcomeInvalidDeclaration {
  outcome: "invalid_declaration";
  reason: string;
}

export type WireMemberToolCommitOutcome = WireMemberToolCommitOutcomeCommitted | WireMemberToolCommitOutcomeNoChange | WireMemberToolCommitOutcomeRevisionConflict | WireMemberToolCommitOutcomeRequestConflict | WireMemberToolCommitOutcomeMemberAbsent | WireMemberToolCommitOutcomeInvalidDeclaration;

export interface WireIdentityConvergenceResolutionOutcomeResolved {
  active_revision: number;
  desired_revision: number;
  outcome: "resolved";
}

export interface WireIdentityConvergenceResolutionOutcomeDesiredRevisionConflict {
  actual: number;
  expected: number;
  outcome: "desired_revision_conflict";
}

export interface WireIdentityConvergenceResolutionOutcomeActiveRevisionConflict {
  actual: number;
  expected: number;
  outcome: "active_revision_conflict";
}

export interface WireIdentityConvergenceResolutionOutcomeNotBlocked {
  outcome: "not_blocked";
}

export interface WireIdentityConvergenceResolutionOutcomeMemberAbsent {
  outcome: "member_absent";
}

export interface WireIdentityConvergenceResolutionOutcomeRequestConflict {
  outcome: "request_conflict";
  request_id: string;
}

export type WireIdentityConvergenceResolutionOutcome = WireIdentityConvergenceResolutionOutcomeResolved | WireIdentityConvergenceResolutionOutcomeDesiredRevisionConflict | WireIdentityConvergenceResolutionOutcomeActiveRevisionConflict | WireIdentityConvergenceResolutionOutcomeNotBlocked | WireIdentityConvergenceResolutionOutcomeMemberAbsent | WireIdentityConvergenceResolutionOutcomeRequestConflict;

export interface MobMemberToolDeclarationParams {
  agent_identity: string;
  mob_id: string;
}

export interface MobMemberToolDeclarationResult {
  agent_identity: string;
  convergence: WireIdentityConvergenceStatus;
  declaration: WireMemberToolDeclaration;
  desired_intent_revision: number;
  mob_id: string;
}

export interface MobApplyMemberToolDeclarationParams {
  agent_identity: string;
  convergence: WireIdentityConvergenceMode;
  declaration: WireMemberToolDeclaration;
  expected_intent_revision: number;
  mob_id: string;
  request_id: string;
}

export interface MobApplyMemberToolDeclarationResult {
  commit: WireMemberToolCommitOutcome;
  convergence: WireIdentityConvergenceStatus;
}

export interface MobAdoptMemberIdentityDeclarationParams {
  agent_identity: string;
  convergence: WireIdentityConvergenceMode;
  declaration_revision: number;
  declaration_scope: string;
  member: WireIdentityProfileMemberDeclaration;
  mob_id: string;
  owned_wiring: WireDesiredIdentityEdge[];
  precondition: WireIdentityAdoptionPrecondition;
  request_id: string;
  session: WireDesiredSessionTarget;
  wiring_custody?: "external_managed" | "identity_owned";
}

export interface MobAdoptMemberIdentityDeclarationResult {
  adoption: WireIdentityAdoptionOutcome;
  convergence: WireIdentityConvergenceStatus;
}

export interface MobResolveIdentityConvergenceBlockParams {
  agent_identity: string;
  convergence: WireIdentityConvergenceMode;
  expected_desired_revision: number;
  mob_id: string;
  observed_active_revision: number;
  request_id: string;
}

export interface MobResolveIdentityConvergenceBlockResult {
  convergence: WireIdentityConvergenceStatus;
  outcome: WireIdentityConvergenceResolutionOutcome;
}
