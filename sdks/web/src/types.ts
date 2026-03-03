// ─── Bootstrap config ───────────────────────────────────────────

/** Configuration for {@link MeerkatRuntime.init}. */
export interface RuntimeConfig {
  /** Backward-compat single API key (treated as Anthropic fallback). */
  apiKey?: string;
  /** Anthropic API key. */
  anthropicApiKey?: string;
  /** OpenAI API key. */
  openaiApiKey?: string;
  /** Gemini API key. */
  geminiApiKey?: string;
  /** Default model for new sessions. */
  model?: string;
  /** Maximum concurrent sessions. Default: 64. */
  maxSessions?: number;
  /** Backward-compat single base URL (mapped to default model's provider). */
  baseUrl?: string;
  /** Anthropic base URL (e.g. for proxy deployments). */
  anthropicBaseUrl?: string;
  /** OpenAI base URL. */
  openaiBaseUrl?: string;
  /** Gemini base URL. */
  geminiBaseUrl?: string;
}

/** Result from runtime initialization. */
export interface InitResult {
  status: 'initialized';
  model: string;
  providers: string[];
  max_sessions?: number;
}

// ─── Session config ─────────────────────────────────────────────

/** Configuration for creating a direct (non-mob) session. */
export interface SessionConfig {
  /** LLM model identifier. */
  model: string;
  /** API key for the model's provider. */
  apiKey: string;
  /** System prompt. */
  systemPrompt?: string;
  /** Max tokens per response. Default: 4096. */
  maxTokens?: number;
  /** Backward-compat single base URL. */
  baseUrl?: string;
  /** Anthropic base URL. */
  anthropicBaseUrl?: string;
  /** OpenAI base URL. */
  openaiBaseUrl?: string;
  /** Gemini base URL. */
  geminiBaseUrl?: string;
  /** Enable comms for this session. */
  commsName?: string;
  /** Whether this session runs in host mode. */
  hostMode?: boolean;
  /** Application-defined labels. */
  labels?: Record<string, string>;
  /** Additional instruction sections appended to the system prompt. */
  additionalInstructions?: string[];
  /** Opaque application context. */
  appContext?: unknown;
}

/** Options for a single turn. */
export interface TurnOptions {
  /** Additional instructions for this turn only. */
  additionalInstructions?: string[];
}

/** Result of a turn execution. */
export interface TurnResult {
  response: string;
  usage: Usage;
  tool_calls: number;
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
}

// ─── Mob types ──────────────────────────────────────────────────

/** Mob definition passed to {@link MeerkatRuntime.createMob}. */
export interface MobDefinition {
  id: string;
  profiles: Record<string, MobProfile>;
  flows?: Record<string, unknown>;
  wiring?: WiringSpec[];
}

export interface MobProfile {
  model: string;
  system_prompt?: string;
  tools?: ToolSpec;
  max_tokens?: number;
}

export interface ToolSpec {
  builtins?: boolean;
  shell?: boolean;
  comms?: boolean;
  memory?: boolean;
}

export interface WiringSpec {
  a: string;
  b: string;
}

/** Spawn specification for a single agent within a mob. */
export interface SpawnSpec {
  profile: string;
  meerkat_id: string;
  runtime_mode?: 'turn_driven' | 'autonomous_host';
  initial_message?: string;
  labels?: Record<string, string>;
}

/** Result of a spawn operation. */
export interface SpawnResult {
  meerkat_id: string;
  status: string;
}

/** A mob member entry from listMembers. */
export interface MobMember {
  meerkat_id: string;
  profile: string;
  status: string;
}

/** Mob status. */
export interface MobStatus {
  mob_id: string;
  status: string;
  member_count: number;
}

/** Mob lifecycle actions. */
export type MobLifecycleAction = 'stop' | 'resume' | 'complete' | 'destroy';

// ─── Event types ────────────────────────────────────────────────

/** Envelope wrapping an agent event with metadata. */
export interface EventEnvelope {
  session_id?: string;
  meerkat_id?: string;
  cursor?: string;
  event: AgentEvent | { type: string; [key: string]: unknown };
}

/** Known agent event types (discriminated union on `type`). */
export type AgentEvent =
  | TextDeltaEvent
  | TextCompleteEvent
  | ToolUseStartEvent
  | ToolResultEvent
  | TurnCompleteEvent
  | TurnErrorEvent
  | CommsReceivedEvent;

export interface TextDeltaEvent { type: 'text_delta'; text: string }
export interface TextCompleteEvent { type: 'text_complete'; text: string }
export interface ToolUseStartEvent { type: 'tool_use_start'; tool_use_id: string; name: string }
export interface ToolResultEvent { type: 'tool_result'; tool_use_id: string; content: string; is_error: boolean }
export interface TurnCompleteEvent { type: 'turn_complete'; usage: Usage }
export interface TurnErrorEvent { type: 'turn_error'; error: string }
export interface CommsReceivedEvent { type: 'comms_received'; from: string; body: string }

/** Type guard for known event types. Unknown events have a `type` not in the known set. */
export function isKnownEvent(event: { type: string }): event is AgentEvent {
  return [
    'text_delta', 'text_complete', 'tool_use_start', 'tool_result',
    'turn_complete', 'turn_error', 'comms_received',
  ].includes(event.type);
}

// ─── Tool callback types ────────────────────────────────────────

/** JSON schema object for tool input. */
export type JsonSchema = Record<string, unknown>;

/** Result returned from a tool callback. */
export interface ToolCallbackResult {
  content: string;
  is_error: boolean;
}

/** A tool callback function. */
export type ToolCallback = (args: string) => Promise<ToolCallbackResult>;

// ─── Flow types ─────────────────────────────────────────────────

/** Status of a running flow. */
export interface FlowStatus {
  run_id: string;
  status: string;
  [key: string]: unknown;
}
