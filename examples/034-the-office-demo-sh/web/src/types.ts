// ═══════════════════════════════════════════════════════════
// The Office — Core Types
// ═══════════════════════════════════════════════════════════

export const AGENT_IDS = [
  "triage", "it-dept", "hr-dept", "facilities", "finance",
  "alex-pa", "sam-pa", "pat-pa", "gate", "archivist",
] as const;
export type AgentId = (typeof AGENT_IDS)[number];

export interface AgentMeta {
  id: AgentId;
  name: string;
  role: string;
  zone: string;
  color: string;
}

export const AGENTS: Record<AgentId, AgentMeta> = {
  triage:     { id: "triage",     name: "Max",    role: "Triage",       zone: "Mail Room",   color: "#4488CC" },
  "it-dept":  { id: "it-dept",    name: "Dev",    role: "IT",           zone: "Dept Row",    color: "#44AA66" },
  "hr-dept":  { id: "hr-dept",    name: "Robin",  role: "HR",           zone: "Dept Row",    color: "#CC6688" },
  facilities: { id: "facilities", name: "Jordan", role: "Facilities",   zone: "Dept Row",    color: "#AA8844" },
  finance:    { id: "finance",    name: "Morgan", role: "Finance",      zone: "Dept Row",    color: "#8866CC" },
  "alex-pa":  { id: "alex-pa",    name: "Aria",   role: "Alex's PA",    zone: "PA Bullpen",  color: "#CC4466" },
  "sam-pa":   { id: "sam-pa",     name: "Scout",  role: "Sam's PA",     zone: "PA Bullpen",  color: "#44AACC" },
  "pat-pa":   { id: "pat-pa",     name: "Quinn",  role: "Pat's PA",     zone: "PA Bullpen",  color: "#CCAA44" },
  gate:       { id: "gate",       name: "Bailey", role: "Compliance",   zone: "Gate",        color: "#CC4444" },
  archivist:  { id: "archivist",  name: "Sage",   role: "Archivist",    zone: "Archive",     color: "#66AA88" },
};

// ── Character State ──
export type CharacterState = "idle" | "on_call" | "thinking";

export interface AgentState {
  id: AgentId;
  state: CharacterState;
  frameIndex: number;
  frameTimer: number;
  /** Currently active call IDs involving this agent */
  activeCalls: Set<string>;
}

// ── Phone Calls ──
export interface PhoneCall {
  id: string;
  from: AgentId;
  to: AgentId;
  color: string;
  startTime: number;
  fadeStart: number | null;
  opacity: number;
}

export const CALL_COLORS: Record<string, string> = {
  routing:   "#4488CC",
  analysis:  "#44AA66",
  action:    "#CC8844",
  knowledge: "#66AA88",
  approval:  "#CC4444",
  response:  "#8866CC",
};

// ── Speech / Thought Bubbles ──
export interface SpeechBubble {
  agentId: AgentId;
  text: string;
  startTime: number;
  duration: number;
}

export interface ThinkBubble {
  agentId: AgentId;
  dotPhase: number;
}

// ── Incidents ──
export interface IncidentMessage {
  from: AgentId | "user" | "system";
  to: AgentId | "system";
  content: string;
  headline: string;
  timestamp: number;
  category: string;
}

export interface Incident {
  id: string;
  title: string;
  icon: string;
  timestamp: number;
  status: "active" | "resolved";
  messages: IncidentMessage[];
}

// ── WASM Runtime ──
export interface RuntimeModule {
  default: () => Promise<unknown>;
  init_runtime_from_config: (configJson: string) => unknown;
  mob_create: (definitionJson: string) => Promise<unknown>;
  mob_spawn: (mobId: string, specsJson: string) => Promise<unknown>;
  mob_wire: (mobId: string, a: string, b: string) => Promise<void>;
  mob_send_message: (mobId: string, meerkatId: string, message: string) => Promise<void>;
  mob_member_subscribe: (mobId: string, meerkatId: string) => Promise<number>;
  poll_subscription: (handle: number) => string;
  close_subscription: (handle: number) => void;
  mob_run_flow: (mobId: string, flowId: string, paramsJson: string) => Promise<unknown>;
  mob_flow_status: (mobId: string, runId: string) => Promise<unknown>;
  mob_list_members: (mobId: string) => Promise<unknown>;
  mob_status: (mobId: string) => Promise<unknown>;
}

// ── Session ──
export interface AgentSub {
  agentId: AgentId;
  handle: number;
}

export interface OfficeSession {
  mobId: string;
  subs: AgentSub[];
  agents: Map<AgentId, AgentState>;
  incidents: Incident[];
  running: boolean;
  seenToolCallIds: Set<string>;
}
