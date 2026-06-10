import { KNOWN_AGENT_EVENT_TYPES } from './generated/events.js';
import type { AgentEvent, SkillKey } from './generated/events.js';
import type { WireMobMemberStatus } from './generated/mob.js';

// ─── Bootstrap / session config (generated wire parity) ─────────
//
// K19: RuntimeConfig / InitResult / SessionConfig / AuthBindingRef are
// generated wire types (sdk-codegen `runtime` module), not hand-modeled
// here. The generated module also owns the fail-closed parse guards
// (`parseInitResult`) consumed by `runtime.ts`.
export type {
  AuthBindingRef,
  InitResult,
  RuntimeConfig,
  SessionConfig,
} from './generated/runtime.js';

/** A content block in a multimodal prompt. */
export type ContentBlock =
  | { type: 'text'; text: string }
  | { type: 'image'; media_type: string; data: string }
  | { type: 'video'; media_type: string; duration_ms: number; source?: 'inline'; data: string };

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

export type { SessionState, WireRunResult } from './generated/session.js';

/** Result of appending runtime system context to a mob member session. */
export interface MobAppendSystemContextResult {
  mob_id: string;
  agent_identity: string;
  status: 'staged' | 'duplicate';
}

/** Delivery receipt for a direct mob member turn. */
export interface MemberDeliveryReceipt {
  agent_identity: string;
  member_ref: MobMemberRef;
  handling_mode: HandlingMode;
}

/** Respawn receipt for a mob member. */
export interface MemberRespawnReceipt {
  agent_identity: string;
  member_ref: MobMemberRef;
}

/** Result envelope for a member respawn operation. */
export interface MobRespawnResult {
  status: 'completed' | 'topology_restore_failed';
  receipt: MemberRespawnReceipt;
  failed_peer_ids?: string[];
}

/**
 * Result of a turn execution.
 *
 * The wire truth is the generated {@link WireRunResult} twin (the same
 * envelope RPC's `turn/start` returns); this SDK type only adds the
 * backward-compatible `response` alias for the canonical `text`. The
 * runtime-owned terminal class is carried by `terminal_cause_kind`; there is
 * no fabricated in-band `status` string. Agent-level faults reject the
 * underlying promise instead of resolving with a synthetic failure payload.
 */
export type TurnResult = import('./generated/session.js').WireRunResult & {
  /** Backward-compatible alias for the canonical `text`. */
  response: string;
};

export type {
  AgentEvent,
  BudgetType,
  HookId,
  HookPoint,
  HookReasonCode,
  InteractionId,
  KnownAgentEventType,
  ReasoningCompleteEvent,
  ReasoningDeltaEvent,
  RunCompletedEvent,
  RunFailedEvent,
  RunStartedEvent,
  SessionId,
  SkillKey,
  SkillName,
  SkillsResolvedEvent,
  SkillResolutionFailedEvent,
  SourceUuid,
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
  CompactionStartedEvent,
  CompactionCompletedEvent,
  CompactionFailedEvent,
  BudgetWarningEvent,
  RetryingEvent,
  InteractionCompleteEvent,
  InteractionFailedEvent,
} from './generated/events.js';

/** Backward-compatible alias for the generated skill key shape. */
export type SkillId = SkillKey;

// ─── Mob types (matches meerkat-mob Rust wire format) ───────────

/** Closed provider vocabulary (matches Rust `meerkat_core::Provider`). */
export type ProviderName =
  | 'anthropic'
  | 'openai'
  | 'gemini'
  | 'self_hosted'
  | 'other';

/** Profile fields that win over durable session metadata on resume. */
export type ResumeOverrideField = 'model' | 'provider' | 'provider_params';

/**
 * User-defined model registry entry (`[models.<id>]`).
 *
 * Matches Rust `meerkat_core::config::CustomModelConfig`.
 */
export interface CustomModelEntry {
  /** Typed provider that serves this model. */
  provider: ProviderName;
  /** Human-readable display name (defaults to the model id). */
  display_name?: string;
  /** Model context window in tokens (drives compaction scaling). */
  context_window?: number;
  /** Maximum output tokens per call. */
  max_output_tokens?: number;
  /** Whether the model accepts image input (conservative default: false). */
  vision?: boolean;
  /** Whether the model supports provider-native web search. */
  web_search?: boolean;
  /** Default call timeout in seconds. */
  call_timeout_secs?: number;
}

/**
 * Mob definition passed to {@link MeerkatRuntime.createMob}.
 *
 * Matches Rust `MobDefinition` in `meerkat-mob/src/definition.rs`.
 */
export interface MobDefinition {
  id: string;
  profiles: Record<string, Profile>;
  /** Mob-scoped custom model registry entries (`[models.<id>]`). */
  models?: Record<string, CustomModelEntry>;
  /** Mob-level default provider for Auto image-generation targets. */
  image_generation_provider?: ProviderName;
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
  /** Explicit typed provider for the profile model. */
  provider?: ProviderName;
  /** Durable self-hosted server binding for configured self-hosted aliases. */
  self_hosted_server_id?: string;
  /** Configured default provider for Auto image-generation targets. */
  image_generation_provider?: ProviderName;
  /** Per-profile auto-compaction threshold override (tokens, non-zero). */
  auto_compact_threshold?: number;
  /** Profile fields that win over durable session metadata on resume. */
  resume_overrides?: ResumeOverrideField[];
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
  /** MCP server names this profile connects to. */
  mcp?: string[];
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
  agent_identity: string;
  runtime_mode?: 'turn_driven' | 'autonomous_host';
  initial_message?: string | ContentBlock[];
  labels?: Record<string, string>;
  context?: Record<string, unknown>;
  additional_instructions?: string[];
}

/**
 * Server-resolved opaque handle for a mob member. Treat as an opaque
 * token: app code never constructs or inspects these — they come back
 * from mob spawn and member-list surfaces, and are passed back on
 * work-lane and member-targeted calls.
 */
export type MobMemberRef = string;

/** Typed lifecycle action for the `mob/lifecycle` surface. */
export type MobLifecycleAction = 'stop' | 'resume' | 'complete' | 'reset' | 'destroy';

/** Result of a spawn operation. */
export interface SpawnResult {
  mob_id: string;
  agent_identity: string;
  member_ref: MobMemberRef;
}

/** A mob member entry from listMembers. */
export interface MobMember {
  agent_identity: string;
  member_ref: MobMemberRef;
  profile: string;
  peer_id?: string;
  external_peer_specs?: Record<string, Record<string, unknown>>;
  runtime_mode?: string;
  state?: string;
  wired_to?: string[];
  labels?: Record<string, string>;
  status?: string;
  error?: string;
  is_final?: boolean;
  kickoff?: Record<string, unknown>;
}

export interface ExternalPeerTarget {
  external: {
    name: string;
    address: string;
    identity: {
      kind: "ed25519_public_key";
      public_key: string;
    };
  };
}

export type MobPeerTarget = string | ExternalPeerTarget;

/** Mob status. */
export interface MobStatus {
  mob_id: string;
  status: string;
  /** @deprecated Use `status`. Kept as an inert projection of generated status. */
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

/**
 * Tri-state peer-connectivity projection for a mob member snapshot.
 *
 * Distinguishes a member with no bridge session (`not_applicable`) from a
 * transiently-unresolved live probe (`probe_timed_out`) from a resolved
 * connectivity snapshot (`known`). The legacy projection collapsed the first
 * two cases into an absent field.
 */
export type MobPeerConnectivity =
  | { status: 'not_applicable' }
  | { status: 'probe_timed_out' }
  | { status: 'known'; snapshot: MobPeerConnectivitySnapshot };

export interface ResolvedModelCapabilities {
  vision: boolean;
  image_input: boolean;
  image_tool_results: boolean;
  inline_video: boolean;
  realtime: boolean;
  web_search: boolean;
  image_generation: boolean;
}

/** Point-in-time execution snapshot for a mob member. */
export interface MobMemberSnapshot {
  status: WireMobMemberStatus;
  member_ref: MobMemberRef;
  output_preview?: string;
  error?: string;
  tokens_used: number;
  is_final: boolean;
  current_session_id?: string;
  resolved_capabilities?: ResolvedModelCapabilities;
  external_member?: unknown;
  kickoff?: Record<string, unknown>;
  peer_connectivity?: MobPeerConnectivity;
}

/** Result envelope for helper-style mob flows. */
export interface MobHelperResult {
  output?: string;
  tokens_used: number;
  agent_identity: string;
  member_ref: MobMemberRef;
}

// ─── Event types (matches meerkat-core AgentEvent serde) ────────

/** Typed event source identity used for event source semantics. */
export type EventSourceIdentity =
  | { type: 'session'; session_id: string }
  | { type: 'runtime'; runtime_id: string }
  | { type: 'interaction'; interaction_id: string }
  | { type: 'callback' }
  | { type: 'external'; source_id: string };

/** Envelope wrapping an agent event with metadata. */
export interface EventEnvelope {
  event_id: string;
  source: EventSourceIdentity;
  source_id: string;
  seq: number;
  mob_id?: string;
  timestamp_ms: number;
  agent_identity?: string;
  member_ref?: MobMemberRef;
  cursor?: string | number;
  payload: AgentEvent | { type: string; [key: string]: unknown };
}

/**
 * Direct-session event item.
 *
 * K19: a lagged receiver surfaces the generated `stream_truncated` AgentEvent
 * (reason `stream_lagged`) — the handwritten `{type:'lagged'}` sentinel is
 * deleted; the lag contract is the same generated event the RPC stream emits.
 */
export type SessionEvent = AgentEvent;

/** Member subscription item from the browser runtime. */
export type MemberEventItem = EventEnvelope;

/** Runtime identity for one mob member incarnation. */
export interface AgentRuntimeId {
  identity: string;
  generation: number;
}

/** Attributed mob-wide event from mob subscriptions. */
export interface AttributedEvent {
  source: AgentRuntimeId;
  source_fence_token?: number;
  role: string;
  envelope: EventEnvelope;
}

/** Mob-wide subscription item from the browser runtime. */
export type AttributedEventItem = AttributedEvent;

/** Structural mob event from the mob event log. */
export interface MobEvent {
  cursor: number;
  timestamp: string;
  mob_id: string;
  kind: Record<string, unknown>;
}

export { KNOWN_AGENT_EVENT_TYPES };

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isWireSkillKey(value: unknown): value is SkillKey {
  return (
    isRecord(value) &&
    isNonEmptyString(value.source_uuid) &&
    isNonEmptyString(value.skill_name)
  );
}

function hasStringFields(value: Record<string, unknown>, fields: readonly string[]): boolean {
  return fields.every((field) => typeof value[field] === 'string');
}

function isSkillResolutionFailureReason(value: unknown): boolean {
  if (!isRecord(value) || typeof value.reason_type !== 'string') {
    return false;
  }
  switch (value.reason_type) {
    case 'not_found':
      return isWireSkillKey(value.key);
    case 'capability_unavailable':
      return isWireSkillKey(value.key) && isNonEmptyString(value.capability);
    case 'load':
    case 'parse':
    case 'unknown':
      return typeof value.message === 'string';
    case 'source_uuid_collision':
      return hasStringFields(value, ['source_uuid', 'existing_fingerprint', 'new_fingerprint']);
    case 'source_uuid_mutation_without_lineage':
      return hasStringFields(value, ['fingerprint', 'existing_source_uuid', 'mutated_source_uuid']);
    case 'missing_skill_remaps':
      return hasStringFields(value, ['event_id', 'event_kind']);
    case 'remap_without_lineage':
      return hasStringFields(value, [
        'from_source_uuid',
        'from_skill_name',
        'to_source_uuid',
        'to_skill_name',
      ]);
    case 'unknown_skill_alias':
      return typeof value.alias === 'string';
    case 'remap_cycle':
      return hasStringFields(value, ['source_uuid', 'skill_name']);
    default:
      return false;
  }
}

function isKnownSkillResolutionEvent(event: { type: string }): boolean {
  const value = event as Record<string, unknown>;

  if (event.type === 'skills_resolved') {
    return (
      Array.isArray(value.skills) &&
      value.skills.every(isWireSkillKey) &&
      typeof value.injection_bytes === 'number' &&
      Number.isFinite(value.injection_bytes) &&
      value.injection_bytes >= 0
    );
  }

  if (event.type === 'skill_resolution_failed') {
    return (
      (value.skill_key == null || isWireSkillKey(value.skill_key)) &&
      (value.reason == null || isSkillResolutionFailureReason(value.reason))
    );
  }

  return true;
}

/** Type guard for known event types. */
export function isKnownEvent(event: { type: string }): event is AgentEvent {
  return (
    (KNOWN_AGENT_EVENT_TYPES as readonly string[]).includes(event.type) &&
    isKnownSkillResolutionEvent(event)
  );
}

// ─── Tool callback types ────────────────────────────────────────

/** JSON schema object for tool input. */
export type JsonSchema = Record<string, unknown>;

/**
 * Result returned from a tool callback.
 *
 * `content` mirrors the canonical `WireToolResultContent` union
 * (`string | ContentBlock[]`): plain text OR typed multimodal blocks
 * (images, video). Block content survives to the agent as typed
 * {@link ContentBlock}s rather than being narrowed to a string at the
 * WASM boundary.
 */
export interface ToolCallbackResult {
  content: string | ContentBlock[];
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
