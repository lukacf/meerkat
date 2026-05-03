// Generated mob wire types for @rkat/web
// Source: artifacts/schemas/wire-types.json

export type WireMobMemberStatus = "active" | "retiring" | "broken" | "completed" | "unknown";

export interface MobStatusResult {
  mob_id: string;
  status: string;
}

export interface MobRespawnResult {
  failed_peer_ids?: string[];
  receipt: Record<string, unknown>;
  status: string;
}

export interface MobEventsResult {
  events: unknown[];
}

export interface MobMemberStatusResult {
  current_session_id?: string;
  error?: string;
  external_member?: unknown;
  is_final: boolean;
  kickoff?: unknown;
  output_preview?: string;
  peer_connectivity?: unknown;
  realtime_attachment_status?: string;
  status: WireMobMemberStatus;
  tokens_used: number;
}

export interface MobAppendSystemContextResult {
  agent_identity: string;
  mob_id: string;
  status: string;
}
