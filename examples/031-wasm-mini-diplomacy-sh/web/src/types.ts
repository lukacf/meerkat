// ═══════════════════════════════════════════════════════════
// Core Types & Constants
// ═══════════════════════════════════════════════════════════

export type Team = "france" | "prussia" | "russia";
export const TEAMS: Team[] = ["france", "prussia", "russia"];
export const TEAM_LABELS: Record<Team, string> = { france: "France", prussia: "Prussia", russia: "Russia" };

export interface RegionState { id: string; controller: Team; defense: number; value: number }
export interface ArenaState {
  turn: number; max_turns: number; regions: RegionState[];
  scores: Record<Team, number>; winner?: Team | "draw";
}
export interface OrderSet { team: Team; aggression: number; fortify: number; target_region: string }
export interface TurnDecision { order: OrderSet; reasoning: string }

// ── DM Channels ──
export type ChannelId =
  | "f-plan-op" | "p-plan-op" | "r-plan-op"
  | "f-plan-amb" | "p-plan-amb" | "r-plan-amb"
  | "f-p-diplo" | "f-r-diplo" | "p-r-diplo"
  | "narrator";
export type MessageRole = "planner" | "operator" | "ambassador" | "narrator" | "system";
export interface ChatMessage {
  channel: ChannelId; role: MessageRole; faction: Team | "neutral";
  content: string; turn: number;
  /** Structured headline from extraction turn (API-enforced schema). */
  headline?: string;
  /** Structured details from extraction turn. */
  details?: string;
  /** Global sequence number for ordering across all channels. */
  seq: number;
}

export const CHANNELS: { id: ChannelId; label: string; subtitle: string; icon: string }[] = [
  { id: "f-plan-op",  label: "France",          subtitle: "Planner \u2194 Operator",              icon: "\u2694" },
  { id: "p-plan-op",  label: "Prussia",         subtitle: "Planner \u2194 Operator",              icon: "\u2694" },
  { id: "r-plan-op",  label: "Russia",          subtitle: "Planner \u2194 Operator",              icon: "\u2694" },
  { id: "f-plan-amb", label: "France",          subtitle: "Planner \u2194 Ambassador",            icon: "\u270D" },
  { id: "p-plan-amb", label: "Prussia",         subtitle: "Planner \u2194 Ambassador",            icon: "\u270D" },
  { id: "r-plan-amb", label: "Russia",          subtitle: "Planner \u2194 Ambassador",            icon: "\u270D" },
  { id: "f-p-diplo",  label: "Franco-Prussian", subtitle: "Ambassadors negotiate",                icon: "\u{1F91D}" },
  { id: "f-r-diplo",  label: "Franco-Russian",  subtitle: "Ambassadors negotiate",                icon: "\u{1F91D}" },
  { id: "p-r-diplo",  label: "Prussian-Russian",subtitle: "Ambassadors negotiate",                icon: "\u{1F91D}" },
  { id: "narrator",   label: "Correspondent",   subtitle: "War narrative",                        icon: "\u{1F4DC}" },
];

// ── WASM Runtime ──
export interface RuntimeModule {
  default: () => Promise<unknown>;
  init_runtime_from_config: (configJson: string) => unknown;
  mob_create: (definitionJson: string) => Promise<unknown>;
  mob_spawn: (mobId: string, specsJson: string) => Promise<unknown>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  /** Resolve a member's cross-mob peer target (JSON PeerTarget) for mob_wire_peer. */
  mob_member_peer_target: (mobId: string, member: string) => Promise<string>;
  /** Install trust from `member` to an external peer (JSON PeerTarget). */
  mob_wire_peer: (mobId: string, member: string, peerJson: string) => Promise<void>;
  mob_member_send: (mobId: string, meerkatId: string, requestJson: string) => Promise<string>;
  mob_run_flow: (mobId: string, flowId: string, paramsJson: string) => Promise<unknown>;
  mob_flow_status: (mobId: string, runId: string) => Promise<unknown>;
  mob_list_members: (mobId: string) => Promise<unknown>;
  mob_member_subscribe: (mobId: string, meerkatId: string) => Promise<string>;
  poll_subscription: (handle: string) => string;
  close_subscription: (handle: string) => void;
}

export interface AgentSub { meerkatId: string; handle: string; role: MessageRole; team: Team }
export interface FactionMob { team: Team; mobId: string }
export interface MatchSession {
  factions: FactionMob[];
  narratorMobId: string | null;
  subs: AgentSub[];
  state: ArenaState;
  messages: ChatMessage[];
  running: boolean;
  prevControllers: Map<string, Team>;
  seenToolCallIds: Set<string>;
}
