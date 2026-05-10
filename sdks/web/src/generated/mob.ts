// Generated mob wire types for @rkat/web
// Source: artifacts/schemas/wire-types.json

export type WireMobMemberStatus = "active" | "retiring" | "broken" | "completed" | "unknown";

export type WireMemberRef = string;

export interface MobStatusResult {
  mob_id: string;
  status: string;
}

export interface MobListResult {
  mobs: MobStatusResult[];
}

export interface MobRespawnResult {
  failed_peer_ids?: string[];
  receipt: Record<string, unknown>;
  status: string;
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
  run: unknown;
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
  output_preview?: string;
  peer_connectivity?: unknown;
  resolved_capabilities?: WireResolvedModelCapabilities;
  status: WireMobMemberStatus;
  tokens_used: number;
}

export interface MobAppendSystemContextResult {
  agent_identity: string;
  mob_id: string;
  status: string;
}
