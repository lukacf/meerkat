import { KNOWN_AGENT_EVENT_TYPES } from './generated/events.js';
import type {
  AgentErrorReport,
  AgentEvent,
  SkillKey,
  StreamLaggedEvent,
  TurnTerminalCauseKind,
  TurnTerminalOutcome,
  Usage,
} from './generated/events.js';
import type {
  MobAppendSystemContextResult as GeneratedMobAppendSystemContextResult,
  MobDefinitionInput,
  MobHelperResult as GeneratedMobHelperResult,
  MobMemberListEntryWire,
  MobMemberSendResult as GeneratedMobMemberSendResult,
  MobMemberStatusResult as GeneratedMobMemberStatusResult,
  MobProfileInput,
  MobRespawnResult as GeneratedMobRespawnResult,
  MobRoleWiringRuleInput,
  MobSpawnResult as GeneratedMobSpawnResult,
  MobSpawnSpecParams,
  MobStatusResult as GeneratedMobStatusResult,
  MobToolConfigInput,
  MobWiringRulesInput,
  WireHandlingMode,
  WireMobMemberStatus,
  WireMemberRef,
  WireMobEvent,
  WireMobRun,
  WireRenderClass,
  WireRenderMetadata,
  WireRenderSalience,
  WireResolvedModelCapabilities,
  WireTrustedPeerSpec,
} from './generated/mob.js';

// ─── Bootstrap config ───────────────────────────────────────────

/** Configuration for {@link MeerkatRuntime.init}. */
export interface RuntimeConfig {
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

/** Structural reference to a realm binding. */
export interface AuthBindingRef {
  realm: string;
  binding: string;
  profile?: string;
}

/** Configuration for creating a direct (non-mob) session.
 *
 * Plan §4d.wasm.2 + §6.13: per-session api_key / base_url fields are
 * deleted. Credentials flow from bootstrap-populated realm config
 * (`initRuntimeFromConfig`) or the host's registered external-auth
 * resolver (`register_external_auth_resolver`).
 */
export interface SessionConfig {
  /** LLM model identifier. */
  model: string;
  /** Optional structural auth binding reference. */
  authBinding?: AuthBindingRef;
  /** System prompt. */
  systemPrompt?: string;
  /** Max tokens per response. Default: 4096. */
  maxTokens?: number;
  /** Enable comms for this session. */
  commsName?: string;
  /** Whether this session runs in keep-alive mode. */
  keepAlive?: boolean;
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
  | { type: 'image'; media_type: string; data: string }
  | { type: 'video'; media_type: string; duration_ms: number; source?: 'inline'; data: string };

/** Canonical ordinary content input. */
export type ContentInput = string | ContentBlock[];

/** Runtime handling mode for ordinary work. */
export type HandlingMode = WireHandlingMode;

/** Standardized rendering class for injected ordinary work. */
export type RenderClass = WireRenderClass;

/** Normalized rendering salience for injected work. */
export type RenderSalience = WireRenderSalience;

/** Normalized rendering metadata for injected work. */
export type RenderMetadata = WireRenderMetadata;

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
  session_id: string;
  status: 'staged' | 'duplicate';
}

/** Runtime-backed state for a direct browser session façade. */
export interface SessionState {
  session_id: string;
  mob_id: string;
  model: string;
  usage: Usage;
  /** @deprecated Direct browser sessions no longer expose a local run counter. */
  run_counter?: number;
  message_count: number;
  is_active: boolean;
  last_assistant_text?: string | null;
}

/** Result of appending runtime system context to a mob member session. */
export type MobAppendSystemContextResult = GeneratedMobAppendSystemContextResult;

/** Delivery receipt for a direct mob member turn. */
export type MemberDeliveryReceipt = GeneratedMobMemberSendResult;

/** Respawn receipt for a mob member. */
export type MemberRespawnReceipt = GeneratedMobRespawnResult['receipt'];

/** Result envelope for a member respawn operation. */
export type MobRespawnResult = GeneratedMobRespawnResult;

/** Canonical terminal result for a direct browser turn. */
export interface TurnTerminalResult {
  outcome: TurnTerminalOutcome;
  cause_kind?: TurnTerminalCauseKind;
  error?: AgentErrorReport;
  metadata?: unknown;
}

/** Result of a turn execution. */
export interface TurnResult {
  /** Canonical text returned by the runtime. */
  text: string;
  /** Backward-compatible alias for {@link text}. */
  response: string;
  usage: Usage;
  tool_calls: number;
  terminal: TurnTerminalResult;
  turns?: number;
  session_id?: string;
  /** @deprecated Use terminal.outcome. */
  status?: TurnTerminalOutcome;
}

export type {
  AgentErrorReport,
  AgentEvent,
  BudgetType,
  HookId,
  HookPoint,
  HookReasonCode,
  InteractionId,
  KnownAgentEventType,
  ProviderImageMetadata,
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
  ToolResultError,
  ToolResultReceivedEvent,
  TurnTerminalCauseKind,
  TurnTerminalOutcome,
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

export type MobDefinition = MobDefinitionInput;

export type Profile = MobProfileInput;

export type ToolConfig = MobToolConfigInput;

export type WiringRules = MobWiringRulesInput;

export type RoleWiringRule = MobRoleWiringRuleInput;

/** Spawn specification for a single agent within a mob. */
export type SpawnSpec = Omit<MobSpawnSpecParams, 'initial_message'> & {
  initial_message?: ContentInput;
};

/**
 * Server-resolved opaque handle for a mob member. Treat as an opaque
 * token: app code never constructs or inspects these — they come back
 * from mob spawn and member-list surfaces, and are passed back on
 * work-lane and member-targeted calls.
 */
export type MobMemberRef = WireMemberRef;

/** Typed lifecycle action for the `mob/lifecycle` surface. */
export type MobLifecycleAction = 'stop' | 'resume' | 'complete' | 'reset' | 'destroy';

/** Result of a mob lifecycle action. */
export interface MobLifecycleResult {
  mob_id: string;
  action: MobLifecycleAction;
  ok: boolean;
  destroy_report?: unknown;
}

/** Result of a spawn operation. */
export type SpawnResult = GeneratedMobSpawnResult;

/** A mob member entry from listMembers. */
export type MobMember = MobMemberListEntryWire & {
  /** @deprecated Use `role`, the generated roster field. */
  profile: MobMemberListEntryWire['role'];
  peer_id?: string;
  external_peer_specs?: Record<string, Record<string, unknown>>;
  kickoff?: Record<string, unknown>;
};

export interface ExternalPeerTarget {
  external: WireTrustedPeerSpec;
}

export type MobPeerTarget = string | ExternalPeerTarget;

export type MobLifecycleStatus =
  | 'Creating'
  | 'Running'
  | 'Stopped'
  | 'Completed'
  | 'Destroyed';

/** Mob status. */
export type MobStatus = GeneratedMobStatusResult;

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

export type ResolvedModelCapabilities = WireResolvedModelCapabilities;

/** Point-in-time execution snapshot for a mob member. */
export type MobMemberSnapshot = Omit<
  GeneratedMobMemberStatusResult,
  'peer_connectivity' | 'resolved_capabilities'
> & {
  member_ref: MobMemberRef;
  resolved_capabilities?: ResolvedModelCapabilities;
  peer_connectivity?: MobPeerConnectivitySnapshot;
};

/** Result envelope for helper-style mob flows. */
export type MobHelperResult = GeneratedMobHelperResult;

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

/** Poll/subscribe lag sentinel emitted by the browser runtime. */
export type SubscriptionLaggedEvent = StreamLaggedEvent;

/** Direct-session event item. */
export type SessionEvent = AgentEvent | SubscriptionLaggedEvent;

/** Member subscription item from the browser runtime. */
export type MemberEventItem = EventEnvelope | SubscriptionLaggedEvent;

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
export type AttributedEventItem = AttributedEvent | SubscriptionLaggedEvent;

/** Structural mob event from the mob event log. */
export type MobEvent = WireMobEvent;

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
      isSkillResolutionFailureReason(value.reason)
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

/** Result returned from a tool callback. */
export interface ToolCallbackResult {
  content: ContentInput;
  is_error: boolean;
}

/** A tool callback function. */
export type ToolCallback = (args: string) => Promise<ToolCallbackResult>;

// ─── Flow types ─────────────────────────────────────────────────

/** Status of a running flow. */
export type FlowStatus = WireMobRun;
