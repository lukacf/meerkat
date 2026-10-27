import type { ConversationRichToolCallBlock } from "./rich-content";

export type ExperienceSectionRefresh =
  | { mode: "poll"; interval_ms: number }
  | { mode: "stream"; topic: string; update_semantics: "full_snapshot" | "append" };

export interface ExperienceSectionMeta {
  schema_version: string;
  refresh: ExperienceSectionRefresh;
  capabilities?: string[];
}

/**
 * Machine-owned liveness projection (meerkat 0.7.29+). All string
 * vocabularies are open — tolerate values you don't recognize.
 * `run_state`: idle | run_open | unknown; `health`: healthy | degraded |
 * wedged | unknown; `last_progress_event`: execution_advanced | became_idle
 * | unchanged.
 */
export interface MemberProgress {
  run_state: string;
  in_flight_work: number;
  last_progress_at_ms: number;
  last_progress_event: string;
  health: string;
}

export function normalizeMemberProgress(value: unknown): MemberProgress | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  return {
    run_state: typeof record.run_state === "string" && record.run_state ? record.run_state : "unknown",
    in_flight_work: typeof record.in_flight_work === "number" && Number.isFinite(record.in_flight_work) ? record.in_flight_work : 0,
    last_progress_at_ms: typeof record.last_progress_at_ms === "number" && Number.isFinite(record.last_progress_at_ms) ? record.last_progress_at_ms : 0,
    last_progress_event: typeof record.last_progress_event === "string" && record.last_progress_event ? record.last_progress_event : "unchanged",
    health: typeof record.health === "string" && record.health ? record.health : "unknown",
  };
}

/// Meta's typed "session needs repair" hold (`session_repair` on the
/// identity status and inspect payloads): the member's durable session is
/// intact but refused until the operator runs the two `rkat` commands and
/// then `meta/reload_member`. Present only while the hold stands.
export interface IdentitySessionRepair {
  session_id: string;
  hold: string;
  diagnose_command: string;
  apply_command: string;
  detail: string;
}

export const IDENTITY_STATE_NEEDS_REPAIR_LABEL = "needs repair";

/// The state label the console shows for a member: the typed repair hold
/// wins over the raw lifecycle state, so a parked-for-repair member reads as
/// repairable rather than failed.
export function identityStateLabel(row: { state: string; session_repair?: IdentitySessionRepair | null }): string {
  return row.session_repair ? IDENTITY_STATE_NEEDS_REPAIR_LABEL : row.state;
}

/**
 * Why facts of a console record are unobserved. The vocabulary is open:
 * tolerate values you don't recognize. `refresh_pending`: the identity
 * binding behind the record moved and the projection had not re-collected
 * it yet, so the record withholds the facts it walked before.
 * `not_observed`: nothing has observed the fact yet, so there is no answer
 * to report.
 */
export type UnobservedReason = "refresh_pending" | "not_observed" | (string & {});

/**
 * Facts a console record holds no current observation of. `fields` names
 * them; a named field is absent, empty, or the vocabulary's "unknown", never
 * a guessed value.
 */
export interface UnobservedFacts {
  reason: UnobservedReason;
  fields: string[];
}

export interface IdentityStatusRow {
  identity: string;
  display_name?: string;
  role?: string;
  state: string;
  addressability: "addressable" | "internal_only";
  labels: Record<string, string>;
  generation?: number;
  progress?: MemberProgress;
  session_repair?: IdentitySessionRepair | null;
  unobserved?: UnobservedFacts;
}

/// The identity inspect view. `continuity` carries Core's facts for the
/// identity: its member generation and the session and runtime Core binds.
export interface IdentityInspectViewState extends IdentityStatusRow {
  continuity: {
    generation?: number;
    session_id?: string;
    agent_runtime_id?: string;
  };
  output_preview?: string | null;
  is_final?: boolean | null;
  peer_reachable_count?: number | null;
  topology_peers?: string[];
  recent_tool_calls?: ConversationRichToolCallBlock[];
  last_activity_ms?: number | null;
}

export interface SidebarWatchFields {
  watched?: boolean;
  alertLevel?: "elevated" | "critical" | null;
  degraded?: boolean;
  degradedReason?: string;
}

export interface ActivityFilterPreset {
  id: string;
  label: string;
  watchedOnly?: boolean;
  alertLevels?: Array<"elevated" | "critical">;
  eventTypeFilter?: string[];
}

export type ResponsePhase = "waiting" | "tool-executing" | "generating" | null;

export interface GatingActionResult {
  pending_id: string;
  action_id: string;
  approver_id: string;
  decision: "approve" | "reject" | "escalate";
  outcome: "allowed" | "safe_draft" | "pending_approval";
  decided_at_ms: number;
  reason?: string;
  next_pending_id?: string;
}

/** Display projection of the current gating owner's records, not execution state. */
export interface GatingAuditEntryView {
  auditId?: string;
  pendingId?: string;
  actionId?: string;
  eventType?: string;
  actorId?: string;
  riskTier: "r0" | "r1" | "r2" | "r3" | "unknown";
  recordedRiskTier?: string;
  outcome?: string;
  decision?: string;
  approverId?: string;
  automaticAllowance: boolean;
}

export interface RoutingSectionView {
  routes: Array<{
    route_key: string;
    recipient: string;
    channel?: string;
    sink: string;
    target_module: string;
    retry_max?: number;
    backoff_ms?: number;
    rate_limit_per_minute?: number;
  }>;
  deliveries: Array<{
    delivery_id: string;
    route_id: string;
    recipient: string;
    sink: string;
    target_module: string;
    status: string;
    first_attempt_ms: number;
    final_attempt_ms: number;
    idempotency_key?: string;
    sink_adapter?: string;
    attempts: Array<{
      attempt: number;
      status: string;
      backoff_ms: number;
    }>;
  }>;
}

export interface GatingActionRequest {
  pending_id: string;
  approver_id: string;
  decision: "approve" | "reject" | "escalate";
  reason?: string;
}

export interface ReplayUnavailableError {
  error: "replay_unavailable";
  stream: "identity" | "all_events" | "timeline";
  requested_last_event_id: string;
  latest_event_id: string;
}

export interface ConsoleInteractionRejectedError {
  code: -32001 | -32002 | -32003 | -32004 | -32602 | -32603;
  message: string;
}

export interface ToolCallAccumulatorState {
  toolCalls: Record<string, ConversationRichToolCallBlock>;
  pendingResults: Record<string, string>;
  timeoutMs: number;
}

function trimString(value: unknown): string | undefined {
  if (typeof value !== "string") {
    return undefined;
  }
  const trimmed = value.trim();
  return trimmed || undefined;
}

function stringRecord(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object") {
    return {};
  }

  return Object.fromEntries(
    Object.entries(value as Record<string, unknown>)
      .map(([key, raw]) => {
        const normalizedKey = trimString(key);
        const normalizedValue = trimString(raw);
        return normalizedKey && normalizedValue ? [normalizedKey, normalizedValue] : null;
      })
      .filter((entry): entry is [string, string] => Boolean(entry)),
  );
}

export function normalizeResponsePhase(value: unknown): ResponsePhase {
  switch (value) {
    case "waiting":
    case "tool-executing":
    case "generating":
      return value;
    case null:
    case undefined:
      return null;
    default:
      return null;
  }
}

function normalizeFiniteNumber(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function normalizeStringArray(value: unknown): string[] | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const normalized = Array.from(new Set(value.map(trimString).filter((entry): entry is string => Boolean(entry))));
  return normalized.length > 0 ? normalized : undefined;
}

export function normalizeSidebarWatchFields(value: unknown): SidebarWatchFields {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const normalized: SidebarWatchFields = {};

  if (typeof record.watched === "boolean") {
    normalized.watched = record.watched;
  }
  if (record.alertLevel === "elevated" || record.alertLevel === "critical" || record.alertLevel === null) {
    normalized.alertLevel = record.alertLevel;
  }
  if (typeof record.degraded === "boolean") {
    normalized.degraded = record.degraded;
  }

  const degradedReason = trimString(record.degradedReason);
  if (degradedReason) {
    normalized.degradedReason = degradedReason;
  }

  return normalized;
}

export function normalizeActivityFilterPreset(value: unknown): ActivityFilterPreset | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }

  const id = trimString(record.id);
  const label = trimString(record.label);
  if (!id || !label) {
    return null;
  }

  const alertLevels = Array.isArray(record.alertLevels)
    ? Array.from(new Set(record.alertLevels.filter((level): level is "elevated" | "critical" =>
      level === "elevated" || level === "critical")))
    : undefined;
  const eventTypeFilter = Array.isArray(record.eventTypeFilter)
    ? Array.from(new Set(record.eventTypeFilter.map(trimString).filter((entry): entry is string => Boolean(entry))))
    : undefined;

  return {
    id,
    label,
    ...(typeof record.watchedOnly === "boolean" ? { watchedOnly: record.watchedOnly } : {}),
    ...(alertLevels?.length ? { alertLevels } : {}),
    ...(eventTypeFilter?.length ? { eventTypeFilter } : {}),
  };
}

export function normalizeExperienceSectionMeta(value: unknown): ExperienceSectionMeta | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }

  const schemaVersion = trimString(record.schema_version);
  const refresh = record.refresh && typeof record.refresh === "object"
    ? record.refresh as Record<string, unknown>
    : null;
  if (!schemaVersion || !refresh) {
    return null;
  }

  if (refresh.mode === "poll" && typeof refresh.interval_ms === "number" && Number.isFinite(refresh.interval_ms) && refresh.interval_ms > 0) {
    const capabilities = Array.isArray(record.capabilities)
      ? Array.from(new Set(record.capabilities.map(trimString).filter((entry): entry is string => Boolean(entry))))
      : undefined;
    return {
      schema_version: schemaVersion,
      refresh: { mode: "poll", interval_ms: refresh.interval_ms },
      ...(capabilities?.length ? { capabilities } : {}),
    };
  }

  if (
    refresh.mode === "stream"
    && (refresh.update_semantics === "full_snapshot" || refresh.update_semantics === "append")
  ) {
    const topic = trimString(refresh.topic);
    if (!topic) {
      return null;
    }
    const capabilities = Array.isArray(record.capabilities)
      ? Array.from(new Set(record.capabilities.map(trimString).filter((entry): entry is string => Boolean(entry))))
      : undefined;
    return {
      schema_version: schemaVersion,
      refresh: {
        mode: "stream",
        topic,
        update_semantics: refresh.update_semantics,
      },
      ...(capabilities?.length ? { capabilities } : {}),
    };
  }

  return null;
}

export function normalizeIdentitySessionRepair(value: unknown): IdentitySessionRepair | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const session_id = trimString(record.session_id);
  const hold = trimString(record.hold);
  const diagnose_command = trimString(record.diagnose_command);
  const apply_command = trimString(record.apply_command);
  if (!session_id || !hold || !diagnose_command || !apply_command) {
    return null;
  }
  return {
    session_id,
    hold,
    diagnose_command,
    apply_command,
    detail: trimString(record.detail) ?? "",
  };
}

export function normalizeIdentityStatusRow(value: unknown): IdentityStatusRow | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }

  const identity = trimString(record.identity);
  const state = trimString(record.state);
  if (!identity || !state) {
    return null;
  }

  const addressability = record.addressability === "internal_only" ? "internal_only" : record.addressability === "addressable" ? "addressable" : null;
  if (!addressability) {
    return null;
  }

  return {
    identity,
    state,
    addressability,
    labels: stringRecord(record.labels),
    ...(trimString(record.display_name) ? { display_name: trimString(record.display_name) } : {}),
    ...(trimString(record.role) ? { role: trimString(record.role) } : {}),
    ...(typeof record.generation === "number" && Number.isFinite(record.generation) ? { generation: record.generation } : {}),
    ...((): { progress?: MemberProgress } => {
      const progress = normalizeMemberProgress(record.progress);
      return progress ? { progress } : {};
    })(),
    ...((): { session_repair?: IdentitySessionRepair } => {
      const session_repair = normalizeIdentitySessionRepair(record.session_repair);
      return session_repair ? { session_repair } : {};
    })(),
    ...((): { unobserved?: UnobservedFacts } => {
      const unobserved = normalizeUnobservedFacts(record.unobserved);
      return unobserved ? { unobserved } : {};
    })(),
  };
}

export function normalizeUnobservedFacts(value: unknown): UnobservedFacts | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const reason = trimString(record.reason);
  if (!reason) {
    return null;
  }
  const fields = Array.isArray(record.fields)
    ? record.fields
      .map((field) => trimString(field))
      .filter((field): field is string => Boolean(field))
    : [];
  return { reason, fields };
}

/** The mob's run state as the console shows it. */
export type ConsoleMobStatus = "running" | "stopped" | "unknown";

/**
 * Whether the mob runs, as a health snapshot reports it. Only an observed
 * `true` is running and only an observed `false` is stopped: `null`, an
 * absent value, or an `unobserved` marker that names `running` is unknown,
 * never one of the two.
 */
export function consoleMobStatus(snapshot: unknown): ConsoleMobStatus {
  const record = snapshot && typeof snapshot === "object" ? snapshot as Record<string, unknown> : null;
  if (!record) {
    return "unknown";
  }
  if (normalizeUnobservedFacts(record.unobserved)?.fields.includes("running")) {
    return "unknown";
  }
  if (record.running === true) {
    return "running";
  }
  if (record.running === false) {
    return "stopped";
  }
  return "unknown";
}

export function normalizeIdentityInspectViewState(value: unknown): IdentityInspectViewState | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  const statusRow = normalizeIdentityStatusRow(value);
  if (!record || !statusRow) {
    return null;
  }

  const continuityRecord = record.continuity && typeof record.continuity === "object"
    ? record.continuity as Record<string, unknown>
    : {};

  return {
    ...statusRow,
    continuity: {
      ...(normalizeFiniteNumber(continuityRecord.generation) !== undefined
        ? { generation: normalizeFiniteNumber(continuityRecord.generation) }
        : {}),
      ...(trimString(continuityRecord.session_id) ? { session_id: trimString(continuityRecord.session_id) } : {}),
      ...(trimString(continuityRecord.agent_runtime_id) ? { agent_runtime_id: trimString(continuityRecord.agent_runtime_id) } : {}),
    },
    ...(trimString(record.output_preview) !== undefined ? { output_preview: trimString(record.output_preview) ?? null } : {}),
    ...(typeof record.is_final === "boolean" || record.is_final === null ? { is_final: record.is_final as boolean | null } : {}),
    ...(normalizeFiniteNumber(record.peer_reachable_count) !== undefined
      ? { peer_reachable_count: normalizeFiniteNumber(record.peer_reachable_count) }
      : record.peer_reachable_count === null
        ? { peer_reachable_count: null }
        : {}),
    ...(normalizeStringArray(record.topology_peers) ? { topology_peers: normalizeStringArray(record.topology_peers) } : {}),
    ...(Array.isArray(record.recent_tool_calls) ? { recent_tool_calls: record.recent_tool_calls as ConversationRichToolCallBlock[] } : {}),
    ...(normalizeFiniteNumber(record.last_activity_ms) !== undefined
      ? { last_activity_ms: normalizeFiniteNumber(record.last_activity_ms) }
      : record.last_activity_ms === null
        ? { last_activity_ms: null }
        : {}),
  };
}

export function normalizeGatingAuditEntry(value: unknown): GatingAuditEntryView {
  const record = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown> : {};
  const detail = record.detail && typeof record.detail === "object" && !Array.isArray(record.detail)
    ? record.detail as Record<string, unknown> : {};
  const tier = record.risk_tier;
  return {
    auditId: trimString(record.audit_id),
    pendingId: trimString(record.pending_id),
    actionId: trimString(record.action_id),
    eventType: trimString(record.event_type),
    actorId: trimString(record.actor_id),
    riskTier: tier === "r0" || tier === "r1" || tier === "r2" || tier === "r3" ? tier : "unknown",
    recordedRiskTier: trimString(tier),
    outcome: trimString(record.outcome),
    decision: trimString(detail.decision),
    approverId: trimString(detail.approver_id),
    automaticAllowance: record.event_type === "evaluated"
      && (record.outcome === "allowed" || record.outcome === "allowed_with_audit"),
  };
}

export function normalizeGatingActionRequest(value: unknown): GatingActionRequest | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const pendingId = trimString(record.pending_id);
  const approverId = trimString(record.approver_id);
  if (!pendingId || !approverId) {
    return null;
  }
  if (record.decision !== "approve" && record.decision !== "reject" && record.decision !== "escalate") {
    return null;
  }
  return {
    pending_id: pendingId,
    approver_id: approverId,
    decision: record.decision,
    ...(trimString(record.reason) ? { reason: trimString(record.reason) } : {}),
  };
}

export function normalizeGatingActionResult(value: unknown): GatingActionResult | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const pendingId = trimString(record.pending_id);
  const actionId = trimString(record.action_id);
  const approverId = trimString(record.approver_id);
  const decidedAt = normalizeFiniteNumber(record.decided_at_ms);
  if (!pendingId || !actionId || !approverId || decidedAt === undefined) {
    return null;
  }
  if (record.decision !== "approve" && record.decision !== "reject" && record.decision !== "escalate") {
    return null;
  }
  if (record.outcome !== "allowed" && record.outcome !== "safe_draft" && record.outcome !== "pending_approval") {
    return null;
  }
  return {
    pending_id: pendingId,
    action_id: actionId,
    approver_id: approverId,
    decision: record.decision,
    outcome: record.outcome,
    decided_at_ms: decidedAt,
    ...(trimString(record.reason) ? { reason: trimString(record.reason) } : {}),
    ...(trimString(record.next_pending_id) ? { next_pending_id: trimString(record.next_pending_id) } : {}),
  };
}

export function normalizeRoutingSectionView(value: unknown): RoutingSectionView | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const routes = Array.isArray(record.routes)
    ? record.routes
      .map((entry) => {
        const route = entry && typeof entry === "object" ? entry as Record<string, unknown> : null;
        if (!route) {
          return null;
        }
        const routeKey = trimString(route.route_key);
        const recipient = trimString(route.recipient);
        const sink = trimString(route.sink);
        const targetModule = trimString(route.target_module);
        if (!routeKey || !recipient || !sink || !targetModule) {
          return null;
        }
        return {
          route_key: routeKey,
          recipient,
          sink,
          target_module: targetModule,
          ...(trimString(route.channel) ? { channel: trimString(route.channel) } : {}),
          ...(normalizeFiniteNumber(route.retry_max) !== undefined ? { retry_max: normalizeFiniteNumber(route.retry_max) } : {}),
          ...(normalizeFiniteNumber(route.backoff_ms) !== undefined ? { backoff_ms: normalizeFiniteNumber(route.backoff_ms) } : {}),
          ...(normalizeFiniteNumber(route.rate_limit_per_minute) !== undefined
            ? { rate_limit_per_minute: normalizeFiniteNumber(route.rate_limit_per_minute) }
            : {}),
        };
      })
      .filter((entry): entry is RoutingSectionView["routes"][number] => Boolean(entry))
    : [];
  const deliveries = Array.isArray(record.deliveries)
    ? record.deliveries
      .map((entry) => {
        const delivery = entry && typeof entry === "object" ? entry as Record<string, unknown> : null;
        if (!delivery) {
          return null;
        }
        const deliveryId = trimString(delivery.delivery_id);
        const routeId = trimString(delivery.route_id);
        const recipient = trimString(delivery.recipient);
        const sink = trimString(delivery.sink);
        const targetModule = trimString(delivery.target_module);
        const status = trimString(delivery.status);
        const firstAttempt = normalizeFiniteNumber(delivery.first_attempt_ms);
        const finalAttempt = normalizeFiniteNumber(delivery.final_attempt_ms);
        if (!deliveryId || !routeId || !recipient || !sink || !targetModule || !status || firstAttempt === undefined || finalAttempt === undefined) {
          return null;
        }
        const attempts = Array.isArray(delivery.attempts)
          ? delivery.attempts
            .map((attemptRaw) => {
              const attempt = attemptRaw && typeof attemptRaw === "object" ? attemptRaw as Record<string, unknown> : null;
              if (!attempt) {
                return null;
              }
              const attemptNumber = normalizeFiniteNumber(attempt.attempt);
              const attemptStatus = trimString(attempt.status);
              const backoff = normalizeFiniteNumber(attempt.backoff_ms);
              if (attemptNumber === undefined || !attemptStatus || backoff === undefined) {
                return null;
              }
              return {
                attempt: attemptNumber,
                status: attemptStatus,
                backoff_ms: backoff,
              };
            })
            .filter((attempt): attempt is RoutingSectionView["deliveries"][number]["attempts"][number] => Boolean(attempt))
          : [];
        return {
          delivery_id: deliveryId,
          route_id: routeId,
          recipient,
          sink,
          target_module: targetModule,
          status,
          first_attempt_ms: firstAttempt,
          final_attempt_ms: finalAttempt,
          attempts,
          ...(trimString(delivery.idempotency_key) ? { idempotency_key: trimString(delivery.idempotency_key) } : {}),
          ...(trimString(delivery.sink_adapter) ? { sink_adapter: trimString(delivery.sink_adapter) } : {}),
        };
      })
      .filter((entry): entry is RoutingSectionView["deliveries"][number] => Boolean(entry))
    : [];
  return { routes, deliveries };
}

export function normalizeReplayUnavailableError(value: unknown): ReplayUnavailableError | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (
    !record ||
    (record.error !== "replay_unavailable" && record.type !== "replay_unavailable")
  ) {
    return null;
  }
  const explicitStream =
    record.stream === "identity" || record.stream === "all_events" || record.stream === "timeline"
      ? record.stream
      : null;
  const requested =
    trimString(record.requested_last_event_id) || trimString(record.requested_cursor);
  const latest =
    trimString(record.latest_event_id) || trimString(record.latest_cursor);
  const stream =
    explicitStream ||
    (requested?.startsWith("console:") || latest?.startsWith("console:") ? "timeline" : null);
  if (!stream || !requested || !latest) {
    return null;
  }
  return {
    error: "replay_unavailable",
    stream,
    requested_last_event_id: requested,
    latest_event_id: latest,
  };
}

export function normalizeConsoleInteractionRejectedError(value: unknown): ConsoleInteractionRejectedError | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const code = record.code;
  const message = trimString(record.message);
  if (
    code !== -32001
    && code !== -32002
    && code !== -32003
    && code !== -32004
    && code !== -32602
    && code !== -32603
  ) {
    return null;
  }
  if (!message) {
    return null;
  }
  return { code, message };
}

export function normalizeToolCallAccumulatorState(value: unknown): ToolCallAccumulatorState | null {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : null;
  if (!record) {
    return null;
  }
  const timeoutMs = normalizeFiniteNumber(record.timeoutMs);
  if (timeoutMs === undefined || timeoutMs <= 0) {
    return null;
  }

  const toolCalls = record.toolCalls && typeof record.toolCalls === "object"
    ? Object.fromEntries(
        Object.entries(record.toolCalls as Record<string, unknown>)
          .map(([toolCallId, raw]) => {
            const normalizedId = trimString(toolCallId);
            const rawBlock = raw && typeof raw === "object" ? raw as Record<string, unknown> : null;
            if (!normalizedId || !rawBlock) {
              return null;
            }
            const name = trimString(rawBlock.name);
            const argumentsText = trimString(rawBlock.arguments);
            const status = rawBlock.status === "pending" || rawBlock.status === "success" || rawBlock.status === "error"
              ? rawBlock.status
              : null;
            if (rawBlock.type !== "tool-call" || !name || !argumentsText || !status) {
              return null;
            }
            return [
              normalizedId,
              {
                type: "tool-call" as const,
                toolCallId: normalizedId,
                name,
                arguments: argumentsText,
                ...(trimString(rawBlock.result) ? { result: trimString(rawBlock.result) } : {}),
                status,
              },
            ];
          })
          .filter((entry): entry is [string, ConversationRichToolCallBlock] => Boolean(entry)),
      )
    : {};

  const pendingResults = record.pendingResults && typeof record.pendingResults === "object"
    ? Object.fromEntries(
        Object.entries(record.pendingResults as Record<string, unknown>)
          .map(([toolCallId, result]) => {
            const normalizedId = trimString(toolCallId);
            const normalizedResult = trimString(result);
            return normalizedId && normalizedResult ? [normalizedId, normalizedResult] : null;
          })
          .filter((entry): entry is [string, string] => Boolean(entry)),
      )
    : {};

  return {
    toolCalls,
    pendingResults,
    timeoutMs,
  };
}
