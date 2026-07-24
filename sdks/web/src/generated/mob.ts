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

export interface MobFlowStatusResult {
  run?: Record<string, unknown> | null;
}

export interface MobRunResult {
  run?: Record<string, unknown> | null;
}

export interface MobHelperResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  output?: string | null;
  tokens_used: number;
}

export interface WireResolvedModelCapabilities {
  image_generation?: boolean;
  image_input?: boolean;
  image_tool_results?: boolean;
  inline_video?: boolean;
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
  status: "applied" | "staged" | "duplicate";
}

export interface MobLifecycleResult {
  action: "stop" | "resume" | "complete" | "reset" | "destroy";
  destroy_report?: unknown;
  mob_id: string;
  ok: boolean;
}
