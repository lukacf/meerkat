import { KNOWN_AGENT_EVENT_TYPES } from './generated/events.js';
import type { AgentEvent, Usage } from './generated/events.js';

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

/** A content block in a multimodal prompt. */
export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; media_type: string; data: string };

/** Canonical ordinary content input. */
export type ContentInput = string | ContentBlock[];

/** Runtime handling mode for ordinary work. */
export type HandlingMode = 'queue' | 'steer';

/** Standardized rendering class for injected ordinary work. */
export type RenderClass =
  | 'user_prompt'
  | 'peer_message'
  | 'peer_request'
  | 'peer_response'
  | 'external_event'
  | 'flow_step'
  | 'continuation'
  | 'system_notice'
  | 'tool_scope_notice'
  | 'ops_progress';

/** Normalized rendering salience for injected work. */
export type RenderSalience = 'background' | 'normal' | 'important' | 'urgent';

/** Normalized rendering metadata for injected work. */
export interface RenderMetadata {
  class: RenderClass;
  salience?: RenderSalience;
}

/** Options for a single turn. */
export interface TurnOptions {
  /** Additional instructions for this turn only. */
  additionalInstructions?: string[];
}

/** Runtime system-context append request. */
export interface AppendSystemContextOptions {
  /** Instruction text injected at the next LLM boundary. */
  text: string;
  /** Optional source label for provenance/debugging. */
  source?: string;
  /** Optional per-session idempotency key. */
  idempotencyKey?: string;
}

/** Result of a runtime system-context append request. */
export interface AppendSystemContextResult {
  handle: number;
  status: 'staged' | 'duplicate';
}

/** Runtime-backed state for a direct browser session façade. */
export interface SessionState {
  handle: number;
  session_id: string;
  mob_id: string;
  model: string;
  usage: Usage;
  run_counter: number;
  message_count: number;
  is_active: boolean;
  last_assistant_text?: string | null;
}

/** Result of appending runtime system context to a mob member session. */
export interface MobAppendSystemContextResult {
  mob_id: string;
  meerkat_id: string;
  session_id: string;
  status: 'staged' | 'duplicate';
}

/** Delivery receipt for a direct mob member turn. */
export interface MemberDeliveryReceipt {
  member_id: string;
  session_id: string;
  handling_mode: HandlingMode;
}

/** Respawn receipt for a mob member. */
export interface MemberRespawnReceipt {
  member_id: string;
  old_session_id?: string | null;
  new_session_id?: string | null;
}

/** Result envelope for a member respawn operation. */
export interface MobRespawnResult {
  status: 'completed' | 'topology_restore_failed';
  receipt: MemberRespawnReceipt;
  failed_peer_ids?: string[];
}

/** Result of a turn execution. */
export interface TurnResult {
  /** Canonical text returned by the runtime. */
  text: string;
  /** Backward-compatible alias for {@link text}. */
  response: string;
  usage: Usage;
  tool_calls: number;
  turns?: number;
  session_id?: string;
  status?: string;
}

export type {
  AgentEvent,
  BudgetType,
  HookId,
  HookPatch,
  HookPatchEnvelope,
  HookPoint,
  HookReasonCode,
  HookRevision,
  InteractionId,
  KnownAgentEventType,
  ReasoningCompleteEvent,
  ReasoningDeltaEvent,
  RunCompletedEvent,
  RunFailedEvent,
  RunStartedEvent,
  SessionId,
  SkillId,
  SkillsResolvedEvent,
  SkillResolutionFailedEvent,
  StopReason,
  StreamTruncatedEvent,
  TextCompleteEvent,
  TextDeltaEvent,
  ToolCallRequestedEvent,
  ToolConfigChangeOperation,
  ToolConfigChangedEvent,
  ToolConfigChangedPayload,
  ToolExecutionCompletedEvent,
  ToolExecutionStartedEvent,
  ToolExecutionTimedOutEvent,
  ToolResultReceivedEvent,
  TurnCompletedEvent,
  TurnStartedEvent,
  Usage,
  HookStartedEvent,
  HookCompletedEvent,
  HookFailedEvent,
  HookDeniedEvent,
  HookRewriteAppliedEvent,
  HookPatchPublishedEvent,
  CompactionStartedEvent,
  CompactionCompletedEvent,
  CompactionFailedEvent,
  BudgetWarningEvent,
  RetryingEvent,
  InteractionCompleteEvent,
  InteractionFailedEvent,
} from './generated/events.js';

// ─── Mob types (matches meerkat-mob Rust wire format) ───────────

/**
 * Mob definition passed to {@link MeerkatRuntime.createMob}.
 *
 * Matches Rust `MobDefinition` in `meerkat-mob/src/definition.rs`.
 */
export interface MobDefinition {
  id: string;
  profiles: Record<string, Profile>;
  /** Wiring rules for automatic peer connections. */
  wiring?: WiringRules;
  /** Named flow definitions. */
  flows?: Record<string, unknown>;
  /** Named MCP server configurations. */
  mcp_servers?: Record<string, unknown>;
  /** Named skill sources. */
  skills?: Record<string, unknown>;
  /** Orchestrator configuration. */
  orchestrator?: unknown;
  /** Backend selection config. */
  backend?: unknown;
}

/**
 * Profile template for spawning agents.
 *
 * Matches Rust `Profile` in `meerkat-mob/src/profile.rs`.
 * Note: there is NO `system_prompt` field — prompts are built from skills.
 */
export interface Profile {
  /** LLM model name (e.g. "claude-sonnet-4-5"). */
  model: string;
  /** Skill references to load. */
  skills?: string[];
  /** Tool configuration. */
  tools?: ToolConfig;
  /** Human-readable role description visible to peers. */
  peer_description?: string;
  /** Whether this agent can receive turns from external callers. */
  external_addressable?: boolean;
  /** Runtime mode: 'turn_driven' or 'autonomous_host'. */
  runtime_mode?: string;
  /** Max peer-count threshold for inline peer lifecycle notifications. */
  max_inline_peer_notifications?: number;
  /** JSON Schema for structured output extraction. */
  output_schema?: unknown;
  /** Provider-specific parameters (e.g. thinking_budget, reasoning_effort). */
  provider_params?: unknown;
}

/** Tool configuration for a profile. Matches Rust `ToolConfig`. */
export interface ToolConfig {
  /** Enable built-in tools (file read, etc.). */
  builtins?: boolean;
  /** Enable shell execution tool. */
  shell?: boolean;
  /** Enable comms tools (peer messaging). */
  comms?: boolean;
  /** Enable memory/semantic search tools. */
  memory?: boolean;
  /** Enable mob management tools (spawn, retire, wire, unwire, list). */
  mob?: boolean;
  /** Enable shared task list tools. */
  mob_tasks?: boolean;
  /** MCP server names this profile connects to. */
  mcp?: string[];
  /** Named Rust tool bundles (re-registered at mob construction time). */
  rust_bundles?: string[];
}

/** Wiring rules controlling automatic peer connections. */
export interface WiringRules {
  /** Automatically wire every spawned agent to the orchestrator. */
  auto_wire_orchestrator?: boolean;
  /** Fan-out wiring rules between profile roles. */
  role_wiring?: RoleWiringRule[];
}

/** Wiring rule between two profile roles. */
export interface RoleWiringRule {
  /** First profile name. */
  a: string;
  /** Second profile name. */
  b: string;
}

/** Spawn specification for a single agent within a mob. */
export interface SpawnSpec {
  profile: string;
  meerkat_id: string;
  runtime_mode?: 'turn_driven' | 'autonomous_host';
  initial_message?: string | ContentBlock[];
  labels?: Record<string, string>;
}

/** Result of a spawn operation. */
export interface SpawnResult {
  status: 'ok' | 'error';
  member_ref?: Record<string, unknown> | null;
  error?: string;
}

/** A mob member entry from listMembers. */
export interface MobMember {
  meerkat_id: string;
  profile: string;
  member_ref: Record<string, unknown>;
  peer_id?: string;
  external_peer_specs?: Record<string, Record<string, unknown>>;
  runtime_mode?: string;
  state?: string;
  wired_to?: string[];
  labels?: Record<string, string>;
}

export interface ExternalPeerTarget {
  external: {
    name: string;
    peer_id: string;
    address: string;
  };
}

export type MobPeerTarget = string | ExternalPeerTarget;

/** Mob status. */
export interface MobStatus {
  mob_id: string;
  state: string;
}

/** Unreachable peer entry from a live member connectivity snapshot. */
export interface MobUnreachablePeer {
  peer: string;
  reason?: string;
}

/** Live peer connectivity projection for a mob member snapshot. */
export interface MobPeerConnectivitySnapshot {
  reachable_peer_count: number;
  unknown_peer_count: number;
  unreachable_peers: MobUnreachablePeer[];
}

/** Point-in-time execution snapshot for a mob member. */
export interface MobMemberSnapshot {
  status: string;
  output_preview?: string;
  error?: string;
  tokens_used: number;
  is_final: boolean;
  current_session_id?: string;
  peer_connectivity?: MobPeerConnectivitySnapshot;
}

/** Result envelope for helper-style mob flows. */
export interface MobHelperResult {
  output?: string;
  tokens_used: number;
  session_id?: string;
}

/** Mob lifecycle actions. */
export type MobLifecycleAction = 'stop' | 'resume' | 'complete' | 'reset' | 'destroy';

// ─── Event types (matches meerkat-core AgentEvent serde) ────────

/** Envelope wrapping an agent event with metadata. */
export interface EventEnvelope {
  session_id?: string;
  meerkat_id?: string;
  cursor?: string | number;
  event: AgentEvent | { type: string; [key: string]: unknown };
}

/** Poll/subscribe lag sentinel emitted by the browser runtime. */
export interface SubscriptionLaggedEvent {
  type: 'lagged';
  skipped: number;
}

/** Direct-session event item. */
export type SessionEvent = AgentEvent | SubscriptionLaggedEvent;

/** Member subscription item from the browser runtime. */
export type MemberEventItem = EventEnvelope | SubscriptionLaggedEvent;

/** Attributed mob-wide event from mob subscriptions. */
export interface AttributedEvent {
  source: string;
  profile: string;
  envelope: EventEnvelope;
}

/** Mob-wide subscription item from the browser runtime. */
export type AttributedEventItem = AttributedEvent | SubscriptionLaggedEvent;

/** Structural mob event from the mob event log. */
export interface MobEvent {
  cursor: number;
  timestamp: string;
  mob_id: string;
  kind: Record<string, unknown>;
}

export { KNOWN_AGENT_EVENT_TYPES };

/** Type guard for known event types. */
export function isKnownEvent(event: { type: string }): event is AgentEvent {
  return (KNOWN_AGENT_EVENT_TYPES as readonly string[]).includes(event.type);
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
