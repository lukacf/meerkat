/**
 * Typed event hierarchy for the Meerkat streaming API.
 *
 * Events form a discriminated union on the `type` field (snake_case to match
 * the wire protocol).  All other fields use idiomatic camelCase.
 * Missing semantic fields remain absent during parsing so partial streaming
 * payloads do not become authoritative SDK state.
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

import { Buffer } from "node:buffer";
import type { ContentInput, SkillKey } from "./types.js";

// ---------------------------------------------------------------------------
// Shared value types
// ---------------------------------------------------------------------------

/**
 * Re-encode the wire `"identity:generation"` display string that appears
 * on event scope frames into the canonical opaque `AgentRuntimeRef`
 * token. Returns `undefined` for absent values; returns `""` for
 * malformed values (unlike the object form in `client.ts` which throws —
 * events flow through many code paths and softer degradation is
 * acceptable for frame metadata).
 */
function encodeAgentRuntimeRefFromDisplay(raw: unknown): string | undefined {
  if (raw == null) {
    return undefined;
  }
  const display = String(raw);
  const sep = display.lastIndexOf(":");
  if (sep <= 0 || sep >= display.length - 1) {
    return "";
  }
  const identity = display.slice(0, sep);
  const generationStr = display.slice(sep + 1);
  const generation = Number(generationStr);
  if (!identity || !Number.isFinite(generation) || !Number.isInteger(generation)) {
    return "";
  }
  const payload = JSON.stringify({ i: identity, g: generation });
  const b64 = Buffer.from(payload, "utf-8").toString("base64");
  return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

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
      /**
       * Opaque incarnation handle. Compare for equality to detect
       * incarnation rotation; internals are not parseable. Encoded
       * client-side from the wire `"identity:generation"` display string,
       * matching the `MemberRef` pattern.
       */
      readonly agent_runtime_id?: string;
      readonly fence_token?: number;
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
  readonly isError?: boolean;
  readonly durationMs: number;
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
  readonly hookId: string;
  readonly point: HookPoint;
}

export interface HookCompletedEvent {
  readonly type: "hook_completed";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly durationMs: number;
}

export interface HookFailedEvent {
  readonly type: "hook_failed";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly error: string;
}

export interface HookDeniedEvent {
  readonly type: "hook_denied";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly reasonCode: string;
  readonly message: string;
  readonly payload?: unknown;
}

export interface HookRewriteAppliedEvent {
  readonly type: "hook_rewrite_applied";
  readonly hookId: string;
  readonly point: HookPoint;
  readonly patch: Record<string, unknown>;
}

export interface HookPatchPublishedEvent {
  readonly type: "hook_patch_published";
  readonly hookId: string;
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

export interface ToolConfigChangedPayload {
  readonly operation?: ToolConfigChangeOperation;
  readonly target?: string;
  readonly status?: string;
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

function parseUsage(raw: Record<string, unknown> | undefined): Usage {
  if (!raw) return { inputTokens: 0, outputTokens: 0 };
  return {
    inputTokens: Number(raw.input_tokens ?? 0),
    outputTokens: Number(raw.output_tokens ?? 0),
    cacheCreationTokens: raw.cache_creation_tokens != null
      ? Number(raw.cache_creation_tokens)
      : undefined,
    cacheReadTokens: raw.cache_read_tokens != null
      ? Number(raw.cache_read_tokens)
      : undefined,
  };
}

function parseContentInput(raw: unknown): ContentInput {
  if (Array.isArray(raw)) return raw as ContentInput;
  return String(raw ?? "");
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
          agent_runtime_id: encodeAgentRuntimeRefFromDisplay(frame.agent_runtime_id),
          fence_token:
            typeof frame.fence_token === "number" ? frame.fence_token : undefined,
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

  switch (type) {
    // Session lifecycle
    case "run_started":
      return { type, sessionId: String(raw.session_id ?? ""), prompt: parseContentInput(raw.prompt) };
    case "run_completed":
      return { type, sessionId: String(raw.session_id ?? ""), result: String(raw.result ?? ""), usage: parseUsage(raw.usage as Record<string, unknown>) };
    case "run_failed":
      return { type, sessionId: String(raw.session_id ?? ""), errorClass: String(raw.error_class ?? ""), error: String(raw.error ?? "") };

    // Turn / LLM
    case "turn_started":
      return { type, turnNumber: Number(raw.turn_number ?? 0) };
    case "text_delta":
      return { type, delta: String(raw.delta ?? "") };
    case "text_complete":
      return { type, content: String(raw.content ?? "") };
    case "tool_call_requested":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), args: raw.args };
    case "tool_result_received": {
      const isError = parseWireBoolean(raw.is_error);
      return {
        type,
        id: String(raw.id ?? ""),
        name: String(raw.name ?? ""),
        ...(isError != null ? { isError } : {}),
      };
    }
    case "turn_completed": {
      const stopReason = parseStopReason(raw.stop_reason);
      return {
        type,
        ...(stopReason != null ? { stopReason } : {}),
        usage: parseUsage(raw.usage as Record<string, unknown>),
      };
    }

    // Tool execution
    case "tool_execution_started":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? "") };
    case "tool_execution_completed": {
      const isError = parseWireBoolean(raw.is_error);
      return {
        type,
        id: String(raw.id ?? ""),
        name: String(raw.name ?? ""),
        result: String(raw.result ?? ""),
        ...(isError != null ? { isError } : {}),
        durationMs: Number(raw.duration_ms ?? 0),
      };
    }
    case "tool_execution_timed_out":
      return { type, id: String(raw.id ?? ""), name: String(raw.name ?? ""), timeoutMs: Number(raw.timeout_ms ?? 0) };

    // Compaction
    case "compaction_started":
      return { type, inputTokens: Number(raw.input_tokens ?? 0), estimatedHistoryTokens: Number(raw.estimated_history_tokens ?? 0), messageCount: Number(raw.message_count ?? 0) };
    case "compaction_completed":
      return { type, summaryTokens: Number(raw.summary_tokens ?? 0), messagesBefore: Number(raw.messages_before ?? 0), messagesAfter: Number(raw.messages_after ?? 0) };
    case "compaction_failed":
      return { type, error: String(raw.error ?? "") };

    // Budget
    case "budget_warning":
      return { type, budgetType: String(raw.budget_type ?? "") as BudgetType, used: Number(raw.used ?? 0), limit: Number(raw.limit ?? 0), percent: Number(raw.percent ?? 0) };

    // Retry
    case "retrying":
      return { type, attempt: Number(raw.attempt ?? 0), maxAttempts: Number(raw.max_attempts ?? 0), error: String(raw.error ?? ""), delayMs: Number(raw.delay_ms ?? 0) };

    // Hooks
    case "hook_started":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint };
    case "hook_completed":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, durationMs: Number(raw.duration_ms ?? 0) };
    case "hook_failed":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, error: String(raw.error ?? "") };
    case "hook_denied":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, reasonCode: String(raw.reason_code ?? ""), message: String(raw.message ?? ""), ...(raw.payload != null ? { payload: raw.payload } : {}) };
    case "hook_rewrite_applied":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, patch: (raw.patch ?? {}) as Record<string, unknown> };
    case "hook_patch_published":
      return { type, hookId: String(raw.hook_id ?? ""), point: String(raw.point ?? "") as HookPoint, envelope: (raw.envelope ?? {}) as Record<string, unknown> };

    // Skills
    case "skills_resolved":
      return { type, skills: (raw.skills ?? []) as string[], injectionBytes: Number(raw.injection_bytes ?? 0) };
    case "skill_resolution_failed": {
      const error = String(raw.error ?? "");
      const skillKey = parseSkillKey(raw.skill_key);
      const reason = parseSkillResolutionFailureReason(raw.reason, error);
      return {
        type,
        ...(skillKey ? { skillKey } : {}),
        ...(reason ? { reason } : {}),
        reference: String(raw.reference ?? ""),
        error,
      };
    }

    // Interaction (comms)
    case "interaction_complete":
      return { type, interactionId: String(raw.interaction_id ?? ""), result: String(raw.result ?? "") };
    case "interaction_failed":
      return { type, interactionId: String(raw.interaction_id ?? ""), error: String(raw.error ?? "") };

    // Stream management
    case "stream_truncated":
      return { type, reason: String(raw.reason ?? "") };
    case "tool_config_changed": {
      const payloadRaw = typeof raw.payload === "object" && raw.payload !== null
        ? (raw.payload as Record<string, unknown>)
        : undefined;
      const appliedAtTurnRaw = payloadRaw?.applied_at_turn;
      const appliedAtTurn = typeof appliedAtTurnRaw === "number" && Number.isFinite(appliedAtTurnRaw)
        ? appliedAtTurnRaw
        : undefined;
      const operation = parseToolConfigChangeOperation(payloadRaw?.operation);
      const target = parseWireString(payloadRaw?.target);
      const status = parseWireString(payloadRaw?.status);
      const persisted = parseWireBoolean(payloadRaw?.persisted);
      return {
        type,
        payload: {
          ...(operation != null ? { operation } : {}),
          ...(target != null ? { target } : {}),
          ...(status != null ? { status } : {}),
          ...(persisted != null ? { persisted } : {}),
          ...(appliedAtTurn != null ? { applied_at_turn: appliedAtTurn } : {}),
        },
      };
    }

    // Unknown — forward-compat
    default:
      return { type, ...raw } as UnknownEvent;
  }
}
