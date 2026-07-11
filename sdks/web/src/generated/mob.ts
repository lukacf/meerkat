// Generated mob wire types for @rkat/web
// Source: artifacts/schemas/wire-types.json

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

export interface MobMemberSendResult {
  agent_identity: string;
  handling_mode: "queue" | "steer";
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobFlowStatusResult {
  run?: Record<string, unknown>;
}

export interface MobRunResult {
  run?: Record<string, unknown>;
}

export interface MobHelperResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  output?: string;
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
  current_session_id?: string;
  error?: string;
  external_member?: unknown;
  is_final: boolean;
  kickoff?: unknown;
  member_ref: WireMemberRef;
  output_preview?: string;
  peer_connectivity?: Record<string, unknown>;
  progress?: WireMemberProgressSnapshot;
  resolved_capabilities?: WireResolvedModelCapabilities;
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
