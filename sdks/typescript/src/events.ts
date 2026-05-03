/**
 * Typed event hierarchy for the Meerkat streaming API.
 *
 * Events form a discriminated union on the `type` field (snake_case to match
 * the wire protocol).  All other fields use idiomatic camelCase.
 * Missing semantic fields remain absent during parsing so partial streaming
 * payloads do not become authoritative SDK state.
 * Malformed known event payloads are preserved as `malformed_event` frames
 * instead of being coerced into typed semantic events with fabricated defaults.
 *
 * @example
 * ```ts
 * for await (const event of session.stream("Explain monads")) {
 *   switch (event.type) {
 *     case "text_delta":
 *       process.stdout.write(event.delta);
 *       break;
 *     case "tool_call_requested":
 *       console.log(`Calling ${event.name}`);
 *       break;
 *     case "turn_completed":
 *       console.log(`Tokens: ${event.usage.inputTokens} in / ${event.usage.outputTokens} out`);
 *       break;
 *   }
 * }
 * ```
 */

import type { ContentBlock, ContentInput, SkillKey } from "./types.js";

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

/** Token usage for a single LLM turn. */
export interface Usage {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly cacheCreationTokens?: number;
  readonly cacheReadTokens?: number;
}

/** Why an LLM turn ended. */
export type StopReason =
  | "end_turn"
  | "tool_use"
  | "max_tokens"
  | "stop_sequence"
  | "content_filter"
  | "cancelled";

/** Which budget dimension triggered a warning. */
export type BudgetType = "tokens" | "time" | "tool_calls";

/** Hook lifecycle points. */
export type HookPoint =
  | "run_started"
  | "run_completed"
  | "run_failed"
  | "pre_llm_request"
  | "post_llm_response"
  | "pre_tool_execution"
  | "post_tool_execution"
  | "turn_boundary";

/** Stable identifier for a configured hook. */
export type HookId = string;

/** Machine-readable agent error class. */
export type AgentErrorClass = string;

/** Structured agent error reason payload. */
export type AgentErrorReason = Record<string, unknown> & {
  readonly reason_type: string;
  readonly hook_id?: HookId | null;
};

/** Structured terminal error report emitted with failed runs. */
export interface AgentErrorReport {
  readonly class: AgentErrorClass;
  readonly message: string;
  readonly reason?: AgentErrorReason | null;
}

// ---------------------------------------------------------------------------
// Scoped streaming attribution
// ---------------------------------------------------------------------------

export type StreamScopeFrame =
  | {
      readonly scope: "primary";
      readonly session_id: string;
    }
  | {
      readonly scope: "mob_member";
      readonly flow_run_id: string;
      readonly agent_identity: string;
    };

export interface ScopedAgentEvent {
  readonly type: "scoped_agent_event";
  readonly scopeId: string;
  readonly scopePath: readonly StreamScopeFrame[];
  readonly event: AgentEvent;
}

// ---------------------------------------------------------------------------
// Session lifecycle events
// ---------------------------------------------------------------------------

export interface RunStartedEvent {
  readonly type: "run_started";
  readonly sessionId: string;
  readonly prompt: ContentInput;
}

export interface RunCompletedEvent {
  readonly type: "run_completed";
  readonly sessionId: string;
  readonly result: string;
  readonly usage: Usage;
}

export interface RunFailedEvent {
  readonly type: "run_failed";
  readonly sessionId: string;
  readonly errorClass: string;
  readonly error: string;
  readonly errorReport?: AgentErrorReport | null;
}

// ---------------------------------------------------------------------------
// Turn / LLM events
// ---------------------------------------------------------------------------

export interface TurnStartedEvent {
  readonly type: "turn_started";
  readonly turnNumber: number;
}

export interface TextDeltaEvent {
  readonly type: "text_delta";
  readonly delta: string;
}

export interface TextCompleteEvent {
  readonly type: "text_complete";
  readonly content: string;
}

export interface ToolCallRequestedEvent {
  readonly type: "tool_call_requested";
  readonly id: string;
  readonly name: string;
  readonly args: unknown;
}

export interface ToolResultReceivedEvent {
  readonly type: "tool_result_received";
  readonly id: string;
  readonly name: string;
  readonly content: readonly ContentBlock[];
  readonly isError?: boolean;
}

export interface TurnCompletedEvent {
  readonly type: "turn_completed";
  readonly stopReason?: StopReason;
  readonly usage: Usage;
}

// ---------------------------------------------------------------------------
// Tool execution events
// ---------------------------------------------------------------------------

export interface ToolExecutionStartedEvent {
  readonly type: "tool_execution_started";
  readonly id: string;
  readonly name: string;
}

export interface ToolExecutionCompletedEvent {
  readonly type: "tool_execution_completed";
  readonly id: string;
  readonly name: string;
  readonly result: string;
  readonly content: readonly ContentBlock[];
  readonly isError?: boolean;
  readonly durationMs?: number;
}

export interface ToolExecutionTimedOutEvent {
  readonly type: "tool_execution_timed_out";
  readonly id: string;
  readonly name: string;
  readonly timeoutMs: number;
}

// ---------------------------------------------------------------------------
// Compaction events
// ---------------------------------------------------------------------------

export interface CompactionStartedEvent {
  readonly type: "compaction_started";
  readonly inputTokens: number;
  readonly estimatedHistoryTokens: number;
  readonly messageCount: number;
}

export interface CompactionCompletedEvent {
  readonly type: "compaction_completed";
  readonly summaryTokens: number;
  readonly messagesBefore: number;
  readonly messagesAfter: number;
}

export interface CompactionFailedEvent {
  readonly type: "compaction_failed";
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Budget events
// ---------------------------------------------------------------------------

export interface BudgetWarningEvent {
  readonly type: "budget_warning";
  readonly budgetType: BudgetType;
  readonly used: number;
  readonly limit: number;
  readonly percent: number;
}

// ---------------------------------------------------------------------------
// Retry events
// ---------------------------------------------------------------------------

export interface RetryingEvent {
  readonly type: "retrying";
  readonly attempt: number;
  readonly maxAttempts: number;
  readonly error: string;
  readonly delayMs: number;
}

// ---------------------------------------------------------------------------
// Hook events
// ---------------------------------------------------------------------------

export interface HookStartedEvent {
  readonly type: "hook_started";
  readonly hookId: HookId;
  readonly point: HookPoint;
}

export interface HookCompletedEvent {
  readonly type: "hook_completed";
  readonly hookId: HookId;
  readonly point: HookPoint;
  readonly durationMs: number;
}

export interface HookFailedEvent {
  readonly type: "hook_failed";
  readonly hookId: HookId;
  readonly point: HookPoint;
  readonly error: string;
}

export interface HookDeniedEvent {
  readonly type: "hook_denied";
  readonly hookId: HookId;
  readonly point: HookPoint;
  readonly reasonCode: string;
  readonly message: string;
  readonly payload?: unknown;
}

export interface HookRewriteAppliedEvent {
  readonly type: "hook_rewrite_applied";
  readonly hookId: HookId;
  readonly point: HookPoint;
  readonly patch: Record<string, unknown>;
}

export interface HookPatchPublishedEvent {
  readonly type: "hook_patch_published";
  readonly hookId: HookId;
  readonly point: HookPoint;
  readonly envelope: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Skill events
// ---------------------------------------------------------------------------

export interface SkillsResolvedEvent {
  readonly type: "skills_resolved";
  readonly skills: readonly string[];
  readonly injectionBytes: number;
}

export type SkillResolutionFailureReason =
  | { readonly reasonType: "not_found"; readonly key: SkillKey }
  | { readonly reasonType: "capability_unavailable"; readonly key: SkillKey; readonly capability: string }
  | { readonly reasonType: "load"; readonly message: string }
  | { readonly reasonType: "parse"; readonly message: string }
  | {
      readonly reasonType: "source_uuid_collision";
      readonly sourceUuid: string;
      readonly existingFingerprint: string;
      readonly newFingerprint: string;
    }
  | {
      readonly reasonType: "source_uuid_mutation_without_lineage";
      readonly fingerprint: string;
      readonly existingSourceUuid: string;
      readonly mutatedSourceUuid: string;
    }
  | { readonly reasonType: "missing_skill_remaps"; readonly eventId: string; readonly eventKind: string }
  | {
      readonly reasonType: "remap_without_lineage";
      readonly fromSourceUuid: string;
      readonly fromSkillName: string;
      readonly toSourceUuid: string;
      readonly toSkillName: string;
    }
  | { readonly reasonType: "unknown_skill_alias"; readonly alias: string }
  | { readonly reasonType: "remap_cycle"; readonly sourceUuid: string; readonly skillName: string }
  | { readonly reasonType: "unknown"; readonly message: string; readonly rawReasonType?: string };

export interface SkillResolutionFailedEvent {
  readonly type: "skill_resolution_failed";
  readonly skillKey?: SkillKey;
  readonly reason?: SkillResolutionFailureReason;
  /** Legacy display mirror. Prefer `skillKey` when present. */
  readonly reference: string;
  /** Legacy display mirror. Prefer `reason` for structured handling. */
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Interaction (comms) events
// ---------------------------------------------------------------------------

export interface InteractionCompleteEvent {
  readonly type: "interaction_complete";
  readonly interactionId: string;
  readonly result: string;
}

export interface InteractionFailedEvent {
  readonly type: "interaction_failed";
  readonly interactionId: string;
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Stream management events
// ---------------------------------------------------------------------------

export interface StreamTruncatedEvent {
  readonly type: "stream_truncated";
  readonly reason: string;
}

export type ToolConfigChangeOperation = "add" | "remove" | "reload";
export type ExternalToolDeltaPhase = "pending" | "applied" | "draining" | "forced" | "failed";

export interface BoundaryAppliedToolConfigChangeStatus {
  readonly kind: "boundary_applied";
  readonly base_changed: boolean;
  readonly visible_changed: boolean;
  readonly revision: number;
}

export interface DeferredCatalogDeltaToolConfigChangeStatus {
  readonly kind: "deferred_catalog_delta";
  readonly added_hidden_count: number;
  readonly removed_hidden_count: number;
  readonly pending_source_count: number;
}

export interface WarningFailedClosedToolConfigChangeStatus {
  readonly kind: "warning_failed_closed";
  readonly error: string;
}

export interface ExternalToolDeltaToolConfigChangeStatus {
  readonly kind: "external_tool_delta";
  readonly phase: ExternalToolDeltaPhase;
  readonly detail?: string;
}

export type ToolConfigChangeStatus =
  | BoundaryAppliedToolConfigChangeStatus
  | DeferredCatalogDeltaToolConfigChangeStatus
  | WarningFailedClosedToolConfigChangeStatus
  | ExternalToolDeltaToolConfigChangeStatus;

export interface ToolConfigChangedPayload {
  readonly operation?: ToolConfigChangeOperation;
  readonly target?: string;
  readonly status?: string;
  readonly status_info?: ToolConfigChangeStatus;
  readonly persisted?: boolean;
  readonly applied_at_turn?: number;
}

export interface ToolConfigChangedEvent {
  readonly type: "tool_config_changed";
  readonly payload: ToolConfigChangedPayload;
}

// ---------------------------------------------------------------------------
// Unknown / forward-compat
// ---------------------------------------------------------------------------

export interface UnknownEvent {
  readonly type: string;
  readonly [key: string]: unknown;
}

export interface MalformedEvent {
  readonly type: "malformed_event";
  readonly rawType: string;
  readonly raw: Record<string, unknown>;
  readonly error: string;
}

// ---------------------------------------------------------------------------
// Discriminated union
// ---------------------------------------------------------------------------

/** All known agent events, discriminated on `type`. */
export type AgentEvent =
  | RunStartedEvent
  | RunCompletedEvent
  | RunFailedEvent
  | TurnStartedEvent
  | TextDeltaEvent
  | TextCompleteEvent
  | ToolCallRequestedEvent
  | ToolResultReceivedEvent
  | TurnCompletedEvent
  | ToolExecutionStartedEvent
  | ToolExecutionCompletedEvent
  | ToolExecutionTimedOutEvent
  | CompactionStartedEvent
  | CompactionCompletedEvent
  | CompactionFailedEvent
  | BudgetWarningEvent
  | RetryingEvent
  | HookStartedEvent
  | HookCompletedEvent
  | HookFailedEvent
  | HookDeniedEvent
  | HookRewriteAppliedEvent
  | HookPatchPublishedEvent
  | SkillsResolvedEvent
  | SkillResolutionFailedEvent
  | InteractionCompleteEvent
  | InteractionFailedEvent
  | StreamTruncatedEvent
  | ToolConfigChangedEvent
  | MalformedEvent
  | UnknownEvent;

/** Backward-compatible alias retained for SDK consumers. */
export type CoreAgentEvent = AgentEvent;

/** Known stream events including optional scoped wrappers. */
export type StreamEvent = AgentEvent | ScopedAgentEvent;

// ---------------------------------------------------------------------------
// Type guards for the most commonly used events
// ---------------------------------------------------------------------------

export function isTextDelta(event: StreamEvent): event is TextDeltaEvent {
  return event.type === "text_delta";
}

export function isTextComplete(event: StreamEvent): event is TextCompleteEvent {
  return event.type === "text_complete";
}

export function isTurnCompleted(event: StreamEvent): event is TurnCompletedEvent {
  return event.type === "turn_completed";
}

export function isToolCallRequested(event: StreamEvent): event is ToolCallRequestedEvent {
  return event.type === "tool_call_requested";
}

export function isRunCompleted(event: StreamEvent): event is RunCompletedEvent {
  return event.type === "run_completed";
}

export function isRunFailed(event: StreamEvent): event is RunFailedEvent {
  return event.type === "run_failed";
}

// ---------------------------------------------------------------------------
// Wire → typed parser
// ---------------------------------------------------------------------------

function isPlainRecord(raw: unknown): raw is Record<string, unknown> {
  return typeof raw === "object" && raw !== null && !Array.isArray(raw);
}

function malformedEvent(raw: Record<string, unknown>, rawType: string, error: string): MalformedEvent {
  return { type: "malformed_event", rawType, raw, error };
}

function requireStringField(raw: Record<string, unknown>, field: string): string {
  const value = raw[field];
  if (typeof value !== "string") {
    throw new Error(`${field} must be string`);
  }
  return value;
}

function requireNumberField(raw: Record<string, unknown>, field: string): number {
  const value = raw[field];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${field} must be number`);
  }
  return value;
}

function requireBooleanField(raw: Record<string, unknown>, field: string): boolean {
  const value = raw[field];
  if (typeof value !== "boolean") {
    throw new Error(`${field} must be boolean`);
  }
  return value;
}

function requireOneOf<T extends string>(
  value: string,
  field: string,
  allowed: readonly T[],
): T {
  if (!(allowed as readonly string[]).includes(value)) {
    throw new Error(`${field} must be one of ${allowed.join(", ")}`);
  }
  return value as T;
}

function parseUsage(raw: unknown): Usage {
  if (!isPlainRecord(raw)) {
    throw new Error("missing usage");
  }
  return {
    inputTokens: requireNumberField(raw, "input_tokens"),
    outputTokens: requireNumberField(raw, "output_tokens"),
    cacheCreationTokens: raw.cache_creation_tokens != null
      ? requireNumberField(raw, "cache_creation_tokens")
      : undefined,
    cacheReadTokens: raw.cache_read_tokens != null
      ? requireNumberField(raw, "cache_read_tokens")
      : undefined,
  };
}

function hasOwn(raw: Record<string, unknown>, field: string): boolean {
  return Object.prototype.hasOwnProperty.call(raw, field);
}

function parseAgentErrorReason(raw: unknown): AgentErrorReason | null | undefined {
  if (raw === undefined) {
    return undefined;
  }
  if (raw === null) {
    return null;
  }
  if (!isPlainRecord(raw)) {
    throw new Error("error_report.reason must be object");
  }

  const reasonType = requireStringField(raw, "reason_type");
  if (!["hook_denied", "hook_timeout", "hook_execution_failed"].includes(reasonType)) {
    return { ...raw, reason_type: reasonType } as AgentErrorReason;
  }

  const reason: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(raw)) {
    if (key !== "hook_id" && key !== "hook_id_string") {
      reason[key] = value;
    }
  }
  reason.reason_type = reasonType;

  if (reasonType === "hook_denied") {
    reason.point = requireStringField(raw, "point");
    reason.reason_code = requireStringField(raw, "reason_code");
  } else {
    reason.hook_id = requireStringField(raw, "hook_id");
  }

  if (reasonType === "hook_timeout") {
    reason.timeout_ms = requireNumberField(raw, "timeout_ms");
  }
  if (reasonType === "hook_execution_failed") {
    reason.reason = requireStringField(raw, "reason");
  }

  if (reasonType === "hook_denied" && hasOwn(raw, "hook_id")) {
    const hookId = raw.hook_id;
    if (hookId !== null && typeof hookId !== "string") {
      throw new Error("error_report.reason.hook_id must be string or null");
    }
    reason.hook_id = hookId;
  }

  return reason as AgentErrorReason;
}

function parseAgentErrorReport(raw: unknown): AgentErrorReport | null | undefined {
  if (raw === undefined) {
    return undefined;
  }
  if (raw === null) {
    return null;
  }
  if (!isPlainRecord(raw)) {
    throw new Error("error_report must be object");
  }

  const report: AgentErrorReport = {
    class: requireStringField(raw, "class"),
    message: requireStringField(raw, "message"),
    ...(hasOwn(raw, "reason") ? { reason: parseAgentErrorReason(raw.reason) ?? null } : {}),
  };
  return report;
}

function parseContentInput(raw: unknown): ContentInput {
  if (Array.isArray(raw)) return raw as ContentInput;
  return String(raw ?? "");
}

function parseContentBlocks(raw: unknown, legacyText?: unknown): readonly ContentBlock[] {
  if (Array.isArray(raw)) return raw as readonly ContentBlock[];
  if (raw != null) throw new Error("content must be a content block array");
  if (typeof legacyText === "string" && legacyText.length > 0) {
    return [{ type: "text", text: legacyText }];
  }
  return [];
}

function parseToolConfigChangeStatus(raw: unknown): ToolConfigChangeStatus | undefined {
  if (raw == null || typeof raw !== "object") {
    return undefined;
  }

  const value = raw as Record<string, unknown>;
  const kind = String(value.kind ?? "");
  switch (kind) {
    case "boundary_applied":
      return {
        kind,
        base_changed: typeof value.base_changed === "boolean" ? value.base_changed : false,
        visible_changed: typeof value.visible_changed === "boolean" ? value.visible_changed : false,
        revision: Number(value.revision ?? 0),
      };
    case "deferred_catalog_delta":
      return {
        kind,
        added_hidden_count: Number(value.added_hidden_count ?? 0),
        removed_hidden_count: Number(value.removed_hidden_count ?? 0),
        pending_source_count: Number(value.pending_source_count ?? 0),
      };
    case "warning_failed_closed":
      return {
        kind,
        error: String(value.error ?? ""),
      };
    case "external_tool_delta":
      return {
        kind,
        phase: String(value.phase ?? "pending") as ExternalToolDeltaPhase,
        ...(value.detail != null ? { detail: String(value.detail) } : {}),
      };
    default:
      return undefined;
  }
}

function parseSkillKey(raw: unknown): SkillKey | undefined {
  if (raw == null || typeof raw !== "object") {
    return undefined;
  }
  const value = raw as Record<string, unknown>;
  return {
    sourceUuid: String(value.source_uuid ?? value.sourceUuid ?? ""),
    skillName: String(value.skill_name ?? value.skillName ?? ""),
  };
}

function emptySkillKey(): SkillKey {
  return { sourceUuid: "", skillName: "" };
}

const STOP_REASONS = new Set<StopReason>([
  "end_turn",
  "tool_use",
  "max_tokens",
  "stop_sequence",
  "content_filter",
  "cancelled",
]);

function parseStopReason(raw: unknown): StopReason | undefined {
  return typeof raw === "string" && STOP_REASONS.has(raw as StopReason)
    ? (raw as StopReason)
    : undefined;
}

const TOOL_CONFIG_CHANGE_OPERATIONS = new Set<ToolConfigChangeOperation>([
  "add",
  "remove",
  "reload",
]);

function parseToolConfigChangeOperation(raw: unknown): ToolConfigChangeOperation | undefined {
  return typeof raw === "string" && TOOL_CONFIG_CHANGE_OPERATIONS.has(raw as ToolConfigChangeOperation)
    ? (raw as ToolConfigChangeOperation)
    : undefined;
}

function parseWireString(raw: unknown): string | undefined {
  return typeof raw === "string" ? raw : undefined;
}

function parseWireBoolean(raw: unknown): boolean | undefined {
  return typeof raw === "boolean" ? raw : undefined;
}

function parseSkillResolutionFailureReason(
  raw: unknown,
  fallbackMessage: string,
): SkillResolutionFailureReason | undefined {
  if (raw == null || typeof raw !== "object") {
    return undefined;
  }

  const value = raw as Record<string, unknown>;
  const reasonTypeRaw = value.reason_type ?? value.reasonType;
  if (typeof reasonTypeRaw !== "string") {
    return undefined;
  }
  const reasonType = reasonTypeRaw;
  switch (reasonType) {
    case "not_found":
      return { reasonType, key: parseSkillKey(value.key) ?? emptySkillKey() };
    case "capability_unavailable":
      return {
        reasonType,
        key: parseSkillKey(value.key) ?? emptySkillKey(),
        capability: String(value.capability ?? ""),
      };
    case "load":
    case "parse":
      return { reasonType, message: String(value.message ?? "") };
    case "source_uuid_collision":
      return {
        reasonType,
        sourceUuid: String(value.source_uuid ?? value.sourceUuid ?? ""),
        existingFingerprint: String(value.existing_fingerprint ?? value.existingFingerprint ?? ""),
        newFingerprint: String(value.new_fingerprint ?? value.newFingerprint ?? ""),
      };
    case "source_uuid_mutation_without_lineage":
      return {
        reasonType,
        fingerprint: String(value.fingerprint ?? ""),
        existingSourceUuid: String(value.existing_source_uuid ?? value.existingSourceUuid ?? ""),
        mutatedSourceUuid: String(value.mutated_source_uuid ?? value.mutatedSourceUuid ?? ""),
      };
    case "missing_skill_remaps":
      return {
        reasonType,
        eventId: String(value.event_id ?? value.eventId ?? ""),
        eventKind: String(value.event_kind ?? value.eventKind ?? ""),
      };
    case "remap_without_lineage":
      return {
        reasonType,
        fromSourceUuid: String(value.from_source_uuid ?? value.fromSourceUuid ?? ""),
        fromSkillName: String(value.from_skill_name ?? value.fromSkillName ?? ""),
        toSourceUuid: String(value.to_source_uuid ?? value.toSourceUuid ?? ""),
        toSkillName: String(value.to_skill_name ?? value.toSkillName ?? ""),
      };
    case "unknown_skill_alias":
      return { reasonType, alias: String(value.alias ?? "") };
    case "remap_cycle":
      return {
        reasonType,
        sourceUuid: String(value.source_uuid ?? value.sourceUuid ?? ""),
        skillName: String(value.skill_name ?? value.skillName ?? ""),
      };
    case "unknown":
      return { reasonType, message: String(value.message ?? fallbackMessage) };
    default:
      return {
        reasonType: "unknown",
        message: String(value.message ?? fallbackMessage),
        rawReasonType: reasonType,
      };
  }
}

/**
 * Parse a raw wire event dict into a typed {@link StreamEvent}.
 *
 * Unknown event types are returned as {@link UnknownEvent} for
 * forward-compatibility.
 */
export function parseEvent(raw: Record<string, unknown>): StreamEvent {
  const normalizeScopePath = (value: unknown): StreamScopeFrame[] => {
    if (!Array.isArray(value)) {
      return [];
    }
    return value.map((entry) => {
      if (!entry || typeof entry !== "object") {
        return { scope: "primary", session_id: "" } as StreamScopeFrame;
      }
      const frame = entry as Record<string, unknown>;
      if (frame.scope === "mob_member") {
        return {
          scope: "mob_member",
          flow_run_id: String(frame.flow_run_id ?? ""),
          agent_identity: String(frame.agent_identity ?? ""),
        } as StreamScopeFrame;
      }
      return {
        scope: "primary",
        session_id: String(frame.session_id ?? ""),
      } as StreamScopeFrame;
    });
  };

  if (
    raw.event &&
    (typeof raw.scope_id === "string" || Array.isArray(raw.scope_path))
  ) {
    const innerRaw = typeof raw.event === "object" && raw.event !== null
      ? (raw.event as Record<string, unknown>)
      : { type: "unknown" };
    return {
      type: "scoped_agent_event",
      scopeId: String(raw.scope_id ?? ""),
      scopePath: normalizeScopePath(raw.scope_path),
      event: parseCoreEvent(innerRaw),
    };
  }

  return parseCoreEvent(raw);
}

/** Parse a raw wire event as a core (non-wrapper) agent event. */
export function parseCoreEvent(raw: Record<string, unknown>): AgentEvent {
  const type = String(raw.type ?? "");

  try {
  switch (type) {
    // Session lifecycle
    case "run_started":
      return { type, sessionId: requireStringField(raw, "session_id"), prompt: parseContentInput(raw.prompt) };
    case "run_completed":
      return { type, sessionId: requireStringField(raw, "session_id"), result: requireStringField(raw, "result"), usage: parseUsage(raw.usage) };
    case "run_failed":
      return {
        type,
        sessionId: requireStringField(raw, "session_id"),
        errorClass: requireStringField(raw, "error_class"),
        error: requireStringField(raw, "error"),
        ...(hasOwn(raw, "error_report") ? { errorReport: parseAgentErrorReport(raw.error_report) ?? null } : {}),
      };

    // Turn / LLM
    case "turn_started":
      return { type, turnNumber: requireNumberField(raw, "turn_number") };
    case "text_delta":
      return { type, delta: requireStringField(raw, "delta") };
    case "text_complete":
      return { type, content: requireStringField(raw, "content") };
    case "tool_call_requested":
      return { type, id: requireStringField(raw, "id"), name: requireStringField(raw, "name"), args: raw.args };
    case "tool_result_received":
      return {
        type,
        id: requireStringField(raw, "id"),
        name: requireStringField(raw, "name"),
        content: parseContentBlocks(raw.content),
        isError: requireBooleanField(raw, "is_error"),
      };
    case "turn_completed":
      return {
        type,
        stopReason: requireOneOf(
          requireStringField(raw, "stop_reason"),
          "stop_reason",
          ["end_turn", "tool_use", "max_tokens", "stop_sequence", "content_filter", "cancelled"] as const,
        ),
        usage: parseUsage(raw.usage),
      };

    // Tool execution
    case "tool_execution_started":
      return { type, id: requireStringField(raw, "id"), name: requireStringField(raw, "name") };
    case "tool_execution_completed":
      return {
        type,
        id: requireStringField(raw, "id"),
        name: requireStringField(raw, "name"),
        result: requireStringField(raw, "result"),
        content: parseContentBlocks(raw.content, raw.result),
        ...(parseWireBoolean(raw.is_error) != null ? { isError: parseWireBoolean(raw.is_error) } : {}),
        ...(typeof raw.duration_ms === "number" && Number.isFinite(raw.duration_ms)
          ? { durationMs: raw.duration_ms }
          : {}),
      };
    case "tool_execution_timed_out":
      return { type, id: requireStringField(raw, "id"), name: requireStringField(raw, "name"), timeoutMs: requireNumberField(raw, "timeout_ms") };

    // Compaction
    case "compaction_started":
      return { type, inputTokens: requireNumberField(raw, "input_tokens"), estimatedHistoryTokens: requireNumberField(raw, "estimated_history_tokens"), messageCount: requireNumberField(raw, "message_count") };
    case "compaction_completed":
      return { type, summaryTokens: requireNumberField(raw, "summary_tokens"), messagesBefore: requireNumberField(raw, "messages_before"), messagesAfter: requireNumberField(raw, "messages_after") };
    case "compaction_failed":
      return { type, error: requireStringField(raw, "error") };

    // Budget
    case "budget_warning":
      return { type, budgetType: requireOneOf(requireStringField(raw, "budget_type"), "budget_type", ["tokens", "time", "tool_calls"] as const), used: requireNumberField(raw, "used"), limit: requireNumberField(raw, "limit"), percent: requireNumberField(raw, "percent") };

    // Retry
    case "retrying":
      return { type, attempt: requireNumberField(raw, "attempt"), maxAttempts: requireNumberField(raw, "max_attempts"), error: requireStringField(raw, "error"), delayMs: requireNumberField(raw, "delay_ms") };

    // Hooks
    case "hook_started":
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint };
    case "hook_completed":
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint, durationMs: requireNumberField(raw, "duration_ms") };
    case "hook_failed":
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint, error: requireStringField(raw, "error") };
    case "hook_denied":
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint, reasonCode: requireStringField(raw, "reason_code"), message: requireStringField(raw, "message"), ...(raw.payload != null ? { payload: raw.payload } : {}) };
    case "hook_rewrite_applied":
      if (!isPlainRecord(raw.patch)) throw new Error("patch must be object");
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint, patch: raw.patch };
    case "hook_patch_published":
      if (!isPlainRecord(raw.envelope)) throw new Error("envelope must be object");
      return { type, hookId: requireStringField(raw, "hook_id"), point: requireStringField(raw, "point") as HookPoint, envelope: raw.envelope };

    // Skills
    case "skills_resolved":
      if (!Array.isArray(raw.skills) || !raw.skills.every((skill) => typeof skill === "string")) {
        throw new Error("skills must be string array");
      }
      return { type, skills: raw.skills, injectionBytes: requireNumberField(raw, "injection_bytes") };
    case "skill_resolution_failed": {
      const reference = requireStringField(raw, "reference");
      const error = requireStringField(raw, "error");
      const skillKey = parseSkillKey(raw.skill_key);
      const reason = parseSkillResolutionFailureReason(raw.reason, error);
      return {
        type,
        ...(skillKey ? { skillKey } : {}),
        ...(reason ? { reason } : {}),
        reference,
        error,
      };
    }

    // Interaction (comms)
    case "interaction_complete":
      return { type, interactionId: requireStringField(raw, "interaction_id"), result: requireStringField(raw, "result") };
    case "interaction_failed":
      return { type, interactionId: requireStringField(raw, "interaction_id"), error: requireStringField(raw, "error") };

    // Stream management
    case "stream_truncated":
      return { type, reason: requireStringField(raw, "reason") };
    case "tool_config_changed": {
      if (!isPlainRecord(raw.payload)) throw new Error("payload must be object");
      const payloadRaw = raw.payload;
      const appliedAtTurnRaw = payloadRaw.applied_at_turn;
      const appliedAtTurn = typeof appliedAtTurnRaw === "number" && Number.isFinite(appliedAtTurnRaw)
        ? appliedAtTurnRaw
        : undefined;
      const statusInfo = parseToolConfigChangeStatus(payloadRaw?.status_info);
      return {
        type,
        payload: {
          ...(statusInfo != null ? { status_info: statusInfo } : {}),
          operation: requireOneOf(requireStringField(payloadRaw, "operation"), "operation", ["add", "remove", "reload"] as const),
          target: requireStringField(payloadRaw, "target"),
          status: requireStringField(payloadRaw, "status"),
          persisted: requireBooleanField(payloadRaw, "persisted"),
          ...(appliedAtTurn != null ? { applied_at_turn: appliedAtTurn } : {}),
        },
      };
    }

    // Unknown — forward-compat
    default:
      return { type, ...raw } as UnknownEvent;
  }
  } catch (error) {
    return malformedEvent(
      raw,
      type,
      error instanceof Error ? error.message : String(error),
    );
  }
}
