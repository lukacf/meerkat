/**
 * Meta runtime — the running instance returned by the builder.
 *
 * @example
 * ```ts
 * const rt = await Meta.builder().mob("mob.toml").gateway(bin).build();
 * const handle = rt.mobHandle();
 *
 * const status = await handle.status();
 * console.log(status.contractVersion, status.loadedModules);
 *
 * await rt.shutdown();
 * ```
 */

import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { readFileSync } from "node:fs";
import { randomUUID } from "node:crypto";

import type { MetaBuilderConfig } from "./builder.js";
import {
  CallbackDispatcher,
  type SessionAgentBuilder,
} from "./agent-builder.js";
import {
  MOB_EVENTS_STALE_CURSOR_CODE,
  CAPABILITY_UNAVAILABLE_CODE,
  CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE,
  STALE_DELIVERY_SCOPE_CODE,
  REGISTERED_SPEC_CHANGED_CODE,
  MEMORY_BACKEND_UNAVAILABLE_CODE,
  STORAGE_RESOLUTION_CODE,
  INIT_IN_PROGRESS_CODE,
  COMPOSITION_PROVENANCE_CODE,
  MEMBER_RELOAD_REQUIRED_CODE,
  MEMBER_RELOAD_REQUIRED_KIND,
  FLOW_ROLE_TARGETS_REFUSED_KIND,
  FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND,
  WORKGRAPH_UNAVAILABLE_CODE,
  WORKGRAPH_CONFLICT_CODE,
  IDENTITY_UNRESUMABLE_CODE,
  IDENTITY_HISTORY_UNAVAILABLE_CODE,
  IDENTITY_HOST_ADOPTED_CODE,
  IDENTITY_WAKE_HELD_KINDS,
  CapabilityUnavailableError,
  CompositionProvenanceError,
  FlowRoleTargetsNotAppliedError,
  FlowRoleTargetsRefusedError,
  FlowRunIdempotencyConflictError,
  MemberReloadRequiredError,
  ConsoleTimelineReplayUnavailableError,
  RegisteredSpecChangedError,
  StaleScopeError,
  MemoryBackendUnavailableError,
  MobEventsStaleError,
  NotConnectedError,
  RpcError,
  StorageResolutionError,
  InitInProgressError,
  TransportError,
  TransportReaderFailedError,
  GatewayStartupCleanupError,
  type GatewayCleanup,
  IdentityHistoryUnavailableError,
  IdentityHostAdoptedError,
  IdentityUnresumableError,
  IdentityWakeHeldError,
  InitOutcomeUnknownError,
  PostAdmissionObservationError,
  TurnFailedError,
  TurnNotDeliveredError,
  TurnTrackingUnavailableError,
  TurnUnknownError,
  TurnWaitTimeoutError,
  WaitEndedError,
  WorkGraphUnavailableError,
  WorkGraphConflictError,
  isRpcError,
  parseAnnouncementRpcError,
} from "./errors.js";
import {
  INIT_PROTOCOL_ACCEPTED_THEN_SETTLED,
  InitDeadlineExceeded,
  type InitWatch,
  PersistentTransport,
  type TransportTermination,
  buildJsonRpcRequest,
} from "./transport.js";
import { parseSseStream, type SseEvent } from "./sse.js";
import {
  EventStream,
  parseAgentEventFromSse,
  parseMobEventFromSse,
  type AgentEventEnvelope,
  type MobEventEnvelope,
} from "./events.js";
import { discoverySpecToDict, toolArtifactRefToWire, type DiscoverySpec } from "./models.js";
import {
  LIVE_EXECUTION_IDENTITY_V1,
  activeLiveChannelHandleToWire,
  type ActiveLiveChannelConnection,
  type ActiveLiveChannelHandle,
  type ExperimentalLiveChannelStatus,
  experimentalLiveGatewayConfigToWire,
  openAiLiveGatewayConfigToWire,
  liveOpenExecutionIdentityParams,
  type LivePlaybackOwner,
  type LivePlaybackOwnerReadiness,
  type LiveAssistantOutputAddress,
  parseExperimentalLiveChannelStatus,
  parseLiveChannelHandle,
  parseLivePlaybackOwnerReadiness,
  parsePendingLiveChannelHandle,
  parseLivePlaybackCompleteResult,
  parseLiveReplacementRequired,
  supportsLiveExecutionIdentityV1,
  supportsLiveExecutionMode,
  type LiveChannelHandle,
  type LiveExecutionIdentityV1,
  type LivePlaybackCompleteResult,
  type LiveReplacementRequired,
  type PendingLiveChannelHandle,
} from "./live.js";
import {
  parseStatusResult,
  parseStorageDoctorResult,
  parseCapabilitiesResult,
  parseReconcileResult,
  parseSpawnResult,
  parseSubscribeResult,
  parseSendMessageResult,
  parseRoutingResolution,
  parseDeliveryResult,
  parseDeliveryHistoryResult,
  parseMemoryQueryResult,
  parseAgentMemoryRecord,
  parseAgentMemoryRecallResult,
  parseAgentMemoryForgetResult,
  parseAgentMemoryUpdateResult,
  parseAgentMemoryManifestResult,
  parseMemoryStoreInfo,
  parseMemoryIndexResult,
  parseCallToolResult,
  parseMemberSnapshot,
  parseMemberReloadResult,
  parseMemberHealth,
  parseRuntimeRouteResult,
  parseGatingEvaluateResult,
  parseGatingDecisionResult,
  parseGatingAuditEntry,
  parseGatingPendingEntry,
  parseRediscoverReport,
  parseReconcileEdgesReport,
  parsePersistedEvent,
  parseMobStructuralEvent,
  parseMobRun,
  parseRichMemberSnapshot,
  parseIdentityResolvedToolsResult,
  parseIdentityRoutingStatusResult,
  parseHelperResult,
  parseMobRunSnapshot,
  parseCrossMobContactEntry,
  parseModelsCatalogResult,
  parseScheduleToolsResult,
  parseMobpackToolsCatalogResult,
  parseMobpackSkillsCatalogResult,
  parseMobpackAgentDefinitionsResult,
  parseMobpackTemplatesResult,
  parseMobpackCatalogsResult,
  parseMobpackValidationResult,
  parseMobpackSourceResult,
  parseMobpackExportResult,
  parseMobpackImportResult,
  parseMobpackDraftListResult,
  parseMobpackDraftGetResult,
  parseMobpackDraftSaveResult,
  parseMobpackDraftDeleteResult,
  parseMobpackDraftHistoryResult,
  parseMobpackApplyOperationResult,
  parseMobpackDeployCommandResult,
  parseMobpackDeployResult,
  eventQueryToDict,
  parseIdentityStatus,
  parseIdentityInspection,
  parseOptionalCompletionCursor,
  parseIdentityBootstrapStatus,
  type IdentityBootstrapStatus,
  completionCursorToDict,
  parseSendResult,
  parseKeyedSendResult,
  parseSendStatusResult,
  parseDispatchResult,
  parseScopedDispatchResult,
  parseScopedRecovery,
  deliveryScopeToDict,
  parseTurnResult,
  completionProgressSince,
  parseBlobGetResult,
  parseBlobUploadResult,
  dispatchInputToDict,
  contentBlockToDict,
  parseWorkGraphItem,
  parseWorkGraphEdge,
  parseWorkGraphAttentionBinding,
  parseWorkGraphSnapshotResult,
  parseWorkGraphItemsResult,
  parseWorkGraphGoalResult,
  parseWorkGraphAttentionReassignResult,
  parseWorkGraphEventEntry,
  workGraphFilterOptionsToDict,
  workGraphReadyOptionsToDict,
  workGraphEventsOptionsToDict,
  workGraphAttentionListOptionsToDict,
  workGraphCreateOptionsToDict,
  workGraphUpdateOptionsToDict,
  workGraphOwnerInputToDict,
  workGraphClaimOptionsToDict,
  workGraphCloseOptionsToDict,
  workGraphEvidenceInputToDict,
  workGraphGoalTargetToDict,
  workGraphGoalCreateOptionsToDict,
  workGraphGoalConfirmOptionsToDict,
  workGraphGoalRequestCloseOptionsToDict,
  workGraphAttentionPauseOptionsToDict,
  type StatusResult,
  type StorageDoctorResult,
  type CapabilitiesResult,
  type ReconcileResult,
  type SpawnResult,
  type SubscribeResult,
  type SendMessageResult,
  type RoutingResolution,
  type DeliveryResult,
  type DeliveryHistoryResult,
  type MemoryQueryResult,
  type AgentMemoryRecord,
  type AgentMemoryRecordMeta,
  type AgentMemoryForgetResult,
  type AgentMemoryUpdateResult,
  type MemoryStoreInfo,
  type MemoryIndexResult,
  type CallToolResult,
  type MemberSnapshot,
  type MemberReloadResult,
  type MemberHealth,
  type RuntimeRouteResult,
  type GatingEvaluateResult,
  type GatingDecisionResult,
  type GatingAuditEntry,
  type GatingPendingEntry,
  type RediscoverReport,
  type ReconcileEdgesReport,
  type PersistedEvent,
  type MobStructuralEvent,
  type MobRun,
  type RichMemberSnapshot,
  type IdentityResolvedToolsResult,
  type IdentityRoutingStatusResult,
  type HelperResult,
  type MobRunSnapshot,
  type CrossMobContactEntry,
  type ModelsCatalogResult,
  type ScheduleToolsResult,
  type MobpackToolsCatalogResult,
  type MobpackSkillsCatalogResult,
  type MobpackAgentDefinitionsResult,
  type MobpackTemplatesResult,
  type MobpackCatalogsResult,
  type MobpackValidationResult,
  type MobpackSourceResult,
  type MobpackExportResult,
  type MobpackImportResult,
  type MobpackDraftListResult,
  type MobpackDraftGetResult,
  type MobpackDraftSaveResult,
  type MobpackDraftDeleteResult,
  type MobpackDraftHistoryResult,
  type MobpackApplyOperationResult,
  type MobpackDeployCommandResult,
  type MobpackDeployResult,
  type EventQuery,
  type IdentityStatus,
  type IdentityInspection,
  type AwaitedTurn,
  type SendResult,
  type KeyedSendResult,
  type SendStatusResult,
  type RegisteredSpec,
  type DispatchResult,
  type DeliveryScope,
  type ScopedDispatchResult,
  type ScopedRecovery,
  type TurnResult,
  type CompletionCursor,
  type BlobGetResult,
  type BlobUploadResult,
  type DispatchInput,
  type DispatchContentBlock,
  type WorkGraphItem,
  type WorkGraphEdge,
  type WorkGraphAttentionBinding,
  type WorkGraphSnapshotResult,
  type WorkGraphItemsResult,
  type WorkGraphGoalResult,
  type WorkGraphAttentionReassignResult,
  type WorkGraphEventEntry,
  type WorkGraphFilterOptions,
  type WorkGraphReadyOptions,
  type WorkGraphEventsOptions,
  type WorkGraphAttentionListOptions,
  type WorkGraphCreateOptions,
  type WorkGraphUpdateOptions,
  type WorkGraphOwnerInput,
  type WorkGraphClaimOptions,
  type WorkGraphCloseOptions,
  type WorkGraphEvidenceInput,
  type WorkGraphGoalTarget,
  type WorkGraphGoalCreateOptions,
  type WorkGraphGoalConfirmOptions,
  type WorkGraphGoalRequestCloseOptions,
  type WorkGraphAttentionPauseOptions,
  parseAnnounceResult,
  parseAnnouncement,
  parseAnnouncementSubscription,
  parseAnnouncementSubscriptionChange,
  parseAnnouncementSubscriptionView,
  parseAnnouncementBatch,
  parseAnnouncementDeliveryObservation,
  parseAnnouncementSourcePutResult,
  parseAnnouncementSource,
  parseAnnouncementSourceView,
  parseSourceAdvanceResult,
  parseAnnouncementAuditEntry,
  parseAnnouncementMemberView,
  parseAnnouncementEscalationsPage,
  type AnnouncementMemberView,
  type AnnouncementEscalationsPage,
  parseAnnouncementBatchHandling,
  type AnnouncementBatchHandling,
  parseAnnouncementRoleTemplate,
  parseAnnouncementDeclarationsView,
  type AnnouncementRoleSelector,
  type AnnouncementRoleTemplate,
  type AnnouncementDeclarationsView,
  type AnnounceResult,
  type Announcement,
  type AnnouncementMember,
  type AnnouncementRef,
  type AnnouncementSubscription,
  type AnnouncementSubscriptionChange,
  type AnnouncementSubscriptionView,
  type AnnouncementBatch,
  type AnnouncementDeliveryObservation,
  type AnnouncementSourcePutResult,
  type AnnouncementSource,
  type AnnouncementSourceView,
  type SourceAdvanceResult,
  type AnnouncementAuditEntry,
  FLOW_ROLE_TARGETS,
  supportsFlowRoleTargets,
  FLOW_RUN_IDEMPOTENCY_KEY,
  supportsFlowRunIdempotencyKey,
} from "./types.js";

// -- Request ID counter ---------------------------------------------------

let requestCounter = 0;
function nextRequestId(method: string): string {
  return `${method}:${++requestCounter}`;
}

/** Narrow an RPC result to a plain record, or `{}` for non-object results. */
function asWireRecord(raw: unknown): Record<string, unknown> {
  return typeof raw === "object" && raw !== null
    ? (raw as Record<string, unknown>)
    : {};
}

/** JSON-RPC 2.0 "method not found": an older gateway without the method. */
const JSONRPC_METHOD_NOT_FOUND = -32601;

/** The feature capability of a keyed `meta/send` that takes `handling_mode`. */
const KEYED_SEND_HANDLING_MODE = "keyed_send_handling_mode";

/**
 * The mob announcement methods a Meta without sources serves. Any other
 * `meta/mob_announcements/*` method came with sources, so method-not-found on
 * it means the gateway's Meta does not support sources.
 */
const ANNOUNCEMENT_METHODS_BEFORE_SOURCES: ReadonlySet<string> = new Set(
  [
    "announce",
    "get",
    "subscriptions/put",
    "subscriptions/pause",
    "subscriptions/resume",
    "subscriptions/cancel",
    "subscriptions/get",
    "subscriptions/list",
    "deliveries/list",
    "deliveries/observe",
    "deliveries/reconcile",
  ].map((name) => `meta/mob_announcements/${name}`),
);

/** A member as an announcement param: the parsed form or the wire form. */
export type AnnouncementMemberParam =
  | AnnouncementMember
  | { readonly mob_id: string; readonly identity: string };

function announcementMemberToWire(member: AnnouncementMemberParam): Record<string, string> {
  return "mobId" in member
    ? { mob_id: member.mobId, identity: member.identity }
    : { mob_id: member.mob_id, identity: member.identity };
}

/** A role template's selector as a param: the parsed form or the wire form. */
export type AnnouncementRoleSelectorParam =
  | AnnouncementRoleSelector
  | { readonly mob_id: string; readonly role: string };

function announcementRoleSelectorToWire(selector: AnnouncementRoleSelectorParam): Record<string, string> {
  return "mobId" in selector
    ? { mob_id: selector.mobId, role: selector.role }
    : { mob_id: selector.mob_id, role: selector.role };
}

/** A wire object a caller declares, sent as given (snake_case). */
export type AnnouncementWireObject = Readonly<Record<string, unknown>>;

/** Drop the unset entries: the gateway's params are strict, and an absent one takes its default. */
function definedParams(params: Record<string, unknown>): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) result[key] = value;
  }
  return result;
}

/**
 * Transport allowance beyond a server-side wait's own deadline, so the
 * gateway's typed answer at the deadline arrives before the transport gives
 * up.
 */
const SERVER_WAIT_TRANSPORT_HEADROOM_MS = 5_000;
// Backoff between exact-ticket observation retries after an admitted
// delivery's wait failed for a transport or RPC reason; bounded by the
// caller's remaining deadline.
const OBSERVATION_RETRY_INITIAL_MS = 100;
const OBSERVATION_RETRY_MAX_MS = 2_000;
/** Options for the `*AndWait` and `*AndWaitOutcome` helpers. */
export interface AndWaitOptions {
  timeoutMs?: number;
  pollIntervalMs?: number;
  /**
   * Throw `TurnTrackingUnavailableError` in every untracked case instead of
   * waiting identity-wide (non-attributed).
   */
  requireAttribution?: boolean;
}

/**
 * Record the admitted delivery on a failure thrown after admission. The
 * error keeps its type and is rethrown unchanged; only `admission` and
 * `ticket` are added where not already set.
 */
function attachAdmission(
  err: unknown,
  admission: SendResult | DispatchResult,
  ticket: string | null,
): void {
  if (typeof err !== "object" || err === null) return;
  const record = err as { admission?: unknown; ticket?: unknown };
  try {
    if (record.admission == null) record.admission = admission;
    if (record.ticket == null) record.ticket = ticket;
  } catch {
    // A frozen error cannot carry the receipt; it still propagates unchanged.
  }
}

/** A turn outcome thrown while waiting: it carries the admission. */
function isTurnOutcomeError(
  err: unknown,
): err is TurnFailedError | TurnUnknownError | WaitEndedError | TurnWaitTimeoutError {
  return (
    err instanceof TurnFailedError ||
    err instanceof TurnUnknownError ||
    err instanceof WaitEndedError ||
    err instanceof TurnWaitTimeoutError
  );
}

function extractMobStructuralEvents(raw: unknown): MobStructuralEvent[] {
  let events: unknown = raw;
  if (typeof raw === "object" && raw !== null) {
    const record = raw as Record<string, unknown>;
    if (Array.isArray(record.events)) {
      events = record.events;
    }
  }
  if (!Array.isArray(events)) {
    return [];
  }
  return events.map(parseMobStructuralEvent);
}

/** Serialize a config value for JSON transport.
 *
 * Recursively walks dicts/arrays with cycle detection. Calls toDict()
 * on objects that have it (e.g. typed config classes). Cyclic or
 * non-serializable leaves become their constructor name.
 */
function serializeConfig(
  value: unknown,
  seen: WeakSet<object> = new WeakSet(),
): unknown {
  if (value === null || value === undefined) return value;
  if (
    typeof value === "boolean" ||
    typeof value === "number" ||
    typeof value === "string"
  )
    return value;
  if (typeof value !== "object") return String(value);
  const obj = value as object;
  if (seen.has(obj)) {
    return `[circular:${obj.constructor?.name ?? "Object"}]`;
  }
  seen.add(obj);
  if ("toDict" in (obj as Record<string, unknown>)) {
    return (obj as { toDict(): unknown }).toDict();
  }
  if (Array.isArray(obj)) {
    return obj.map((v) => serializeConfig(v, seen));
  }
  const result: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    result[k] = serializeConfig(v, seen);
  }
  return result;
}

export interface BlobUploadInput {
  readonly blob: Blob;
  readonly mediaType?: string;
  readonly filename?: string;
  readonly alt?: string;
}

export type BlobUploadSource = Blob | BlobUploadInput;

export interface SendMessageOptions {
  readonly attachments?: readonly BlobUploadSource[];
  readonly handlingMode?: "queue" | "steer";
}

export interface RememberAgentMemoryOptions {
  readonly title: string;
  readonly body: string;
  readonly tags?: readonly string[];
  readonly realm?: string;
}

export interface RecallAgentMemoryOptions {
  readonly realm?: string;
  readonly selection?: "always" | "contextual";
  readonly queryText?: string;
  readonly queryTerms?: readonly string[];
  readonly maxEntries?: number;
}

export interface ForgetAgentMemoryOptions {
  readonly realm?: string;
}

export interface UpdateAgentMemoryOptions {
  readonly title: string;
  readonly body: string;
  readonly tags?: readonly string[];
  readonly realm?: string;
}

export interface ManifestAgentMemoryOptions {
  readonly realm?: string;
  readonly tier?: "working_set" | "full";
  readonly k?: number;
}

/** Input alternatives for {@link MobHandle.mobpackImport}. */
export interface MobpackImportOptions {
  readonly mobToml?: string;
  readonly contentBase64?: string;
  readonly document?: Record<string, unknown>;
  readonly sourceName?: string;
}

/** Options for {@link MobHandle.mobpackCreate}. */
export interface MobpackCreateOptions {
  readonly template?: string;
  readonly name?: string;
  readonly trigger?: string;
}

/** Options for {@link MobHandle.mobpackSave}. */
export interface MobpackSaveOptions {
  readonly validation?: Record<string, unknown>;
  readonly stage?: string;
  readonly expectedRevision?: number;
  readonly expectedEtag?: string;
}

/** Options for {@link MobHandle.mobpackUndo} and {@link MobHandle.mobpackRedo}. */
export interface MobpackHistoryOptions {
  readonly expectedRevision?: number;
  readonly expectedEtag?: string;
}

function mobpackHistoryParams(
  draftId: string,
  options?: MobpackHistoryOptions,
): Record<string, unknown> {
  const params: Record<string, unknown> = { id: draftId };
  if (options?.expectedRevision !== undefined) {
    params.expected_revision = options.expectedRevision;
  }
  if (options?.expectedEtag !== undefined) {
    params.expected_etag = options.expectedEtag;
  }
  return params;
}

interface NormalizedBlobUpload {
  readonly uploadId: string;
  readonly blob: Blob;
  readonly mediaType: string;
  readonly filename: string;
  readonly alt?: string;
}

function normalizeBlobUpload(
  input: BlobUploadSource,
  index: number,
): NormalizedBlobUpload {
  const record = input instanceof Blob ? { blob: input } : input;
  const blob = record.blob;
  const mediaType = record.mediaType || blob.type || "application/octet-stream";
  const extension = mediaType.includes("/") ? mediaType.split("/")[1] : "bin";
  return {
    uploadId: `upload-${index + 1}`,
    blob,
    mediaType,
    filename: record.filename || `attachment-${index + 1}.${extension}`,
    alt: record.alt,
  };
}

// -- MetaRuntime --------------------------------------------------------

export interface JobsListOptions {
  readonly sessionId: string;
  readonly limit?: number;
}

export interface JobSubscriptionOptions {
  readonly subscriptionId: string;
  readonly sessionId: string;
  readonly delivery: Record<string, unknown>;
}

export interface MonitorStartOptions {
  readonly sessionId: string;
  readonly submissionKey: string;
  readonly command: string;
  readonly timeoutSecs: number;
  readonly restartClass:
    | "checkpoint_resumable"
    | "replayable"
    | "non_resumable";
  readonly delivery: Record<string, unknown>;
  readonly protocol?: "framed_jsonl" | "lines";
  readonly workingDir?: string;
  readonly maxLineBytes?: number;
  readonly maxNotificationsPerWindow?: number;
  readonly notificationWindowMs?: number;
  readonly maxRetainedDiagnosticBytes?: number;
}

/** Safe application-facing durable-job surface. */
export class JobsHandle {
  /** @internal */
  constructor(private readonly runtime: MetaRuntime) {}

  get(jobId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/get", { job_id: jobId });
  }

  list(options: JobsListOptions): Promise<unknown> {
    const params: Record<string, unknown> = { session_id: options.sessionId };
    if (options.limit !== undefined) params.limit = options.limit;
    return this.runtime._rpc("jobs/list", params);
  }

  cancel(jobId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/cancel", { job_id: jobId });
  }

  progress(jobId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/progress", { job_id: jobId });
  }

  result(jobId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/result", { job_id: jobId });
  }

  artifacts(jobId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/artifacts", { job_id: jobId });
  }

  retry(jobId: string, retryDueAtMs: number): Promise<unknown> {
    return this.runtime._rpc("jobs/retry", {
      job_id: jobId,
      retry_due_at_ms: retryDueAtMs,
    });
  }

  health(): Promise<unknown> {
    return this.runtime._rpc("jobs/health", {});
  }

  subscribe(jobId: string, options: JobSubscriptionOptions): Promise<unknown> {
    return this.runtime._rpc("jobs/subscribe", {
      job_id: jobId,
      subscription_id: options.subscriptionId,
      session_id: options.sessionId,
      delivery: options.delivery,
    });
  }

  unsubscribe(jobId: string, subscriptionId: string): Promise<unknown> {
    return this.runtime._rpc("jobs/unsubscribe", {
      job_id: jobId,
      subscription_id: subscriptionId,
    });
  }
}

/** Durable monitor convenience surface; lifecycle remains job-owned. */
export class MonitorsHandle {
  /** @internal */
  constructor(private readonly runtime: MetaRuntime) {}

  start(options: MonitorStartOptions): Promise<unknown> {
    const params: Record<string, unknown> = {
      session_id: options.sessionId,
      submission_key: options.submissionKey,
      command: options.command,
      timeout_secs: options.timeoutSecs,
      protocol: options.protocol ?? "framed_jsonl",
      restart_class: options.restartClass,
      delivery: options.delivery,
    };
    if (options.workingDir !== undefined) params.working_dir = options.workingDir;
    if (options.maxLineBytes !== undefined) params.max_line_bytes = options.maxLineBytes;
    if (options.maxNotificationsPerWindow !== undefined) {
      params.max_notifications_per_window = options.maxNotificationsPerWindow;
    }
    if (options.notificationWindowMs !== undefined) {
      params.notification_window_ms = options.notificationWindowMs;
    }
    if (options.maxRetainedDiagnosticBytes !== undefined) {
      params.max_retained_diagnostic_bytes = options.maxRetainedDiagnosticBytes;
    }
    return this.runtime._rpc("monitors/start", params);
  }
}


/** The typed process warnings the `*AndWait` helpers emit. */
export type TurnWarningType =
  | "TurnTrackingUnavailableWarning"
  | "TurnOutputTruncatedWarning"
  | "TurnOutputUnavailableWarning";

/**
 * Emit a typed Node process warning: observable with
 * `process.on("warning", ...)`, where `warning.name` is `type`.
 */
function emitTurnWarning(type: TurnWarningType, message: string): void {
  process.emitWarning(message, { type });
}

/**
 * The `*AndWait` return value for a completed turn: its text, or `null` when
 * it committed none. Truncated text and "no output of its own" are never
 * returned silently: each emits a typed warning.
 */
function textOfTurn(
  identity: string,
  turn: TurnResult,
  operation: string,
): string | null {
  if (turn.outputStatus === "text") {
    if (turn.outputTruncated) {
      emitTurnWarning(
        "TurnOutputTruncatedWarning",
        `${operation} for identity ${identity}: turn ${turn.ticket} returned ` +
          `text cut at the gateway's bound`,
      );
    }
    return turn.output;
  }
  if (turn.outputStatus === "empty") return null;
  emitTurnWarning(
    "TurnOutputUnavailableWarning",
    `${operation} for identity ${identity}: turn ${turn.ticket} completed ` +
      `without output of its own (${turn.outputStatus ?? "unknown"}); ` +
      `resolving null`,
  );
  return null;
}

/**
 * `rt:{identity}:{generation}`: a generated runtime alias, which the gateway
 * resolves to the durable identity it names (the generation is numeric).
 */
const RUNTIME_ALIAS_GENERATION = /^\+?[0-9]+$/;
const U64_MAX = 18446744073709551615n;

/**
 * The member id `meta/run_flow` echoes for a requested one: a generated
 * runtime alias names its durable identity; any other id is kept.
 */
function durableMemberAlias(member: string): string {
  if (member.startsWith("rt:")) {
    const rest = member.slice(3);
    const separator = rest.lastIndexOf(":");
    if (separator > 0) {
      const generation = rest.slice(separator + 1);
      if (
        RUNTIME_ALIAS_GENERATION.test(generation) &&
        BigInt(generation) <= U64_MAX
      ) {
        return rest.slice(0, separator);
      }
    }
  }
  return member;
}

/** The `role_targets` parameter: each role with its member ids. */
function roleTargetsRequest(
  roleTargets: Readonly<Record<string, readonly string[]>>,
): Record<string, string[]> {
  const request: Record<string, string[]> = {};
  for (const [role, members] of Object.entries(roleTargets)) {
    if (
      !Array.isArray(members) ||
      !members.every((member) => typeof member === "string")
    ) {
      throw new TypeError(
        `roleTargets[${JSON.stringify(role)}] must be an array of member id strings`,
      );
    }
    request[role] = [...members];
  }
  return request;
}

/** Whether `meta/run_flow` echoed the binding it was sent. */
function roleTargetsEchoed(
  requested: Readonly<Record<string, readonly string[]>>,
  echoed: unknown,
): boolean {
  if (typeof echoed !== "object" || echoed === null || Array.isArray(echoed)) {
    return false;
  }
  const echoedRoles = echoed as Record<string, unknown>;
  const requestedRoles = Object.keys(requested);
  if (
    Object.keys(echoedRoles).length !== requestedRoles.length ||
    !requestedRoles.every((role) =>
      Object.prototype.hasOwnProperty.call(echoedRoles, role),
    )
  ) {
    return false;
  }
  for (const role of requestedRoles) {
    const echoedMembers = echoedRoles[role];
    if (
      !Array.isArray(echoedMembers) ||
      !echoedMembers.every((member) => typeof member === "string")
    ) {
      return false;
    }
    const expected = new Set((requested[role] ?? []).map(durableMemberAlias));
    const actual = new Set(echoedMembers as string[]);
    if (
      expected.size !== actual.size ||
      ![...expected].every((member) => actual.has(member))
    ) {
      return false;
    }
  }
  return true;
}

/** The typed SDK error for a JSON-RPC error payload. */
function rpcErrorFromPayload(
  err: Record<string, unknown>,
  rid: string,
  method: string,
): Error {
  const code = Number(err.code ?? -1);
  const message = String(err.message ?? String(err));
  if (code === CAPABILITY_UNAVAILABLE_CODE) {
    return new CapabilityUnavailableError(message, rid, method, err.data);
  }
  if (code === REGISTERED_SPEC_CHANGED_CODE) {
    return new RegisteredSpecChangedError(message, rid, method, err.data);
  }
  if (code === STALE_DELIVERY_SCOPE_CODE) {
    return new StaleScopeError(message, rid, method, err.data);
  }
  if (code === MEMORY_BACKEND_UNAVAILABLE_CODE) {
    return new MemoryBackendUnavailableError(message, rid, method, err.data);
  }
  if (code === CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE) {
    return new ConsoleTimelineReplayUnavailableError(message, rid, method, err.data);
  }
  if (code === STORAGE_RESOLUTION_CODE) {
    return new StorageResolutionError(message, rid, method, err.data);
  }
  if (code === INIT_IN_PROGRESS_CODE) {
    return new InitInProgressError(message, rid, method, err.data);
  }
  if (code === COMPOSITION_PROVENANCE_CODE) {
    return new CompositionProvenanceError(message, rid, method, err.data);
  }
  if (
    code === MEMBER_RELOAD_REQUIRED_CODE &&
    typeof err.data === "object" &&
    err.data !== null &&
    (err.data as Record<string, unknown>).kind === MEMBER_RELOAD_REQUIRED_KIND
  ) {
    return new MemberReloadRequiredError(message, rid, method, err.data);
  }
  if (
    code === -32602 &&
    typeof err.data === "object" &&
    err.data !== null &&
    (err.data as Record<string, unknown>).kind === FLOW_ROLE_TARGETS_REFUSED_KIND
  ) {
    return new FlowRoleTargetsRefusedError(message, rid, method, err.data);
  }
  if (
    code === -32602 &&
    typeof err.data === "object" &&
    err.data !== null &&
    (err.data as Record<string, unknown>).kind === FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND
  ) {
    return new FlowRunIdempotencyConflictError(message, rid, method, err.data);
  }
  if (code === WORKGRAPH_UNAVAILABLE_CODE) {
    return new WorkGraphUnavailableError(message, rid, method, err.data);
  }
  if (code === WORKGRAPH_CONFLICT_CODE) {
    return new WorkGraphConflictError(message, rid, method, err.data);
  }
  if (code === IDENTITY_UNRESUMABLE_CODE) {
    return new IdentityUnresumableError(message, rid, method, err.data);
  }
  if (code === IDENTITY_HISTORY_UNAVAILABLE_CODE) {
    return new IdentityHistoryUnavailableError(message, rid, method, err.data);
  }
  if (code === IDENTITY_HOST_ADOPTED_CODE) {
    return new IdentityHostAdoptedError(message, rid, method, err.data);
  }
  const announcementError = parseAnnouncementRpcError(code, message, rid, method, err.data);
  if (announcementError !== null) return announcementError;
  if (
    code === -32603 &&
    typeof err.data === "object" &&
    err.data !== null &&
    IDENTITY_WAKE_HELD_KINDS.has(String((err.data as Record<string, unknown>).kind))
  ) {
    return new IdentityWakeHeldError(message, rid, method, err.data);
  }
  const rpcError = new RpcError(code, message, rid, method, err.data);
  if (code === MOB_EVENTS_STALE_CURSOR_CODE) {
    return MobEventsStaleError.fromRpcError(rpcError);
  }
  return rpcError;
}

/**
 * Running Meta runtime instance.
 *
 * Supports explicit lifecycle (`connect` / `shutdown`).
 */
export class MetaRuntime {
  private _config: MetaBuilderConfig;
  private _transport: PersistentTransport | null;
  private readonly _injectedTransport: PersistentTransport | null;
  private _running = false;
  private _lifecycleTail: Promise<void> = Promise.resolve();
  private _lifecycleTailResult: Promise<void> | null = null;
  private _lifecycleTailIntent: "connect" | "shutdown" | null = null;
  private _lifecycleSequence = 0;
  private _dispatcher = new CallbackDispatcher();
  private _rustHttpBase: string | null = null;
  private _rustHttpPublicBase: string | null = null;
  // Set while a stop of `_transport` was requested and has not fulfilled:
  // the runtime keeps that transport (and its live child) until it does.
  private _stopPending = false;
  // The transport this runtime last bootstrapped, for its termination record.
  private _lastGatewayTransport: PersistentTransport | null = null;
  readonly jobs = new JobsHandle(this);
  readonly monitors = new MonitorsHandle(this);

  /** @internal */
  constructor(config: MetaBuilderConfig, transport?: PersistentTransport) {
    this._config = config;
    this._transport = transport ?? null;
    this._injectedTransport = transport ?? null;
  }

  /** @internal */
  static async _create(config: MetaBuilderConfig): Promise<MetaRuntime> {
    const runtime = new MetaRuntime(config);
    await runtime.connect();
    return runtime;
  }

  /** Explicitly connect to the runtime. Idempotent. */
  async connect(): Promise<void> {
    if (this._running && this._lifecycleTailIntent === null) return;
    if (
      this._lifecycleTailIntent === "connect" &&
      this._lifecycleTailResult !== null
    ) {
      return this._lifecycleTailResult;
    }

    const previous = this._lifecycleTail;
    const sequence = ++this._lifecycleSequence;
    const operation = (async () => {
      await previous;
      if (this._running) return;

      // A retained transport whose stop has not fulfilled settles first, on
      // that same owner: no second gateway child starts beside a live one,
      // and the factory is not invoked until it does.
      await this._settleRetainedTransport();

      try {
        await this._bootstrap();
      } catch (error) {
        // A failed bootstrap must not leave an unowned child behind: the
        // runtime keeps the transport until its stop fulfils.
        const transport = this._transport;
        this._rustHttpBase = null;
        this._rustHttpPublicBase = null;
        if (transport !== null) {
          try {
            await this._stopRetained(transport);
          } catch (cleanupError) {
            // Every rejected stop keeps the transport with this runtime, so
            // its cleanup travels with the failure: no runtime may be
            // returned. A rejection is not proof the process still runs; the
            // stop's record tells what it observed.
            throw new GatewayStartupCleanupError(error, cleanupError, this._gatewayCleanup());
          }
        }
        throw error;
      }

      // A later queued operation owns the final state. Do not briefly reopen
      // RPC admission when an ordered shutdown is already waiting behind us.
      if (this._lifecycleSequence === sequence) this._running = true;
    })();

    const connect = operation.finally(() => {
      if (this._lifecycleSequence === sequence) {
        this._lifecycleTailResult = null;
        this._lifecycleTailIntent = null;
      }
    });
    this._lifecycleTailResult = connect;
    this._lifecycleTailIntent = "connect";
    this._lifecycleTail = connect.then(
      () => undefined,
      () => undefined,
    );
    return connect;
  }

  /**
   * Stop `transport` and release it only once that stop fulfils. A rejected
   * stop leaves it as this runtime's retained transport, with whatever it
   * still owns, for a later `shutdown()` or `connect()` to retry.
   */
  private async _stopRetained(transport: PersistentTransport): Promise<void> {
    this._stopPending = true;
    await transport.stop();
    if (this._transport === transport) this._transport = null;
    this._stopPending = false;
  }

  /** Rejoin the stop of a retained transport, inside a queued lifecycle operation. */
  private async _settleRetainedTransport(): Promise<void> {
    const transport = this._transport;
    if (this._stopPending && transport !== null) await this._stopRetained(transport);
  }

  /** This runtime's gateway cleanup, for a caller that has no runtime. */
  private _gatewayCleanup(): GatewayCleanup {
    return {
      shutdown: () => this.shutdown(),
      lastTermination: () => this.lastGatewayTermination(),
    };
  }

  /**
   * What the last stop of this runtime's gateway transport observed (see
   * {@link PersistentTransport.lastTermination}), kept after `shutdown()`;
   * `null` while that gateway runs, before any stop, or without a gateway.
   * An exit 0 without escalation is the process outcome only.
   */
  lastGatewayTermination(): TransportTermination | null {
    return (this._transport ?? this._lastGatewayTransport)?.lastTermination() ?? null;
  }

  /**
   * The shutdown horizon, in milliseconds, that this runtime's gateway
   * advertised and its transport accepted (see
   * {@link PersistentTransport.advertisedShutdownHorizonMs}). A process that
   * supervises this application waits at least this long, plus the
   * transport's own TERM and KILL graces, before it kills the gateway.
   * `null` without a running gateway or when the gateway advertised no valid
   * horizon.
   */
  get gatewayShutdownHorizonMs(): number | null {
    return this._transport?.advertisedShutdownHorizonMs() ?? null;
  }

  /**
   * Compose the gateway transport for one connect, before any init request:
   * the configured `gatewayTransport()` factory, else the transport given to
   * the constructor, else a default `PersistentTransport`. Subclasses may
   * override it. The returned transport finishes its own trusted startup in
   * `startAsync()`; remote RPC and model parameters never select it.
   */
  protected _makeGatewayTransport(gatewayBin: string): PersistentTransport {
    const timeoutMs = this._config.gatewayTimeoutMs ?? undefined;
    const factory = this._config.gatewayTransportFactory;
    if (factory) {
      const transport = factory({ gatewayBin, timeoutMs });
      if (!(transport instanceof PersistentTransport)) {
        throw new TypeError("gateway transport factory must return a PersistentTransport");
      }
      return transport;
    }
    if (this._injectedTransport !== null) return this._injectedTransport;
    return new PersistentTransport(gatewayBin, { timeout: timeoutMs });
  }

  private async _bootstrap(): Promise<void> {
    if (this._config.gatewayBin) {
      const transport = this._makeGatewayTransport(this._config.gatewayBin);
      this._transport = transport;
      this._lastGatewayTransport = transport;
      if (
        this._config.gatewayTransportFactory ||
        this._makeGatewayTransport !== MetaRuntime.prototype._makeGatewayTransport
      ) {
        transport.requireHostStartup();
      }

      // Register builder FIRST — init may trigger callback/build_agent
      if (this._config.sessionBuilder) {
        this._dispatcher.registerBuilder(this._config.sessionBuilder);
      }
      if (this._config.errorCallback !== null) {
        this._dispatcher.registerErrorCallback(this._config.errorCallback);
      }
      if (this._config.rosterProvider !== null) {
        this._dispatcher.registerRosterProvider(this._config.rosterProvider);
      }
      if (this._config.topologyProvider !== null) {
        this._dispatcher.registerTopologyProvider(
          this._config.topologyProvider,
        );
      }
      if (this._config.agentCustomizer !== null) {
        this._dispatcher.registerAgentCustomizer(this._config.agentCustomizer);
      }
      this._dispatcher.registerJobRpc(
        (method, params) => this._rpcUnchecked(method, params),
      );
      if (this._config.jobCredentialResolver !== null) {
        this._dispatcher.registerJobCredentialResolver(
          this._config.jobCredentialResolver,
        );
      }
      transport.setCallbackHandler(
        this._dispatcher.handleCallback.bind(this._dispatcher),
      );
      // Host-owned startup completes before any init request is written.
      await transport.startAsync();

      if (!transport.isRunning()) {
        throw new TransportError(
          `gateway binary failed to start: ${this._config.gatewayBin}`,
        );
      }

      try {
        const initResult = await this._runInit(transport);
        if (
          typeof initResult === "object" &&
          initResult !== null &&
          "http_base_url" in initResult
        ) {
          this._rustHttpBase =
            String(
              (initResult as Record<string, unknown>).http_base_url ?? "",
            ) || null;
        }
        if (typeof initResult === "object" && initResult !== null) {
          const publicBase = (initResult as Record<string, unknown>)
            .http_public_base_url;
          this._rustHttpPublicBase =
            typeof publicBase === "string" && publicBase ? publicBase : null;
        }
      } catch (err) {
        // Pre-fix every error path here was rewritten to a generic
        // `TransportError`, destroying the original RPC code/message
        // (e.g. config errors like `bad mob.toml: line 7`). The fix:
        // re-throw a structured RpcError whenever one was received —
        // fail-closed init refusals (the typed StorageResolutionError)
        // write the error response and then exit, so the process being
        // dead does not make the structured error a transport failure.
        if (isRpcError(err) || err instanceof InitOutcomeUnknownError) {
          throw err;
        }
        if (!transport.isRunning()) {
          throw new TransportError("gateway process died during bootstrap");
        }
        throw new TransportError(
          `meta/init failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
    } else if (this._config.sessionBuilder) {
      this._dispatcher.registerBuilder(this._config.sessionBuilder);
    } else {
      console.warn(
        "[meta] runtime started without gateway or session builder — " +
          "RPC calls will fail with NotConnectedError",
      );
    }
  }

  /**
   * Run `meta/init` as accepted-then-settled (#550).
   *
   * The init id is generated and its watch registered before the request is
   * written. A gateway that supports the protocol answers `accepted` at once;
   * readiness comes only from the correlated `ready` settlement, and a
   * `failed` settlement throws the typed error with `data.durable_effects`.
   * An older gateway answers once with the final result, which is used as
   * before. Losing the answer, the reader or the gateway after the request
   * was written throws `InitOutcomeUnknownError`: the gateway may have
   * changed native state, so it is never reported as a refusal.
   */
  private async _runInit(transport: PersistentTransport): Promise<unknown> {
    const initId = `init-${randomUUID()}`;
    const params = this._buildInitParams();
    params.init_protocol = INIT_PROTOCOL_ACCEPTED_THEN_SETTLED;
    params.init_id = initId;
    const watch = transport.openInitWatch(initId);
    try {
      return await this._initWithWatch(transport, watch, params);
    } finally {
      transport.closeInitWatch(watch);
    }
  }

  private async _initWithWatch(
    transport: PersistentTransport,
    watch: InitWatch,
    params: Record<string, unknown>,
  ): Promise<unknown> {
    const initId = watch.initId;
    const method = "meta/init";
    const rid = nextRequestId(method);
    const request = buildJsonRpcRequest(rid, method, params);
    let response: Record<string, unknown>;
    try {
      response = (await transport.sendAsync(
        request as unknown as Record<string, unknown>,
      )) as Record<string, unknown>;
    } catch (err) {
      const reason = err instanceof TransportReaderFailedError
        ? err.message
        : `no answer to meta/init: ${err instanceof Error ? err.message : String(err)}`;
      throw new InitOutcomeUnknownError(initId, watch.lastPhase, reason);
    }
    if ("error" in response) {
      // Refused before acceptance (or by an older gateway): typed, and
      // nothing past the pre-durable gates ran.
      throw rpcErrorFromPayload(response.error as Record<string, unknown>, rid, method);
    }
    const result = response.result as Record<string, unknown> | null | undefined;
    if (!(typeof result === "object" && result !== null && result.init_state === "accepted")) {
      // An older gateway: its single response is the final init result.
      return result;
    }
    if (result.init_id !== initId) {
      throw new InitOutcomeUnknownError(
        initId,
        watch.lastPhase,
        `the gateway accepted a different init_id (${JSON.stringify(result.init_id)})`,
      );
    }
    let settled: Record<string, unknown>;
    try {
      settled = await watch.settlement(this._config.initDeadlineMs);
    } catch (err) {
      if (err instanceof TransportReaderFailedError || err instanceof InitDeadlineExceeded) {
        throw new InitOutcomeUnknownError(initId, watch.lastPhase, err.message);
      }
      throw err;
    }
    if (settled.outcome === "ready") {
      const ready: Record<string, unknown> = { ...settled };
      delete ready.init_id;
      delete ready.outcome;
      return ready;
    }
    if (settled.outcome === "failed") {
      const data = settled.data;
      const errorData: Record<string, unknown> =
        typeof data === "object" && data !== null && !Array.isArray(data)
          ? { ...(data as Record<string, unknown>) }
          : data === undefined || data === null ? {} : { detail: data };
      errorData.durable_effects = settled.durable_effects ?? "possible";
      throw rpcErrorFromPayload(
        {
          code: settled.code ?? -32603,
          message: settled.message ?? "meta/init failed",
          data: errorData,
        },
        rid,
        method,
      );
    }
    throw new InitOutcomeUnknownError(
      initId,
      watch.lastPhase,
      `unrecognized meta/init settlement outcome ${JSON.stringify(settled.outcome)}`,
    );
  }

  private _buildInitParams(): Record<string, unknown> {
    const params: Record<string, unknown> = {};
    if (this._config.preparedApplication) {
      params.prepared_application = {
        bundle_base64: Buffer.from(this._config.preparedApplication.bundle).toString("base64"),
        digest: this._config.preparedApplication.digest,
      };
    } else if (this._config.mobConfigPath) {
      params.mob_config = readFileSync(this._config.mobConfigPath, "utf-8");
    }
    if (this._config.modules.length > 0) {
      params.modules = this._config.modules;
    }
    params.has_session_builder = Boolean(this._config.sessionBuilder);
    const runtimeOptions: Record<string, unknown> = {};
    if (
      this._config.gatingConfigPath !== null &&
      this._config.gatingConfigPath !== undefined
    ) {
      runtimeOptions.gating_config_path = this._config.gatingConfigPath;
    }
    if (this._config.routingConfigPath) {
      runtimeOptions.routing_config_path = this._config.routingConfigPath;
    }
    if (this._config.workgraphEnabled !== null) {
      runtimeOptions.workgraph = this._config.workgraphEnabled;
    }
    if (this._config.mobAnnouncements != null) {
      runtimeOptions.mob_announcements = { ...this._config.mobAnnouncements };
    }
    if (this._config.forkInstructionPolicy !== null) {
      runtimeOptions.fork_instruction_policy = this._config.forkInstructionPolicy;
    }
    if (this._config.memoryConfig) {
      runtimeOptions.memory_config = serializeConfig(this._config.memoryConfig);
    }
    if (this._config.agentMemoryConfig) {
      runtimeOptions.agent_memory = serializeConfig(this._config.agentMemoryConfig);
    }
    if (this._config.authConfig) {
      runtimeOptions.auth_config = serializeConfig(this._config.authConfig);
    }
    if (this._config.eventLog) {
      runtimeOptions.event_log = serializeConfig(this._config.eventLog);
    }
    if (this._config.runtimeStore) {
      runtimeOptions.runtime_store = serializeConfig(this._config.runtimeStore);
    }
    if (this._config.consoleConfigPath) {
      runtimeOptions.console_config_path = this._config.consoleConfigPath;
    }
    if (
      this._config.accessConfigPath !== null &&
      this._config.accessConfigPath !== undefined
    ) {
      runtimeOptions.access_config_path = this._config.accessConfigPath;
    }
    if (this._config.meerkatConfigPath) {
      runtimeOptions.meerkat_config_path = this._config.meerkatConfigPath;
    }
    if (this._config.providerCredentialsRoot) {
      runtimeOptions.provider_credentials_root =
        this._config.providerCredentialsRoot;
    }
    if (this._config.consoleRequireAppAuth !== null) {
      runtimeOptions.console_require_app_auth =
        this._config.consoleRequireAppAuth;
    }
    if (this._config.consoleReadOnly !== null) {
      runtimeOptions.console_read_only = this._config.consoleReadOnly;
    }
    if (this._config.httpListen) {
      runtimeOptions.http_listen = this._config.httpListen;
    }
    if (this._config.httpPublicBaseUrl) {
      runtimeOptions.http_public_base_url = this._config.httpPublicBaseUrl;
    }
    if (typeof this._config.allowRemote === "boolean") {
      runtimeOptions.allow_remote = this._config.allowRemote;
    }
    if (this._config.consoleFetchTimeoutMs !== null) {
      runtimeOptions.console_fetch_timeout_ms =
        this._config.consoleFetchTimeoutMs;
    }
    if (this._config.demoLlm) {
      runtimeOptions.demo_llm = true;
    }
    if (this._config.memberCommsAddress) {
      runtimeOptions.member_comms_address = this._config.memberCommsAddress;
    }
    if (this._config.implicitDelegateIdleRetireSecs !== undefined) {
      runtimeOptions.implicit_delegate_idle_retire_secs =
        this._config.implicitDelegateIdleRetireSecs;
    }
    if (this._config.installedArtifacts !== undefined && this._config.installedArtifacts !== null) {
      runtimeOptions.installed_artifacts = this._config.installedArtifacts.map(
        toolArtifactRefToWire,
      );
    }
    if (this._config.maxSessions !== null) {
      runtimeOptions.max_sessions = this._config.maxSessions;
    }
    if (this._config.experimentalLiveConfig != null) {
      runtimeOptions.experimental_live = experimentalLiveGatewayConfigToWire(
        this._config.experimentalLiveConfig,
      );
    }
    if (this._config.openaiLiveConfig != null) {
      runtimeOptions.openai_live = openAiLiveGatewayConfigToWire(
        this._config.openaiLiveConfig,
      );
    }
    if (this._config.consoleVoiceConfig != null) {
      runtimeOptions.console_voice = openAiLiveGatewayConfigToWire(
        this._config.consoleVoiceConfig,
      );
    }
    const expectedRevision = this._config.declareSpecUpdateExpectedRevision;
    if (expectedRevision !== undefined && expectedRevision !== null) {
      runtimeOptions.mob_composition = { authority: "authoritative" };
      runtimeOptions.declare_spec_update = { expected_revision: expectedRevision };
    }
    params.runtime_options = runtimeOptions;
    if (this._config.persistentState) {
      params.persistent_state = this._config.persistentState;
    }
    const inFlightDrainMs = this._config.shutdownInFlightDrainMs;
    if (inFlightDrainMs !== undefined && inFlightDrainMs !== null) {
      params.shutdown_in_flight_drain_ms = inFlightDrainMs;
    }
    if (this._config.rosterProvider !== null) {
      params.has_roster_provider = true;
    }
    if (this._config.topologyProvider !== null) {
      params.has_topology_provider = true;
    }
    if (this._config.agentCustomizer !== null) {
      params.has_agent_customizer = true;
    }
    return params;
  }

  /** @internal */
  async _rpc(
    method: string,
    params?: Record<string, unknown>,
    options: { transportTimeoutMs?: number } = {},
  ): Promise<unknown> {
    if (!this._running) {
      throw new NotConnectedError(
        "runtime not started — no transport available",
      );
    }
    return this._rpcUnchecked(method, params, options);
  }

  private async _rpcUnchecked(
    method: string,
    params?: Record<string, unknown>,
    options: { transportTimeoutMs?: number } = {},
  ): Promise<unknown> {
    if (this._transport === null) {
      throw new NotConnectedError(
        "runtime not started — no transport available",
      );
    }
    const rid = nextRequestId(method);
    const request = buildJsonRpcRequest(rid, method, params ?? {});
    const response = (await this._transport.sendAsync(
      request as unknown as Record<string, unknown>,
      { timeoutMs: options.transportTimeoutMs },
    )) as Record<string, unknown>;

    if ("error" in response) {
      throw rpcErrorFromPayload(response.error as Record<string, unknown>, rid, method);
    }
    return response.result;
  }

  get rustHttpBaseUrl(): string | null {
    return this._rustHttpBase;
  }

  /**
   * Base URL for clients that reach the gateway through a proxy: the
   * `http_public_base_url` the launch advertised (see
   * `MetaBuilder.httpPublicBaseUrl`), falling back to `rustHttpBaseUrl`
   * when none was declared. The SDK's own SSE, multipart and console RPC
   * calls keep using `rustHttpBaseUrl`: this process spawned the gateway
   * and shares its network namespace, so the same-host form always answers.
   */
  get rustHttpPublicBaseUrl(): string | null {
    return this._rustHttpPublicBase ?? this._rustHttpBase;
  }

  setRustHttpBase(url: string): void {
    this._rustHttpBase = url;
  }

  /** @internal */
  _registerLiveOutputConsumer(
    channelId: string,
    consumer: (output: LiveAssistantOutputAddress) => void,
  ): () => void {
    return this._dispatcher.registerLiveOutputConsumer(channelId, consumer);
  }

  mobHandle(): MobHandle {
    return new MobHandle(this);
  }

  sseBridge(): SseBridge {
    return new SseBridge(this);
  }

  async shutdown(): Promise<void> {
    // Close public RPC admission synchronously, even when teardown must wait
    // for an already-admitted connect to finish its ordered bootstrap.
    this._running = false;
    if (
      this._lifecycleTailIntent === "shutdown" &&
      this._lifecycleTailResult !== null
    ) {
      return this._lifecycleTailResult;
    }

    const previous = this._lifecycleTail;
    const sequence = ++this._lifecycleSequence;
    const operation = (async () => {
      await previous;
      this._running = false;
      const transport = this._transport;
      this._rustHttpBase = null;
      this._rustHttpPublicBase = null;
      // Public RPC admission is closed (`_running`); the transport stays this
      // runtime's until its stop fulfils, so a failed stop can be rejoined.
      if (transport !== null) await this._stopRetained(transport);
    })();

    const shutdown = operation.finally(() => {
      if (this._lifecycleSequence === sequence) {
        this._lifecycleTailResult = null;
        this._lifecycleTailIntent = null;
      }
    });
    this._lifecycleTailResult = shutdown;
    this._lifecycleTailIntent = "shutdown";
    this._lifecycleTail = shutdown.then(
      () => undefined,
      () => undefined,
    );
    return shutdown;
  }

  get isRunning(): boolean {
    return this._running;
  }

  // -- Identity-first APIs (REQ-47) -----------------------------------------

  /** Get agent snapshot by identity. */
  async agent(identity: string): Promise<MemberSnapshot> {
    const status = parseIdentityStatus(
      await this._rpc("meta/status_identity", { identity }),
    );
    return {
      agentIdentity: status.agentRuntimeId || status.identity,
      role: status.profile,
      state: status.lifecycleState,
      wiredTo: [],
      labels: status.labels,
    };
  }

  /**
   * Send content to an identity. Content can be a string or content blocks.
   *
   * With `{ trackTurn: true }` the admitted turn is tracked by ticket: the
   * result's `turnTicket` names THIS turn (see {@link waitForTurn}), or
   * `turnUnavailable` says why it could not be tracked. The returned
   * `completionBaseline` is an identity-wide barrier for
   * {@link waitForCompletion}. Never wait by comparing `outputPreview` text.
   *
   * A host that authored `content` from the identity's registered row
   * ({@link registeredSpec}) passes that row's `specDigest` as
   * `expectedSpecDigest`: the gateway re-checks it against the registered
   * row under the identity's lifecycle lock before delivery and throws
   * {@link RegisteredSpecChangedError}, with nothing submitted, when the row
   * changed.
   */
  async send(
    identity: string,
    content: string | DispatchContentBlock[],
    options?: {
      trackTurn?: boolean;
      expectedSpecDigest?: string;
      idempotencyKey?: undefined;
      /** An unkeyed send queues: `"queue"` is the default; a steer needs a key. */
      handlingMode?: "queue";
    },
  ): Promise<SendResult>;
  /**
   * A keyed send: the gateway commits `content` as a member continuation
   * under `idempotencyKey`, which belongs to the identity (not a session),
   * and resolves the continuation's receipt with its current delivery
   * status. A retry under the same key never sends twice: it resolves the
   * first receipt, after a respawn, reset or gateway restart too, and other
   * content under the key is refused (`data.kind`
   * `input_idempotency_conflict`). Keys never expire. Follow an `admitted`
   * status with {@link sendStatus}. A key does not combine with `trackTurn`
   * or `expectedSpecDigest`. A gateway that does not advertise
   * `meta/send_status` would ignore the key, so the key is refused with
   * {@link CapabilityUnavailableError} before anything is sent.
   *
   * `handlingMode` is `"queue"` (the default) or `"steer"`, committed with
   * the content: `"steer"` delivers it into the identity's running turn at
   * its next boundary, or starts a turn when the identity is idle, and a
   * replay of the key in another mode is refused as other content is. A
   * steer requires a key: without one it is refused before any call. A
   * gateway that does not advertise the `keyed_send_handling_mode` feature
   * capability would ignore the mode and queue, so a keyed steer is refused
   * there with {@link CapabilityUnavailableError} before anything is sent.
   */
  async send(
    identity: string,
    content: string | DispatchContentBlock[],
    options: {
      idempotencyKey: string;
      handlingMode?: "queue" | "steer";
      trackTurn?: false;
      expectedSpecDigest?: undefined;
    },
  ): Promise<KeyedSendResult>;
  async send(
    identity: string,
    content: string | DispatchContentBlock[],
    options: {
      trackTurn?: boolean;
      expectedSpecDigest?: string;
      idempotencyKey?: string;
      handlingMode?: "queue" | "steer";
    } = {},
  ): Promise<SendResult | KeyedSendResult> {
    if (
      options.idempotencyKey !== undefined &&
      (options.trackTurn || options.expectedSpecDigest !== undefined)
    ) {
      const combined = options.trackTurn ? "trackTurn" : "expectedSpecDigest";
      throw new TypeError(`idempotencyKey does not combine with ${combined}`);
    }
    if (options.handlingMode !== undefined) {
      if (options.handlingMode !== "queue" && options.handlingMode !== "steer") {
        throw new TypeError('handlingMode must be "queue" or "steer"');
      }
      if (options.handlingMode === "steer" && options.idempotencyKey === undefined) {
        throw new TypeError('handlingMode "steer" needs idempotencyKey: an unkeyed send queues');
      }
    }
    const params: Record<string, unknown> = { identity };
    if (typeof content === "string") {
      params.content = content;
    } else {
      params.content = content.map(contentBlockToDict);
    }
    if (options.idempotencyKey !== undefined) {
      await this.requireKeyedSend(options.handlingMode === "steer");
      params.idempotency_key = options.idempotencyKey;
      if (options.handlingMode !== undefined) {
        params.handling_mode = options.handlingMode;
      }
      return parseKeyedSendResult(await this._rpc("meta/send", params));
    }
    if (options.trackTurn) params.track_turn = true;
    if (options.expectedSpecDigest !== undefined) {
      params.expected_spec_digest = options.expectedSpecDigest;
    }
    return parseSendResult(await this._rpc("meta/send", params));
  }

  /**
   * Refuse a keyed send, typed, on a gateway that does not serve it: such a
   * gateway ignores `idempotency_key` and would send unkeyed. Keyed sends ship
   * with `meta/send_status`, so its advertisement is the gate. A keyed steer
   * also needs the `keyed_send_handling_mode` feature capability: a gateway
   * without it ignores the mode and queues.
   */
  private async requireKeyedSend(steer = false): Promise<void> {
    const raw = await this._rpc("meta/capabilities");
    const methods =
      typeof raw === "object" && raw !== null
        ? (raw as Record<string, unknown>).methods
        : undefined;
    if (!Array.isArray(methods) || !methods.includes("meta/send_status")) {
      throw new CapabilityUnavailableError(
        "this gateway does not serve keyed sends: it would ignore idempotency_key and send unkeyed",
        "",
        "meta/send",
        { requires: "meta/send_status" },
      );
    }
    if (steer) {
      const features = (raw as Record<string, unknown>).feature_capabilities;
      if (!Array.isArray(features) || !features.includes(KEYED_SEND_HANDLING_MODE)) {
        throw new CapabilityUnavailableError(
          "this gateway does not serve keyed steers: it would ignore handling_mode and queue",
          "",
          "meta/send",
          { capability: KEYED_SEND_HANDLING_MODE },
        );
      }
    }
  }

  /**
   * The state of an input in a session `identity` has had (`meta/send_status`),
   * as meerkat's `session/input_status` reports it: follow a keyed send's
   * `admitted` status (its `sessionId` and `inputId`) to its terminal state.
   * Resolves `state: null` when the session holds no such input. A session
   * that is neither the identity's current session nor one it held before is
   * refused (`data.kind` `session_not_held_by_identity`).
   */
  async sendStatus(
    identity: string,
    sessionId: string,
    inputId: string,
  ): Promise<SendStatusResult> {
    return parseSendStatusResult(
      await this._rpc("meta/send_status", {
        identity,
        session_id: sessionId,
        input_id: inputId,
      }),
    );
  }

  /**
   * The identity's REGISTERED roster row (`meta/registered_spec`): the full
   * row the gateway holds and delivers under, including
   * `additional_instructions`, with its canonical digest. Not the roster
   * provider's desired row and not proof that the member's actor adopted
   * every field. Informational: pass `specDigest` to {@link send} as
   * `expectedSpecDigest` and the gateway re-checks it before delivery.
   */
  async registeredSpec(identity: string): Promise<RegisteredSpec> {
    const raw = (await this._rpc("meta/registered_spec", { identity })) as Record<
      string,
      unknown
    >;
    return {
      identity: String(raw.identity ?? identity),
      spec: (raw.spec ?? {}) as Record<string, unknown>,
      specDigest: String(raw.spec_digest),
    };
  }

  /** Dispatch structured input to an identity (`trackTurn` as for {@link send}). */
  async dispatch(
    identity: string,
    input: DispatchInput,
    options?: { trackTurn?: boolean; expectedScope?: undefined },
  ): Promise<DispatchResult>;
  /**
   * Scope-bound dispatch: exact content to the session `expectedScope` (a
   * {@link DeliveryScope} from {@link status}, persisted beforehand) pins,
   * never materialized, repaired, retargeted or prepared. Needs both
   * `idempotencyKey` and `correlationId`, does not combine with `trackTurn`,
   * resolves the admission receipt, and throws {@link StaleScopeError} when
   * the scope moved. Recover a lost reply with {@link recoverDelivery} and
   * the same scope.
   */
  async dispatch(
    identity: string,
    input: DispatchInput,
    options: { trackTurn?: false; expectedScope: DeliveryScope },
  ): Promise<ScopedDispatchResult>;
  async dispatch(
    identity: string,
    input: DispatchInput,
    options: { trackTurn?: boolean; expectedScope?: DeliveryScope } = {},
  ): Promise<DispatchResult | ScopedDispatchResult> {
    const params: Record<string, unknown> = {
      identity,
      dispatch_input: dispatchInputToDict(input),
    };
    if (options.expectedScope !== undefined) {
      if (options.trackTurn) {
        throw new TypeError("expectedScope does not combine with trackTurn");
      }
      params.expected_scope = deliveryScopeToDict(options.expectedScope);
      return parseScopedDispatchResult(await this._rpc("meta/dispatch", params));
    }
    if (options.trackTurn) params.track_turn = true;
    return parseDispatchResult(await this._rpc("meta/dispatch", params));
  }

  /**
   * Read one scoped delivery's state from the scope's ORIGINAL session.
   * Never reads the identity's current session and never resubmits. `absent`
   * is an authoritative point-in-time miss and `unresolved` an unreachable
   * original owner; neither is permission to retry. `timeoutMs` bounds the
   * evidence read (gateway default when omitted).
   */
  async recoverDelivery(
    identity: string,
    scope: DeliveryScope,
    options: { idempotencyKey: string; correlationId: string; timeoutMs?: number },
  ): Promise<ScopedRecovery> {
    const params: Record<string, unknown> = {
      identity,
      scope: deliveryScopeToDict(scope),
      idempotency_key: options.idempotencyKey,
      correlation_id: options.correlationId,
    };
    if (options.timeoutMs !== undefined) {
      params.timeout_ms = Math.max(0, Math.floor(options.timeoutMs));
    }
    return parseScopedRecovery(await this._rpc("meta/recover_delivery", params));
  }

  /**
   * Read one ticketed turn (`meta/turn_result`): its state and, once
   * completed, its OWN output.
   */
  async turnResult(identity: string, ticket: string): Promise<TurnResult> {
    return parseTurnResult(
      await this._rpc("meta/turn_result", { identity, ticket }),
    );
  }

  /**
   * Wait for the turn `ticket` names and resolve ITS typed result.
   *
   * `ticket` is the `turnTicket` of a send/dispatch made with
   * `{ trackTurn: true }`. Unlike {@link waitForCompletion}, no other delivery
   * to this identity (a peer message, a scheduled turn, a fork completion
   * wake) can satisfy this wait, and the output is the turn's own.
   *
   * Resolves the completed {@link TurnResult}. Read `outputStatus` before
   * the text: only `text` carries `output` (`outputTruncated` says whether it
   * was cut); `empty` means the turn committed no text; `no_own_result` means
   * the runtime folded the input into a run already in progress or
   * deduplicated it onto an earlier admission; `unavailable` means the
   * gateway cannot report per-turn output. Throws {@link TurnFailedError}
   * when the turn failed, {@link TurnUnknownError} when the gateway knows no
   * such turn for this identity, and a timeout error.
   *
   * Event-driven: one server-side `meta/wait_for_turn`, which the gateway
   * answers when the ticket settles. `timeoutMs` is the deadline.
   * `pollIntervalMs` applies only against a gateway predating that method,
   * which is polled with `meta/turn_result` instead.
   */
  async waitForTurn(
    identity: string,
    ticket: string,
    options: { timeoutMs?: number; pollIntervalMs?: number } = {},
  ): Promise<TurnResult> {
    const timeoutMs = options.timeoutMs ?? 90_000;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const raw = await this._serverWait(
        "meta/wait_for_turn",
        { identity, ticket },
        deadline - Date.now(),
      );
      if (raw === null) {
        return this._pollTurnCompat(identity, ticket, timeoutMs, options);
      }
      if (raw.wait === "shutting_down" && raw.state === "pending") {
        throw new WaitEndedError(identity, "shutting_down", `turn ${ticket}`);
      }
      // A server deadline shorter than ours (clamped) is re-issued.
      if (
        raw.wait === "timed_out" &&
        raw.state === "pending" &&
        deadline - Date.now() > 0
      ) {
        continue;
      }
      return settledTurn(identity, ticket, parseTurnResult(raw), timeoutMs);
    }
  }

  /**
   * Run a server-side wait (`meta/wait_for_completion`,
   * `meta/wait_for_turn`) with `timeoutMs` as the caller's deadline. The
   * gateway answers when the awaited change happens (it waits on the
   * runtime's typed change signal) or, typed, at the deadline. Resolves
   * `null` against a gateway predating the method.
   */
  private async _serverWait(
    method: string,
    params: Record<string, unknown>,
    timeoutMs: number,
  ): Promise<Record<string, unknown> | null> {
    const remaining = Math.max(0, Math.floor(timeoutMs));
    try {
      return asWireRecord(
        await this._rpc(
          method,
          { ...params, timeout_ms: remaining },
          { transportTimeoutMs: remaining + SERVER_WAIT_TRANSPORT_HEADROOM_MS },
        ),
      );
    } catch (err) {
      if (isRpcError(err) && err.code === JSONRPC_METHOD_NOT_FOUND) return null;
      throw err;
    }
  }

  /** Compatibility wait for a gateway predating `meta/wait_for_turn`. */
  private async _pollTurnCompat(
    identity: string,
    ticket: string,
    timeoutMs: number,
    options: { pollIntervalMs?: number },
  ): Promise<TurnResult> {
    const pollIntervalMs = options.pollIntervalMs ?? 500;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const result = await this.turnResult(identity, ticket);
      const remaining = deadline - Date.now();
      if (result.state !== "pending" || remaining <= 0) {
        return settledTurn(identity, ticket, result, timeoutMs);
      }
      await new Promise((resolve) =>
        setTimeout(resolve, Math.min(pollIntervalMs, remaining)),
      );
    }
  }

  /**
   * Read the identity's completion cursor without touching its session.
   *
   * `meta/completion_cursor` is a cursor-only read: the gateway answers
   * from the identity's in-memory completion cursor and never reads the
   * member session. {@link inspectIdentity} does read it (an execution
   * snapshot on the member's session task), which a tight completion poll
   * must avoid because it can hold a staged run.
   *
   * Resolves `null` for a live alias with no identity authority (nothing
   * tracks its turns; do not read that as "no turns yet"). Against a gateway
   * predating the method this falls back to {@link inspectIdentity}.
   */
  async completionCursor(identity: string): Promise<CompletionCursor | null> {
    let raw: unknown;
    try {
      raw = await this._rpc("meta/completion_cursor", { identity });
    } catch (err) {
      if (isRpcError(err) && err.code === JSONRPC_METHOD_NOT_FOUND) {
        return (await this.inspectIdentity(identity)).completionCursor;
      }
      throw err;
    }
    return parseOptionalCompletionCursor(asWireRecord(raw).completion_cursor);
  }

  /**
   * Wait until every identity has completed at least one turn (its
   * autonomous kickoff turn).
   *
   * Event-driven: one server-side `meta/wait_for_completion` per identity
   * (no baseline), which the gateway answers when the first completion is
   * recorded. It never reads the member session and does not poll; an
   * identity that has not materialized yet is simply waited for. `timeoutMs`
   * is the overall deadline.
   *
   * Only as compatibility, a gateway predating the server-side wait, or a
   * live alias with no completion cursor, falls back to the old polling
   * proxy (`pollIntervalMs`): the cursor where one exists, else a non-null
   * `outputPreview`, which under-reports an agent whose kickoff turn
   * committed no assistant text.
   *
   * Throws naming the identities that did not become ready; an RPC error for
   * an identity (an invalid identity) is thrown as is.
   */
  async waitUntilReady(
    identities: string[],
    options: { timeoutMs?: number; pollIntervalMs?: number } = {},
  ): Promise<void> {
    const timeoutMs = options.timeoutMs ?? 60_000;
    const pollIntervalMs = options.pollIntervalMs ?? 1_500;
    const deadline = Date.now() + timeoutMs;
    /** `null` once ready, else why not. */
    const ready = async (identity: string): Promise<string | null> => {
      for (;;) {
        const raw = await this._serverWait(
          "meta/wait_for_completion",
          { identity },
          deadline - Date.now(),
        );
        if (raw === null || raw.outcome === "untracked") {
          return (await this._pollReadyCompat(identity, deadline, pollIntervalMs))
            ? null
            : "timed_out";
        }
        if (raw.outcome === "completed") return null;
        // A failed kickoff run is not readiness; a server deadline shorter
        // than ours (clamped) is re-issued. Both keep waiting while the
        // caller's deadline allows.
        if (
          (raw.outcome === "run_failed" || raw.outcome === "timed_out") &&
          deadline - Date.now() > 0
        ) {
          continue;
        }
        return String(raw.outcome);
      }
    };
    const results = await Promise.all(identities.map(ready));
    const remaining = identities
      .map((identity, index) => ({ identity, reason: results[index] }))
      .filter(({ reason }) => reason !== null)
      .map(({ identity, reason }) =>
        reason === "timed_out" ? identity : `${identity} (${reason})`,
      )
      .sort();
    if (remaining.length > 0) {
      throw new Error(
        `identities did not become ready within ${timeoutMs}ms: ` +
          `${remaining.join(", ")}`,
      );
    }
  }

  /**
   * Compatibility readiness poll for a gateway predating
   * `meta/wait_for_completion`, or a live alias with no cursor.
   */
  private async _pollReadyCompat(
    identity: string,
    deadline: number,
    pollIntervalMs: number,
  ): Promise<boolean> {
    for (;;) {
      try {
        const inspection = await this.inspectIdentity(identity);
        const cursor = inspection.completionCursor;
        if (cursor !== null ? cursor.turns > 0 : inspection.outputPreview !== null) {
          return true;
        }
      } catch {
        // Not ready yet; retried on the next poll.
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) return false;
      await new Promise((resolve) =>
        setTimeout(resolve, Math.min(pollIntervalMs, remaining)),
      );
    }
  }

  /**
   * Wait until a turn completes past `baseline`, returning the
   * `outputPreview` read at completion.
   *
   * `baseline` is the `completionBaseline` from {@link send} or
   * {@link dispatch}. This compares cursors, so two consecutive turns emitting
   * byte-identical text are still two distinct completions.
   *
   * IDENTITY-WIDE: any completion on this identity after `baseline`
   * satisfies the wait (a peer message, a scheduled turn, a fork completion
   * wake), and the returned preview is the session's latest output, not
   * necessarily your turn's. To wait for one specific turn, use
   * {@link waitForTurn} with a ticket, as {@link sendAndWait} does.
   *
   * Do not mix ticket waits and cursor waits on one identity: a ticket can
   * settle before the identity health monitor counts the same turn on the
   * cursor, so a later cursor wait can be satisfied by that earlier turn.
   *
   * Event-driven: one server-side `meta/wait_for_completion`, which the
   * gateway answers when a completion is recorded (it never reads the member
   * session and does not poll); the output is then read once. `timeoutMs` is
   * the deadline. `pollIntervalMs` applies only against a gateway predating
   * that method, which is polled instead.
   *
   * Throws if the wait times out, if the identity exposes no cursor, or if the
   * runtime incarnation changed — turn counts do not carry across
   * incarnations, so that is reported rather than guessed at either way.
   */
  async waitForCompletion(
    identity: string,
    baseline: CompletionCursor,
    options: { timeoutMs?: number; pollIntervalMs?: number } = {},
  ): Promise<string | null> {
    const timeoutMs = options.timeoutMs ?? 90_000;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const raw = await this._serverWait(
        "meta/wait_for_completion",
        { identity, after: completionCursorToDict(baseline) },
        deadline - Date.now(),
      );
      if (raw === null) {
        await this._pollCursorPastCompat(identity, baseline, timeoutMs, options);
        return (await this.inspectIdentity(identity)).outputPreview;
      }
      const cursor = parseOptionalCompletionCursor(raw.completion_cursor);
      if (raw.outcome === "completed" && cursor !== null) {
        return (await this.inspectIdentity(identity)).outputPreview;
      }
      if (cursor === null || raw.outcome === "untracked") {
        throw new Error(
          `identity ${identity} reports no completion cursor; this is a live ` +
            `alias with no identity authority`,
        );
      }
      if (raw.outcome === "incarnation_changed") {
        throw new Error(
          `completion baseline ${baseline.epoch}:${baseline.turns} for ` +
            `identity ${identity} belongs to a superseded runtime ` +
            `incarnation (now ${cursor.epoch}:${cursor.turns}); capture a ` +
            `fresh baseline`,
        );
      }
      if (raw.outcome === "timed_out") {
        // A server deadline shorter than ours (clamped) is re-issued.
        if (deadline - Date.now() > 0) continue;
        throw new Error(
          `identity ${identity} did not complete a turn past ` +
            `${baseline.epoch}:${baseline.turns} within ${timeoutMs}ms`,
        );
      }
      throw new WaitEndedError(
        identity,
        String(raw.outcome),
        `baseline ${baseline.epoch}:${baseline.turns}`,
      );
    }
  }

  /**
   * Compatibility wait for a gateway predating
   * `meta/wait_for_completion`.
   */
  private async _pollCursorPastCompat(
    identity: string,
    baseline: CompletionCursor,
    timeoutMs: number,
    options: { pollIntervalMs?: number },
  ): Promise<void> {
    const pollIntervalMs = options.pollIntervalMs ?? 500;
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const cursor = await this.completionCursor(identity);
      if (cursor === null) {
        throw new Error(
          `identity ${identity} reports no completion cursor; the gateway ` +
            `predates the completion contract or this is a live alias with ` +
            `no identity authority`,
        );
      }
      const progress = completionProgressSince(cursor, baseline);
      if (progress === "completed") return;
      if (progress === "incarnation_changed") {
        throw new Error(
          `completion baseline ${baseline.epoch}:${baseline.turns} for ` +
            `identity ${identity} belongs to a superseded runtime ` +
            `incarnation (now ${cursor.epoch}:${cursor.turns}); capture a ` +
            `fresh baseline`,
        );
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new Error(
          `identity ${identity} did not complete a turn past ` +
            `${baseline.epoch}:${baseline.turns} within ${timeoutMs}ms`,
        );
      }
      await new Promise((resolve) =>
        setTimeout(resolve, Math.min(pollIntervalMs, remaining)),
      );
    }
  }

  /**
   * Send, then wait for the turn that send started and resolve its output.
   *
   * Resolves the text of {@link sendAndWaitOutcome}; use that method for the
   * typed {@link AwaitedTurn}, which says whether the text is attributed to
   * this send. The contract:
   *
   * - Tracked (a `turn_driven` member): the send carries a turn ticket and
   *   the wait is per-admission, so another delivery to this identity cannot
   *   satisfy it and the text is this turn's own committed output (`null`
   *   when it committed none). Text cut at the gateway's bound emits a
   *   `TurnOutputTruncatedWarning`; a turn that completed without output of
   *   its own resolves `null` with a `TurnOutputUnavailableWarning`.
   * - Untracked (an `autonomous_host` member, the default mode; an
   *   externally bound member; host human input; a bridge that cannot report
   *   output; `runtime_refused`; `session_rotated`; a gateway predating turn
   *   tickets): the call waits on the identity-wide completion cursor, as
   *   before, and emits a `TurnTrackingUnavailableWarning` naming the
   *   `turnUnavailable` code. That text is NOT attributed: another
   *   delivery's turn may have produced it (`AwaitedTurn.attributed` is
   *   `false` and `untrackedCode` carries the code).
   * - `requireAttribution: true` throws `TurnTrackingUnavailableError` in
   *   every untracked case.
   * - `not_delivered` (nothing ran) throws `TurnNotDeliveredError`.
   *
   * The send happens exactly once; nothing here ever resends. Every failure
   * after admission carries the send result as `admission`: a transport or
   * RPC failure of the exact-ticket wait is retried, for that ticket only,
   * until `timeoutMs` and then throws `PostAdmissionObservationError` (with
   * `ticket`); `TurnFailedError`, `TurnUnknownError`, `WaitEndedError` and
   * `TurnWaitTimeoutError` carry it. Warnings are Node process warnings
   * (`process.on("warning")`, whose `name` is the warning type).
   */
  async sendAndWait(
    identity: string,
    content: string | DispatchContentBlock[],
    options: AndWaitOptions = {},
  ): Promise<string | null> {
    return (await this.sendAndWaitOutcome(identity, content, options)).text;
  }

  /** {@link sendAndWait}, resolving the typed {@link AwaitedTurn}. */
  async sendAndWaitOutcome(
    identity: string,
    content: string | DispatchContentBlock[],
    options: AndWaitOptions = {},
  ): Promise<AwaitedTurn> {
    const result = await this.send(identity, content, { trackTurn: true });
    return this._awaitAdmitted(identity, result, "send", options);
  }

  /**
   * Dispatch, then wait for the turn it started and resolve its output (the
   * contract of {@link sendAndWait}; the typed form is
   * {@link dispatchAndWaitOutcome}).
   */
  async dispatchAndWait(
    identity: string,
    input: DispatchInput,
    options: AndWaitOptions = {},
  ): Promise<string | null> {
    return (await this.dispatchAndWaitOutcome(identity, input, options)).text;
  }

  /** {@link dispatchAndWait}, resolving the typed {@link AwaitedTurn}. */
  async dispatchAndWaitOutcome(
    identity: string,
    input: DispatchInput,
    options: AndWaitOptions = {},
  ): Promise<AwaitedTurn> {
    const result = await this.dispatch(identity, input, { trackTurn: true });
    return this._awaitAdmitted(identity, result, "dispatch", options);
  }

  private async _awaitAdmitted(
    identity: string,
    result: SendResult | DispatchResult,
    operation: string,
    options: AndWaitOptions,
  ): Promise<AwaitedTurn> {
    if (result.turnTicket != null) {
      const ticket = result.turnTicket;
      const turn = await this._observeAdmittedTurn(
        identity,
        result,
        ticket,
        operation,
        options,
      );
      return {
        text: textOfTurn(identity, turn, operation),
        attributed: true,
        admission: result,
        ticket,
        outputStatus: turn.outputStatus,
        untrackedCode: null,
      };
    }
    const unavailable = result.turnUnavailable;
    if (unavailable != null && !unavailable.delivered) {
      const refused = new TurnNotDeliveredError(
        identity,
        operation,
        unavailable.code,
        unavailable.reason,
      );
      refused.admission = result;
      throw refused;
    }
    const code = unavailable?.code ?? null;
    const reason = unavailable?.reason ?? null;
    if (options.requireAttribution === true) {
      throw new TurnTrackingUnavailableError(identity, operation, result, code, reason);
    }
    const detail =
      code !== null
        ? `${code}: ${reason}`
        : "the gateway returned no turn ticket (it predates turn tickets)";
    emitTurnWarning(
      "TurnTrackingUnavailableWarning",
      `${operation} for identity ${identity} could not track its own turn ` +
        `(${detail}); waiting on the identity-wide completion cursor ` +
        `instead, which another delivery's completion can also satisfy, so ` +
        `the result is not attributed to this delivery`,
    );
    let text: string | null;
    try {
      text = await this._waitForAdmission(
        identity,
        result.completionBaseline,
        operation,
        options,
      );
    } catch (err) {
      // RPC and transport failures are observation failures. The cursor
      // path's own errors (a wait that ended, the deadline, no baseline, a
      // superseded incarnation, a live alias, a raw transport timeout or
      // closed process) keep their type and semantics; each still carries
      // the admission, with no ticket, so the caller never redispatches.
      if (isRpcError(err) || err instanceof TransportError || err instanceof NotConnectedError) {
        throw new PostAdmissionObservationError(identity, operation, result, null, 1, err);
      }
      attachAdmission(err, result, null);
      throw err;
    }
    return {
      text,
      attributed: false,
      admission: result,
      ticket: null,
      outputStatus: null,
      untrackedCode: code,
    };
  }

  /**
   * Wait for an admitted delivery's own turn, by its ticket only. A
   * transport or RPC failure of the wait retries the same exact-ticket
   * observation, with bounded backoff, until the caller's deadline; the
   * delivery is never repeated. The turn's typed outcomes (failed, unknown,
   * wait ended, still pending at the deadline) are thrown with the admission
   * attached.
   */
  private async _observeAdmittedTurn(
    identity: string,
    result: SendResult | DispatchResult,
    ticket: string,
    operation: string,
    options: AndWaitOptions,
  ): Promise<TurnResult> {
    const timeoutMs = options.timeoutMs ?? 90_000;
    const deadline = Date.now() + timeoutMs;
    for (let attempts = 1; ; attempts += 1) {
      try {
        return await this.waitForTurn(identity, ticket, {
          timeoutMs: Math.max(0, deadline - Date.now()),
          pollIntervalMs: options.pollIntervalMs,
        });
      } catch (err) {
        if (isTurnOutcomeError(err)) {
          // A tracked outcome names its turn: a WaitEndedError gets the
          // top-level ticket too, for parity with the other outcomes.
          attachAdmission(err, result, ticket);
          throw err;
        }
        const remaining = deadline - Date.now();
        if (remaining <= 0) {
          throw new PostAdmissionObservationError(
            identity,
            operation,
            result,
            ticket,
            attempts,
            err,
          );
        }
        const backoff = Math.min(
          OBSERVATION_RETRY_INITIAL_MS * 2 ** Math.min(attempts - 1, 5),
          OBSERVATION_RETRY_MAX_MS,
          remaining,
        );
        await new Promise((resolve) => setTimeout(resolve, backoff));
      }
    }
  }

  private async _waitForAdmission(
    identity: string,
    baseline: CompletionCursor | null,
    operation: string,
    options: { timeoutMs?: number; pollIntervalMs?: number },
  ): Promise<string | null> {
    if (baseline === null) {
      throw new Error(
        `${operation} returned no completion_baseline for identity ` +
          `${identity}; the gateway predates the completion contract, so the ` +
          `turn cannot be awaited without an unsound text comparison`,
      );
    }
    return this.waitForCompletion(identity, baseline, options);
  }

  /** Subscribe to events for an identity. */
  async subscribe(identity: string): Promise<unknown> {
    return this._rpc("meta/subscribe", { identity });
  }

  /** Get identity status. */
  async status(identity: string): Promise<IdentityStatus> {
    return parseIdentityStatus(
      await this._rpc("meta/status_identity", { identity }),
    );
  }

  /** Respawn an identity (non-destructive recovery). */
  async respawn(identity: string): Promise<unknown> {
    return this._rpc("meta/respawn", { identity });
  }

  /** Retire an identity. */
  async retire(identity: string): Promise<unknown> {
    return this._rpc("meta/retire", { identity });
  }

  /** Reset an identity: destructive, it continues on a fresh session. */
  async reset(identity: string): Promise<unknown> {
    return this._rpc("meta/reset", { identity });
  }

  /**
   * Append one keyed instruction activation to a member's current session.
   *
   * Host-only. `activation` is Meerkat's `InstructionActivationRequest`
   * (`revision`, `activation_id`, `expectation`, optional `supersedes`,
   * `body`). The activation is a new transcript row that the member's next
   * turn sees; history is never rewritten. Resolves to `{ identity, receipt }`,
   * where `receipt.disposition` is `"applied"` or `"duplicate"`.
   *
   * A refusal rejects with an RPC error whose `data.class` is `"admission"`
   * (plus `data.instruction_activation_code`, for example `"session_busy"` or
   * `"live_channel_open"`), `"session"` (plus `data.session_error_code` and,
   * when the request itself was refused, `data.instruction_activation_error_code`
   * such as `"digest_mismatch"` or `"effective_activation_conflict"`),
   * `"runtime"` or `"owner_task"`. Nothing is retried: on `session_busy` or
   * `live_channel_open`, retry at your own next boundary.
   *
   * A call made while the member is mid-turn BLOCKS until that turn
   * finalizes, so it can wait for a whole turn. It then applies at the
   * boundary, never inside the turn, or rejects with `session_busy` if more
   * runtime work is already queued there. `options.transportTimeoutMs` sets
   * how long this call waits for the reply; it defaults to the transport's
   * request timeout, so raise it for members with long turns. If the wait
   * times out, the activation may still apply at the boundary: re-send the
   * identical activation, which answers `"applied"`, `"duplicate"` or the
   * typed refusal.
   */
  async activateMemberInstruction(
    identity: string,
    activation: Record<string, unknown>,
    options: { transportTimeoutMs?: number } = {},
  ): Promise<Record<string, unknown>> {
    const raw = await this._rpc(
      "meta/member_activate_instruction",
      { identity, activation },
      { transportTimeoutMs: options.transportTimeoutMs },
    );
    return raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  }

  /**
   * Read the durable instruction activations of a member's current session.
   * Host-only. Resolves to `{ identity, page }` with Meerkat's
   * `InstructionActivationReadPage`.
   */
  async memberInstructionActivations(
    identity: string,
    query: { namespace?: string; key?: string; offset?: number; limit?: number } = {},
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = { identity };
    for (const [name, value] of Object.entries(query)) {
      if (value !== undefined) params[name] = value;
    }
    const raw = await this._rpc("meta/member_instruction_activations", params);
    return raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  }

  /**
   * Start the identity over: its earlier session is no longer resumed, and
   * its next session starts empty. A live identity moves to a fresh session
   * at once, without a wake; a retired identity stays retired and starts on
   * a fresh session at its next wake; an identity with no history is left
   * as it is. An unlisted or retired identity is never made live. An
   * identity a host intent declaration manages is refused with
   * `IdentityHostAdoptedError` (-32053).
   */
  async deleteIdentity(identity: string): Promise<unknown> {
    return this._rpc("meta/delete_identity", { identity });
  }

  /** Inspect an identity's execution state. */
  async inspectIdentity(identity: string): Promise<IdentityInspection> {
    return parseIdentityInspection(
      await this._rpc("meta/inspect_identity", { identity }),
    );
  }

  /** Re-run identity-first reconciliation. */
  async reconcileIdentity(): Promise<unknown> {
    return this._rpc("meta/reconcile_identity", {});
  }

  /** Identity bootstrap snapshot (`meta/status_identity_bootstrap`). */
  async identityBootstrapStatus(): Promise<IdentityBootstrapStatus> {
    return parseIdentityBootstrapStatus(
      await this._rpc("meta/status_identity_bootstrap", {}),
    );
  }

  /**
   * Wait for the identity bootstrap (`meta/wait_identity_bootstrap`):
   * `target` is `"materialized"` (default) or `"startup_ready"`. The gateway
   * answers when the barrier is terminal or at `timeoutMs`; each identity's
   * entry carries its typed `restore` progress during an eager pass.
   */
  async waitIdentityBootstrap(
    options: { target?: string; timeoutMs?: number } = {},
  ): Promise<IdentityBootstrapStatus> {
    const params: Record<string, unknown> = {};
    if (options.target !== undefined) params.target = options.target;
    let transportTimeoutMs: number | undefined;
    if (options.timeoutMs !== undefined) {
      params.timeout_ms = Math.max(0, Math.floor(options.timeoutMs));
      transportTimeoutMs =
        (params.timeout_ms as number) + SERVER_WAIT_TRANSPORT_HEADROOM_MS;
    }
    return parseIdentityBootstrapStatus(
      await this._rpc("meta/wait_identity_bootstrap", params, {
        transportTimeoutMs,
      }),
    );
  }
}

/** The typed result of a turn wait: its result, or the matching error. */
function settledTurn(
  identity: string,
  ticket: string,
  result: TurnResult,
  timeoutMs: number,
): TurnResult {
  if (result.state === "completed") return result;
  if (result.state === "failed") {
    throw new TurnFailedError(identity, ticket, result.error ?? "");
  }
  if (result.state === "unknown") {
    throw new TurnUnknownError(identity, ticket);
  }
  throw new TurnWaitTimeoutError(identity, ticket, timeoutMs);
}

// -- MobHandle ------------------------------------------------------------

/**
 * Proxy for the Meta RPC API. Returns typed result objects.
 *
 * @example
 * ```ts
 * const handle = runtime.mobHandle();
 * const members = await handle.listMembers();
 * await handle.send(members[0].agentIdentity, "Hello!");
 * ```
 */
export class MobHandle {
  /** @internal */
  constructor(private readonly _runtime: MetaRuntime) {}

  // -- Status & capabilities ----------------------------------------------

  async status(): Promise<StatusResult> {
    return parseStatusResult(await this._runtime._rpc("meta/status"));
  }

  async capabilities(): Promise<CapabilitiesResult> {
    return parseCapabilitiesResult(
      await this._runtime._rpc("meta/capabilities"),
    );
  }

  async modelsCatalog(): Promise<ModelsCatalogResult> {
    return parseModelsCatalogResult(
      await this._runtime._rpc("meta/models/catalog"),
    );
  }

  /**
   * Meerkat's durable schedule tool definitions (`meta/schedule/tools`), the
   * base definitions a member whose profile sets `tools.schedule` is given.
   * Static: the gateway answers without a schedule store or host. Rejects on
   * a malformed result.
   */
  async scheduleTools(): Promise<ScheduleToolsResult> {
    return parseScheduleToolsResult(
      await this._runtime._rpc("meta/schedule/tools"),
    );
  }

  /**
   * Run the read-only storage doctor over a Meta state directory.
   *
   * `stateDir` is required until the gateway can report its own persistent
   * state directory (Phase M2); omitting it rejects with
   * `CapabilityUnavailableError`. `identity` narrows the continuity
   * checkpoint census to one identity.
   */
  async storageDoctor(options?: {
    stateDir?: string;
    identity?: string;
  }): Promise<StorageDoctorResult> {
    const params: Record<string, unknown> = {};
    if (options?.stateDir !== undefined) params.state_dir = options.stateDir;
    if (options?.identity !== undefined) params.identity = options.identity;
    return parseStorageDoctorResult(
      await this._runtime._rpc("meta/storage/doctor", params),
    );
  }

  async toolsCatalog(): Promise<MobpackToolsCatalogResult> {
    return parseMobpackToolsCatalogResult(
      await this._runtime._rpc("meta/tools/catalog"),
    );
  }

  async skillsCatalog(): Promise<MobpackSkillsCatalogResult> {
    return parseMobpackSkillsCatalogResult(
      await this._runtime._rpc("meta/skills/catalog"),
    );
  }

  async agentDefinitions(): Promise<MobpackAgentDefinitionsResult> {
    return parseMobpackAgentDefinitionsResult(
      await this._runtime._rpc("meta/agent_definitions/list"),
    );
  }

  async mobpackTemplates(): Promise<MobpackTemplatesResult> {
    return parseMobpackTemplatesResult(
      await this._runtime._rpc("meta/mobpacks/templates"),
    );
  }

  async mobpackCatalogs(): Promise<MobpackCatalogsResult> {
    return parseMobpackCatalogsResult(
      await this._runtime._rpc("meta/mobpacks/catalogs"),
    );
  }

  // -- Mobpack authoring ----------------------------------------------------

  /** Validate a mobpack authoring document. */
  async mobpackValidate(
    document: Record<string, unknown>,
    rkatValidate?: boolean,
  ): Promise<MobpackValidationResult> {
    const params: Record<string, unknown> = { document };
    if (rkatValidate !== undefined) params.rkat_validate = rkatValidate;
    return parseMobpackValidationResult(
      await this._runtime._rpc("meta/mobpacks/validate", params),
    );
  }

  /** Render the deployable source files (mob.toml etc.) for a document. */
  async mobpackSource(
    document: Record<string, unknown>,
  ): Promise<MobpackSourceResult> {
    return parseMobpackSourceResult(
      await this._runtime._rpc("meta/mobpacks/source", { document }),
    );
  }

  /** Export a mobpack document as a base64-encoded archive. */
  async mobpackExport(
    document: Record<string, unknown>,
  ): Promise<MobpackExportResult> {
    return parseMobpackExportResult(
      await this._runtime._rpc("meta/mobpacks/export", { document }),
    );
  }

  /** Import a mob.toml, mobpack archive, or editor document. */
  async mobpackImport(
    options: MobpackImportOptions,
  ): Promise<MobpackImportResult> {
    const params: Record<string, unknown> = {};
    if (options.mobToml !== undefined) params.mob_toml = options.mobToml;
    if (options.contentBase64 !== undefined) {
      params.content_base64 = options.contentBase64;
    }
    if (options.document !== undefined) params.document = options.document;
    if (options.sourceName !== undefined) {
      params.source_name = options.sourceName;
    }
    return parseMobpackImportResult(
      await this._runtime._rpc("meta/mobpacks/import", params),
    );
  }

  /** List mobpack draft registry rows. */
  async mobpackList(): Promise<MobpackDraftListResult> {
    return parseMobpackDraftListResult(
      await this._runtime._rpc("meta/mobpacks/list", {}),
    );
  }

  /** Fetch a single mobpack draft registry row by id. */
  async mobpackGet(draftId: string): Promise<MobpackDraftGetResult> {
    return parseMobpackDraftGetResult(
      await this._runtime._rpc("meta/mobpacks/get", { id: draftId }),
    );
  }

  /** Create a new mobpack draft from a starter template. */
  async mobpackCreate(
    options?: MobpackCreateOptions,
  ): Promise<MobpackDraftSaveResult> {
    const params: Record<string, unknown> = {};
    if (options?.template !== undefined) params.template = options.template;
    if (options?.name !== undefined) params.name = options.name;
    if (options?.trigger !== undefined) params.trigger = options.trigger;
    return parseMobpackDraftSaveResult(
      await this._runtime._rpc("meta/mobpacks/create", params),
    );
  }

  /** Save a mobpack draft, optionally guarded by revision/etag. */
  async mobpackSave(
    draftId: string,
    document: Record<string, unknown>,
    options?: MobpackSaveOptions,
  ): Promise<MobpackDraftSaveResult> {
    const params: Record<string, unknown> = { id: draftId, document };
    if (options?.validation !== undefined) {
      params.validation = options.validation;
    }
    if (options?.stage !== undefined) params.stage = options.stage;
    if (options?.expectedRevision !== undefined) {
      params.expected_revision = options.expectedRevision;
    }
    if (options?.expectedEtag !== undefined) {
      params.expected_etag = options.expectedEtag;
    }
    return parseMobpackDraftSaveResult(
      await this._runtime._rpc("meta/mobpacks/save", params),
    );
  }

  /** Delete a mobpack draft, optionally guarded by revision. */
  async mobpackDelete(
    draftId: string,
    expectedRevision?: number,
  ): Promise<MobpackDraftDeleteResult> {
    const params: Record<string, unknown> = { id: draftId };
    if (expectedRevision !== undefined) {
      params.expected_revision = expectedRevision;
    }
    return parseMobpackDraftDeleteResult(
      await this._runtime._rpc("meta/mobpacks/delete", params),
    );
  }

  /** Step a mobpack draft one entry back in its undo history. */
  async mobpackUndo(
    draftId: string,
    options?: MobpackHistoryOptions,
  ): Promise<MobpackDraftHistoryResult> {
    return parseMobpackDraftHistoryResult(
      await this._runtime._rpc(
        "meta/mobpacks/undo",
        mobpackHistoryParams(draftId, options),
      ),
    );
  }

  /** Step a mobpack draft one entry forward in its redo history. */
  async mobpackRedo(
    draftId: string,
    options?: MobpackHistoryOptions,
  ): Promise<MobpackDraftHistoryResult> {
    return parseMobpackDraftHistoryResult(
      await this._runtime._rpc(
        "meta/mobpacks/redo",
        mobpackHistoryParams(draftId, options),
      ),
    );
  }

  /** Apply a structured authoring operation to a mobpack document. */
  async mobpackApplyOperation(
    document: Record<string, unknown>,
    operation: Record<string, unknown>,
    expectedCatalogSnapshotId?: string,
  ): Promise<MobpackApplyOperationResult> {
    const params: Record<string, unknown> = { document, operation };
    if (expectedCatalogSnapshotId !== undefined) {
      params.expected_catalog_snapshot_id = expectedCatalogSnapshotId;
    }
    return parseMobpackApplyOperationResult(
      await this._runtime._rpc("meta/mobpacks/apply_operation", params),
    );
  }

  /** Preview the `rkat mob run` deploy command for a mobpack document. */
  async mobpackDeployCommand(
    document: Record<string, unknown>,
  ): Promise<MobpackDeployCommandResult> {
    return parseMobpackDeployCommandResult(
      await this._runtime._rpc("meta/mobpacks/deploy_command", { document }),
    );
  }

  /** Plan (and optionally execute) a mobpack deploy on the host. */
  async mobpackDeploy(
    document: Record<string, unknown>,
    execute?: boolean,
  ): Promise<MobpackDeployResult> {
    const params: Record<string, unknown> = { document };
    if (execute !== undefined) params.execute = execute;
    return parseMobpackDeployResult(
      await this._runtime._rpc("meta/mobpacks/deploy", params),
    );
  }

  // -- Spawn & reconcile --------------------------------------------------

  async spawn(spec: DiscoverySpec): Promise<SpawnResult> {
    return parseSpawnResult(
      await this._runtime._rpc(
        "meta/spawn_member",
        discoverySpecToDict(spec),
      ),
    );
  }

  async spawnMember(moduleId: string): Promise<SpawnResult> {
    return parseSpawnResult(
      await this._runtime._rpc("meta/spawn_member", { module_id: moduleId }),
    );
  }

  async reconcile(modules: string[]): Promise<ReconcileResult> {
    return parseReconcileResult(
      await this._runtime._rpc("meta/reconcile", { modules }),
    );
  }

  // -- Event subscription -------------------------------------------------

  async subscribeEvents(
    scope = "mob",
    lastEventId?: string,
    agentId?: string,
  ): Promise<SubscribeResult> {
    const params: Record<string, unknown> = { scope };
    if (lastEventId !== undefined) params.last_event_id = lastEventId;
    if (agentId !== undefined) params.agent_id = agentId;
    return parseSubscribeResult(
      await this._runtime._rpc("meta/events/subscribe", params),
    );
  }

  async *subscribeAgent(
    memberId: string,
  ): AsyncGenerator<AgentEventEnvelope, void, undefined> {
    const bridge = this._runtime.sseBridge();
    for await (const sse of bridge.agentEvents(memberId)) {
      yield parseAgentEventFromSse(sse);
    }
  }

  async *subscribeMob(): AsyncGenerator<MobEventEnvelope, void, undefined> {
    const bridge = this._runtime.sseBridge();
    for await (const sse of bridge.mobEvents()) {
      yield parseMobEventFromSse(sse);
    }
  }

  async queryEvents(query?: EventQuery): Promise<PersistedEvent[]> {
    const params = query ? eventQueryToDict(query) : {};
    const raw = await this._runtime._rpc("meta/query_events", params);
    let events = raw;
    if (typeof raw === "object" && raw !== null) {
      const record = raw as Record<string, unknown>;
      if (record.status === "no_event_log_configured") {
        events = Array.isArray(record.events) ? record.events : [];
      }
    }
    if (Array.isArray(events)) {
      return events.map(parsePersistedEvent);
    }
    return [];
  }

  /**
   * Query structural mob events from the meerkat ledger. Pass the
   * highest seen `cursor` as `EventQuery.afterSeq` on the next call to
   * paginate. Without `afterSeq` the call returns the latest matching
   * events (default `limit = 256`).
   *
   * Throws {@link MobEventsStaleError} when `afterSeq` is past the
   * current ledger frontier; the exception carries `afterCursor` and
   * `latestCursor` so callers can rewind.
   */
  async queryMobEvents(query?: EventQuery): Promise<MobStructuralEvent[]> {
    const params = query ? eventQueryToDict(query) : {};
    try {
      const raw = await this._runtime._rpc("meta/mob_events/query", params);
      return extractMobStructuralEvents(raw);
    } catch (err) {
      if (isRpcError(err) && err.code === MOB_EVENTS_STALE_CURSOR_CODE) {
        throw MobEventsStaleError.fromRpcError(err);
      }
      throw err;
    }
  }

  /**
   * Replay structural mob events as an async iterator. Yields the
   * snapshot frame returned by `meta/mob_events/subscribe`. Live
   * tailing for production streaming uses the SSE bridge at
   * `/meta/mob_events/stream`.
   *
   * Throws {@link MobEventsStaleError} when `afterSeq` is past the
   * current ledger frontier.
   */
  async *subscribeMobEvents(
    query?: EventQuery,
  ): AsyncGenerator<MobStructuralEvent, void, undefined> {
    const params = query ? eventQueryToDict(query) : {};
    let raw: unknown;
    try {
      raw = await this._runtime._rpc("meta/mob_events/subscribe", params);
    } catch (err) {
      if (isRpcError(err) && err.code === MOB_EVENTS_STALE_CURSOR_CODE) {
        throw MobEventsStaleError.fromRpcError(err);
      }
      throw err;
    }
    for (const event of extractMobStructuralEvents(raw)) {
      yield event;
    }
  }

  // -- Messaging ----------------------------------------------------------

  async send(
    memberId: string,
    message: string | DispatchContentBlock[],
    options?: SendMessageOptions,
  ): Promise<SendMessageResult> {
    const params: Record<string, unknown> = { member_id: memberId };
    if (options?.handlingMode) {
      params.handling_mode = options.handlingMode;
    }
    const uploads = options?.attachments?.map(normalizeBlobUpload) ?? [];
    if (typeof message === "string") {
      if (uploads.length > 0) {
        params.content = [
          ...(message.trim() ? [{ type: "text", text: message }] : []),
          ...uploads.map((upload) => ({
            type: "image_upload",
            upload_id: upload.uploadId,
            media_type: upload.mediaType,
            ...(upload.alt ? { alt: upload.alt } : {}),
          })),
        ];
      } else {
        params.message = message;
      }
    } else {
      params.content = [
        ...message.map(contentBlockToDict),
        ...uploads.map((upload) => ({
          type: "image_upload",
          upload_id: upload.uploadId,
          media_type: upload.mediaType,
          ...(upload.alt ? { alt: upload.alt } : {}),
        })),
      ];
    }
    if (uploads.length > 0) {
      return parseSendMessageResult(
        await this._multipartRpc("meta/send_message", params, uploads),
      );
    }
    return parseSendMessageResult(
      await this._runtime._rpc("meta/send_message", params),
    );
  }

  /** Alias for {@link send}. */
  sendMessage = this.send.bind(this);

  async getBlob(blobId: string): Promise<BlobGetResult> {
    return parseBlobGetResult(
      await this._runtime._rpc("meta/blob/get", { blob_id: blobId }),
    );
  }

  async uploadBlob(file: BlobUploadSource): Promise<BlobUploadResult> {
    const upload = normalizeBlobUpload(file, 0);
    return parseBlobUploadResult(
      await this._multipartRpc(
        "meta/blob/upload",
        {
          upload: {
            type: "image_upload",
            upload_id: upload.uploadId,
            media_type: upload.mediaType,
            ...(upload.alt ? { alt: upload.alt } : {}),
          },
        },
        [upload],
      ),
    );
  }

  async upload_blob(file: BlobUploadSource): Promise<BlobUploadResult> {
    return this.uploadBlob(file);
  }

  private async _multipartRpc(
    method: string,
    params: Record<string, unknown>,
    uploads: readonly NormalizedBlobUpload[],
  ): Promise<unknown> {
    const baseUrl = this._runtime.rustHttpBaseUrl;
    if (baseUrl === null) {
      throw new NotConnectedError(
        "multipart RPC requires rustHttpBaseUrl — start the gateway or call runtime.setRustHttpBase(...)",
      );
    }
    const id = nextRequestId(method);
    const form = new FormData();
    form.append(
      "payload",
      JSON.stringify(buildJsonRpcRequest(id, method, params)),
    );
    for (const upload of uploads) {
      const blob =
        upload.blob.type === upload.mediaType
          ? upload.blob
          : upload.blob.slice(0, upload.blob.size, upload.mediaType);
      form.append(`file:${upload.uploadId}`, blob, upload.filename);
    }
    const response = await fetch(
      `${baseUrl.replace(/\/$/, "")}/console/rpc/multipart`,
      {
        method: "POST",
        body: form,
      },
    );
    const responseText = await response.text();
    let body: Record<string, unknown> | null = null;
    if (responseText.trim() !== "") {
      try {
        body = JSON.parse(responseText) as Record<string, unknown>;
      } catch {
        if (!response.ok) {
          throw new TransportError(
            `multipart RPC failed (status=${response.status}): ${responseText}`,
          );
        }
        throw new TransportError("multipart RPC returned non-JSON response");
      }
    }
    if (!response.ok && body === null) {
      throw new TransportError(
        `multipart RPC failed (status=${response.status}): ${response.statusText}`,
      );
    }
    if (body === null) {
      throw new TransportError("multipart RPC returned an empty response");
    }
    if ("error" in body) {
      throw rpcErrorFromPayload(body.error as Record<string, unknown>, id, method);
    }
    if (!response.ok) {
      throw new TransportError(
        `multipart RPC failed (status=${response.status}): ${responseText}`,
      );
    }
    return body.result;
  }

  async ensureMember(
    memberId: string,
    role: string,
    options?: {
      labels?: Record<string, string>;
      context?: unknown;
      resumeSessionId?: string;
      additionalInstructions?: string[];
      runtimeMode?: "autonomous_host" | "turn_driven";
      backend?: "session" | "external";
      binding?: Record<string, unknown>;
    },
  ): Promise<MemberSnapshot> {
    const params: Record<string, unknown> = {
      role,
      agent_identity: memberId,
    };
    if (options?.labels) params.labels = options.labels;
    if (options?.context !== undefined) params.context = options.context;
    if (options?.resumeSessionId) {
      params.resume_session_id = options.resumeSessionId;
    }
    if (options?.additionalInstructions) {
      params.additional_instructions = options.additionalInstructions;
    }
    if (options?.runtimeMode) params.runtime_mode = options.runtimeMode;
    if (options?.backend) params.backend = options.backend;
    if (options?.binding) params.binding = options.binding;
    return parseMemberSnapshot(
      await this._runtime._rpc("meta/ensure_member", params),
    );
  }

  async findMembers(
    labelKey: string,
    labelValue: string,
  ): Promise<MemberSnapshot[]> {
    const raw = await this._runtime._rpc("meta/find_members", {
      label_key: labelKey,
      label_value: labelValue,
    });
    if (Array.isArray(raw)) {
      return raw.map(parseMemberSnapshot);
    }
    return [];
  }

  // -- Roster -------------------------------------------------------------

  async listMembers(): Promise<MemberSnapshot[]> {
    const raw = await this._runtime._rpc("meta/list_members");
    if (Array.isArray(raw)) {
      return raw.map(parseMemberSnapshot);
    }
    return [];
  }

  async getMember(memberId: string): Promise<MemberSnapshot> {
    return parseMemberSnapshot(
      await this._runtime._rpc("meta/get_member", { member_id: memberId }),
    );
  }

  async retireMember(memberId: string): Promise<void> {
    await this._runtime._rpc("meta/retire_member", {
      member_id: memberId,
    });
  }

  /**
   * Respawn a member. On an identity-first gateway this is DESTRUCTIVE (a
   * fresh session under an advanced generation); to clear a
   * durability-degraded member without losing its transcript use
   * {@link reloadMember}.
   */
  async respawnMember(memberId: string): Promise<void> {
    await this._runtime._rpc("meta/respawn_member", {
      member_id: memberId,
    });
  }

  /**
   * Non-destructive cold reload of an identity's live member: same identity,
   * same durable session id, same generation. The repair for a
   * member whose sends fail with the reload-required class. Identity-first
   * gateways only.
   */
  async reloadMember(memberId: string): Promise<MemberReloadResult> {
    return parseMemberReloadResult(
      await this._runtime._rpc("meta/reload_member", { member_id: memberId }),
    );
  }

  /**
   * Lifecycle and delivery health for one identity, read in-process: unlike
   * {@link memberStatus} it never crosses the mob actor loop, so it answers
   * while the loop is stalled.
   */
  async memberHealth(memberId: string): Promise<MemberHealth> {
    return parseMemberHealth(
      await this._runtime._rpc("meta/member_health", { member_id: memberId }),
    );
  }

  async memberStatus(memberId: string): Promise<RichMemberSnapshot> {
    return parseRichMemberSnapshot(
      await this._runtime._rpc("meta/member_status", { member_id: memberId }),
    );
  }

  async identityResolvedTools(identity: string): Promise<readonly string[]> {
    const result = await this.identityResolvedToolsDetail(identity);
    return result.tools;
  }

  async identityResolvedToolsDetail(identity: string): Promise<IdentityResolvedToolsResult> {
    return parseIdentityResolvedToolsResult(
      await this._runtime._rpc("meta/identity/resolved_tools", { identity }),
    );
  }

  /**
   * Meerkat's typed model-routing status for an identity's live session.
   *
   * Rejects if the runtime machine does not hold the session. The rejection
   * carries a machine-readable `reason` in its error data
   * (`runtime_unsupported`, `no_current_session`, `member_lookup_failed`,
   * `session_not_held`, `upstream_read_failed`, `invalid_identity`) so a
   * fleet sweep can classify an identity rather than only fail it.
   *
   * `no_current_session` covers BOTH an identity that was materialized and
   * never addressed (the normal state after a restart, not a defect) AND an
   * identity that does not exist at all. This surface cannot distinguish them,
   * so a sweep must assert it received a status for every identity it expected
   * rather than merely that nothing threw.
   */
  async identityRoutingStatus(identity: string): Promise<IdentityRoutingStatusResult> {
    return parseIdentityRoutingStatusResult(
      await this._runtime._rpc("meta/identity/routing_status", { identity }),
    );
  }

  /**
   * Force-cancel a running member immediately. A mob refusal rejects with
   * meerkat's typed code and data; a member whose runtime registration must be
   * reloaded first rejects with {@link MemberReloadRequiredError}.
   */
  async forceCancelMember(memberId: string): Promise<void> {
    await this._runtime._rpc("meta/force_cancel_member", {
      member_id: memberId,
    });
  }

  /**
   * Stop one exact run of a member (meerkat's run-fenced Stop). The member's
   * runtime stops `runId` and terminalizes every input already bound to it,
   * including durable steers that joined the run. Resolves to meerkat's typed
   * receipt: `outcome` is `stopped` (with `contributors`), `not_current` (the
   * run is no longer current; nothing was touched), or `not_stoppable`.
   */
  async stopMemberRun(
    memberId: string,
    runId: string,
    reason: string,
  ): Promise<MemberRunStopReceipt> {
    const raw = (await this._runtime._rpc("meta/stop_member_run", {
      member_id: memberId,
      run_id: runId,
      reason,
    })) as { receipt?: unknown };
    const receipt = raw?.receipt as MemberRunStopReceipt | undefined;
    if (
      !receipt ||
      typeof receipt !== "object" ||
      (receipt.outcome !== "stopped" &&
        receipt.outcome !== "not_current" &&
        receipt.outcome !== "not_stoppable")
    ) {
      throw new Error(`invalid meta/stop_member_run receipt: ${JSON.stringify(raw)}`);
    }
    return receipt;
  }

  /**
   * Wait until all current mob members are startup-ready for orchestration.
   *
   * Relays meerkat 0.6's `MobHandle::wait_for_ready`. Returns
   * `{ ready: [...], timeout: false }` on full convergence; on timeout
   * returns `{ ready: [], timeout: true }`. Pass `timeoutSeconds` to bound
   * the wait, or omit it to wait up to a generous server-side default ceiling
   * (~10 minutes); the wait returns as soon as members converge.
   */
  async waitReady(
    timeoutSeconds?: number,
  ): Promise<{ ready: unknown[]; timeout: boolean }> {
    const params: Record<string, unknown> = {};
    if (timeoutSeconds !== undefined) {
      params.timeout_ms = Math.round(timeoutSeconds * 1000);
    }
    const raw = await this._runtime._rpc("meta/wait_ready", params);
    if (typeof raw !== "object" || raw === null) {
      return { ready: [], timeout: false };
    }
    const r = raw as Record<string, unknown>;
    return {
      ready: Array.isArray(r.ready) ? r.ready : [],
      timeout: Boolean(r.timeout),
    };
  }

  // -- Flows --------------------------------------------------------------

  async cancelFlow(runId: string): Promise<void> {
    await this._runtime._rpc("meta/cancel_flow", { run_id: runId });
  }

  async flowStatus(runId: string): Promise<MobRunSnapshot | null> {
    const raw = await this._runtime._rpc("meta/flow_status", {
      run_id: runId,
    });
    if (raw === null) return null;
    if (
      typeof raw === "object" &&
      raw !== null &&
      (raw as Record<string, unknown>).status === "not_found"
    ) {
      return null;
    }
    return parseMobRunSnapshot(raw);
  }

  /**
   * List all configured flow IDs in this mob definition. Relays meerkat
   * 0.6's `MobHandle::list_flows`. Order is unspecified.
   */
  async listFlows(): Promise<string[]> {
    const raw = await this._runtime._rpc("meta/list_flows");
    if (Array.isArray(raw)) {
      return raw.map((id) => String(id));
    }
    if (typeof raw === "object" && raw !== null) {
      const flows = (raw as Record<string, unknown>).flows;
      if (Array.isArray(flows)) {
        return flows.map((id) => String(id));
      }
    }
    return [];
  }

  /**
   * List flow runs for this mob, optionally filtered to one `flowId`.
   * Relays `MobHandle::list_runs`. Each {@link MobRun} carries the full
   * meerkat ledger projection — `step_ledger`, `failure_ledger`,
   * `frames`, `loops`, `loop_iteration_ledger`, `flow_state`,
   * `activation_params`, etc. — verbatim from the wire JSON.
   */
  async listRuns(flowId?: string): Promise<MobRun[]> {
    const params: Record<string, unknown> = {};
    if (flowId !== undefined) params.flow_id = flowId;
    const raw = await this._runtime._rpc("meta/list_runs", params);
    let runs: unknown[] = [];
    if (Array.isArray(raw)) {
      runs = raw;
    } else if (typeof raw === "object" && raw !== null) {
      const maybe = (raw as Record<string, unknown>).runs;
      if (Array.isArray(maybe)) {
        runs = maybe;
      }
    }
    const result: MobRun[] = [];
    for (const entry of runs) {
      if (typeof entry === "object" && entry !== null) {
        result.push(parseMobRun(entry as Record<string, unknown>));
      }
    }
    return result;
  }

  /**
   * Start a flow run and return its run ID. Relays meerkat 0.6's
   * `MobHandle::run_flow`. `params` is forwarded verbatim as the flow's
   * activation params (any JSON value).
   *
   * `options.roleTargets` binds every role the flow's steps use to existing
   * members of this mob, for example `{reviewer: ["rev-a1", "rev-a2"]}`.
   * Each step then dispatches only to the bound members of its role that are
   * still active, never to another member of the role, and the run fails
   * (`insufficient_targets`) when they can no longer meet the step's
   * collection policy. The gateway must advertise the `flow_role_targets`
   * feature capability: without it this throws
   * {@link CapabilityUnavailableError} and starts nothing, since an older
   * gateway would ignore the binding and run the flow role-wide. A binding
   * the mob refuses throws {@link FlowRoleTargetsRefusedError}. A run the
   * gateway started without echoing the same binding is canceled, and
   * {@link FlowRoleTargetsNotAppliedError} is thrown.
   *
   * `options.idempotencyKey` admits the run once under a caller key: a replay
   * with the same key, params and role targets resolves to the same run,
   * also when the first answer was lost, and the same key with other params
   * or another binding throws {@link FlowRunIdempotencyConflictError} without
   * a second run. The gateway must advertise the `flow_run_idempotency_key`
   * feature capability: without it this throws
   * {@link CapabilityUnavailableError} and starts nothing, since an older
   * gateway would ignore the key.
   */
  async runFlow(
    flowId: string,
    params: unknown = null,
    options?: {
      readonly roleTargets?: Readonly<Record<string, readonly string[]>>;
      readonly idempotencyKey?: string;
    },
  ): Promise<string> {
    const rpcParams: Record<string, unknown> = { flow_id: flowId, params };
    let requested: Record<string, string[]> | null = null;
    if (options?.roleTargets !== undefined) {
      requested = roleTargetsRequest(options.roleTargets);
    }
    const idempotencyKey = options?.idempotencyKey;
    if (idempotencyKey !== undefined && typeof idempotencyKey !== "string") {
      throw new TypeError("idempotencyKey must be a string");
    }
    if (requested !== null || idempotencyKey !== undefined) {
      const capabilities = await this.capabilities();
      let missing: string | null = null;
      if (requested !== null && !supportsFlowRoleTargets(capabilities)) {
        missing = FLOW_ROLE_TARGETS;
      } else if (
        idempotencyKey !== undefined &&
        !supportsFlowRunIdempotencyKey(capabilities)
      ) {
        missing = FLOW_RUN_IDEMPOTENCY_KEY;
      }
      if (missing !== null) {
        throw new CapabilityUnavailableError(
          `capability ${missing} is not available`,
          "",
          "meta/run_flow",
          { capability: missing },
        );
      }
    }
    if (requested !== null) {
      rpcParams.role_targets = requested;
    }
    if (idempotencyKey !== undefined) {
      rpcParams.idempotency_key = idempotencyKey;
    }
    const raw = await this._runtime._rpc("meta/run_flow", rpcParams);
    const result =
      typeof raw === "object" && raw !== null
        ? (raw as Record<string, unknown>)
        : null;
    const runId = result?.run_id;
    if (result === null || typeof runId !== "string") {
      throw new Error(`unexpected run_flow response: ${JSON.stringify(raw)}`);
    }
    if (requested !== null) {
      const echoed = result.role_targets;
      if (!roleTargetsEchoed(requested, echoed)) {
        let cancelError: unknown = null;
        try {
          await this.cancelFlow(runId);
        } catch (error) {
          cancelError = error;
        }
        throw new FlowRoleTargetsNotAppliedError(
          `meta/run_flow started run ${runId} without echoing its role_targets; ` +
            (cancelError === null
              ? "the run was canceled"
              : `canceling it failed: ${String(cancelError)}`),
          runId,
          requested,
          echoed,
          cancelError,
        );
      }
    }
    return runId;
  }

  async collectCompleted(): Promise<Array<[string, RichMemberSnapshot]>> {
    const raw = await this._runtime._rpc("meta/collect_completed");
    const entries =
      typeof raw === "object" && raw !== null
        ? Array.isArray((raw as Record<string, unknown>).completed)
          ? ((raw as Record<string, unknown>).completed as unknown[])
          : []
        : Array.isArray(raw)
          ? raw
          : [];
    const result: Array<[string, RichMemberSnapshot]> = [];
    for (const entry of entries) {
      const record =
        typeof entry === "object" && entry !== null
          ? (entry as Record<string, unknown>)
          : {};
      const memberId = String(record.member_id ?? "");
      result.push([
        memberId,
        parseRichMemberSnapshot(record.snapshot ?? record),
      ]);
    }
    return result;
  }

  // -- Routing ------------------------------------------------------------

  async resolveRouting(
    recipient: string,
    options?: Record<string, unknown>,
  ): Promise<RoutingResolution> {
    return parseRoutingResolution(
      await this._runtime._rpc("meta/routing/resolve", {
        recipient,
        ...(options ?? {}),
      }),
    );
  }

  async listRoutes(): Promise<RuntimeRouteResult[]> {
    const raw = await this._runtime._rpc("meta/routing/routes/list");
    const routes =
      typeof raw === "object" && raw !== null
        ? (((raw as Record<string, unknown>).routes as unknown[]) ?? [])
        : [];
    return (routes as unknown[]).map(parseRuntimeRouteResult);
  }

  async addRoute(
    routeKey: string,
    recipient: string,
    sink: string,
    targetModule: string,
    channel?: string,
  ): Promise<RuntimeRouteResult> {
    const params: Record<string, unknown> = {
      route_key: routeKey,
      recipient,
      sink,
      target_module: targetModule,
    };
    if (channel !== undefined) params.channel = channel;
    const raw = await this._runtime._rpc("meta/routing/routes/add", params);
    const routeData =
      typeof raw === "object" && raw !== null
        ? ((raw as Record<string, unknown>).route ?? raw)
        : raw;
    return parseRuntimeRouteResult(routeData);
  }

  async deleteRoute(routeKey: string): Promise<RuntimeRouteResult> {
    const raw = await this._runtime._rpc("meta/routing/routes/delete", {
      route_key: routeKey,
    });
    const deletedData =
      typeof raw === "object" && raw !== null
        ? ((raw as Record<string, unknown>).deleted ?? raw)
        : raw;
    return parseRuntimeRouteResult(deletedData);
  }

  // -- Delivery -----------------------------------------------------------

  async sendDelivery(
    options: Record<string, unknown>,
  ): Promise<DeliveryResult> {
    return parseDeliveryResult(
      await this._runtime._rpc("meta/delivery/send", options),
    );
  }

  async deliveryHistory(
    recipient?: string,
    sink?: string,
    limit = 20,
  ): Promise<DeliveryHistoryResult> {
    const params: Record<string, unknown> = { limit };
    if (recipient !== undefined) params.recipient = recipient;
    if (sink !== undefined) params.sink = sink;
    return parseDeliveryHistoryResult(
      await this._runtime._rpc("meta/delivery/history", params),
    );
  }

  // -- Memory -------------------------------------------------------------

  /**
   * Query the operational memory assertion ledger.
   *
   * Pass `{ entity, topic, store }` for the Rust gateway's exact-filter
   * contract. The string overload is forwarded as `query`, which the gateway
   * applies as a case-insensitive substring filter across entity, topic, and
   * fact (reason for conflict signals), after the exact filters.
   */
  async memoryQuery(
    queryOrOptions?: string | {
      entity?: string;
      topic?: string;
      store?: string;
    },
    options?: Record<string, unknown>,
  ): Promise<MemoryQueryResult> {
    const params: Record<string, unknown> =
      typeof queryOrOptions === "string"
        ? { query: queryOrOptions, ...(options ?? {}) }
        : { ...(queryOrOptions ?? {}) };
    return parseMemoryQueryResult(
      await this._runtime._rpc("meta/memory/query", params),
    );
  }

  async memoryStores(): Promise<MemoryStoreInfo[]> {
    const raw = await this._runtime._rpc("meta/memory/stores");
    const stores =
      typeof raw === "object" && raw !== null
        ? (((raw as Record<string, unknown>).stores as unknown[]) ?? [])
        : [];
    return (stores as unknown[]).map(parseMemoryStoreInfo);
  }

  async memoryIndex(
    entity: string,
    topic: string,
    store: string,
    options?: Record<string, unknown>,
  ): Promise<MemoryIndexResult> {
    return parseMemoryIndexResult(
      await this._runtime._rpc("meta/memory/index", {
        entity,
        topic,
        store,
        ...(options ?? {}),
      }),
    );
  }

  async rememberAgentMemory(
    identity: string,
    memory: RememberAgentMemoryOptions,
  ): Promise<AgentMemoryRecord> {
    const params: Record<string, unknown> = {
      identity,
      title: memory.title,
      body: memory.body,
    };
    if (memory.realm !== undefined) params.realm = memory.realm;
    if (memory.tags !== undefined) params.tags = [...memory.tags];
    return parseAgentMemoryRecord(
      await this._runtime._rpc("meta/agent_memory/remember", params),
    );
  }

  async recallAgentMemory(
    identity: string,
    options: RecallAgentMemoryOptions = {},
  ): Promise<AgentMemoryRecord[]> {
    const params: Record<string, unknown> = { identity };
    if (options.realm !== undefined) params.realm = options.realm;
    if (options.selection !== undefined) params.selection = options.selection;
    if (options.queryText !== undefined) params.query_text = options.queryText;
    if (options.queryTerms !== undefined) params.query_terms = [...options.queryTerms];
    if (options.maxEntries !== undefined) params.max_entries = options.maxEntries;
    const result = parseAgentMemoryRecallResult(
      await this._runtime._rpc("meta/agent_memory/recall", params),
    );
    return [...result.records];
  }

  async forgetAgentMemory(
    identity: string,
    memoryId: string,
    options: ForgetAgentMemoryOptions = {},
  ): Promise<AgentMemoryForgetResult> {
    const params: Record<string, unknown> = { identity, memory_id: memoryId };
    if (options.realm !== undefined) params.realm = options.realm;
    return parseAgentMemoryForgetResult(
      await this._runtime._rpc("meta/agent_memory/forget", params),
    );
  }

  /**
   * Supersede a durable memory record within its lineage: the new
   * title/body/tags become the active record; the prior stays retrievable
   * with provenance and is no longer recalled.
   */
  async updateAgentMemory(
    identity: string,
    memoryId: string,
    memory: UpdateAgentMemoryOptions,
  ): Promise<AgentMemoryUpdateResult> {
    const params: Record<string, unknown> = {
      identity,
      memory_id: memoryId,
      title: memory.title,
      body: memory.body,
    };
    if (memory.realm !== undefined) params.realm = memory.realm;
    if (memory.tags !== undefined) params.tags = [...memory.tags];
    return parseAgentMemoryUpdateResult(
      await this._runtime._rpc("meta/agent_memory/update", params),
    );
  }

  /**
   * List durable memory record metadata (id/kind/title/description/age/rank
   * — never bodies). Tier "working_set" (default) returns the top-K ranked
   * records plus the recent/unranked slice; "full" returns everything.
   */
  async manifestAgentMemory(
    identity: string,
    options: ManifestAgentMemoryOptions = {},
  ): Promise<AgentMemoryRecordMeta[]> {
    const params: Record<string, unknown> = { identity };
    if (options.realm !== undefined) params.realm = options.realm;
    if (options.tier !== undefined) params.tier = options.tier;
    if (options.k !== undefined) params.k = options.k;
    const result = parseAgentMemoryManifestResult(
      await this._runtime._rpc("meta/agent_memory/manifest", params),
    );
    return [...result.records];
  }

  // -- Tools --------------------------------------------------------------

  async callTool(
    moduleId: string,
    tool: string,
    args?: Record<string, unknown>,
  ): Promise<CallToolResult> {
    const params: Record<string, unknown> = { module_id: moduleId, tool };
    if (args) params.arguments = args;
    return parseCallToolResult(
      await this._runtime._rpc("meta/call_tool", params),
    );
  }

  toolCaller(moduleId: string): ToolCaller {
    return new ToolCaller(this, moduleId);
  }

  async sessionStoreBigQuery(
    options: Record<string, unknown>,
  ): Promise<unknown> {
    return this._runtime._rpc("meta/session_store/bigquery", options);
  }

  // -- Gating -------------------------------------------------------------

  async gatingEvaluate(
    action: string,
    actorId: string,
    options?: Record<string, unknown>,
  ): Promise<GatingEvaluateResult> {
    return parseGatingEvaluateResult(
      await this._runtime._rpc("meta/gating/evaluate", {
        action,
        actor_id: actorId,
        ...(options ?? {}),
      }),
    );
  }

  async gatingPending(): Promise<GatingPendingEntry[]> {
    const raw = await this._runtime._rpc("meta/gating/pending");
    const entries =
      typeof raw === "object" && raw !== null
        ? (((raw as Record<string, unknown>).pending as unknown[]) ?? [])
        : [];
    return (entries as unknown[]).map(parseGatingPendingEntry);
  }

  async gatingDecide(
    pendingId: string,
    decision: string,
    approverId: string,
    options?: Record<string, unknown>,
  ): Promise<GatingDecisionResult> {
    return parseGatingDecisionResult(
      await this._runtime._rpc("meta/gating/decide", {
        pending_id: pendingId,
        decision,
        approver_id: approverId,
        ...(options ?? {}),
      }),
    );
  }

  async gatingAudit(limit = 100): Promise<GatingAuditEntry[]> {
    const raw = await this._runtime._rpc("meta/gating/audit", { limit });
    const entries =
      typeof raw === "object" && raw !== null
        ? (((raw as Record<string, unknown>).entries as unknown[]) ?? [])
        : [];
    return (entries as unknown[]).map(parseGatingAuditEntry);
  }

  // -- WorkGraph ------------------------------------------------------------
  //
  // Goals, work items, and attention bindings (`meta/workgraph/*`). Wire
  // params are snake_case; option objects here are typed camelCase and
  // converted at the call site. See docs/design/workgraph-wire-contract.md.

  async workgraphSnapshot(
    options?: WorkGraphFilterOptions,
  ): Promise<WorkGraphSnapshotResult> {
    return parseWorkGraphSnapshotResult(
      await this._runtime._rpc(
        "meta/workgraph/snapshot",
        workGraphFilterOptionsToDict(options),
      ),
    );
  }

  async workgraphList(
    options?: WorkGraphFilterOptions,
  ): Promise<WorkGraphItem[]> {
    const result = parseWorkGraphItemsResult(
      await this._runtime._rpc(
        "meta/workgraph/list",
        workGraphFilterOptionsToDict(options),
      ),
    );
    return [...result.items];
  }

  async workgraphGet(
    id: string,
    options?: { namespace?: string },
  ): Promise<WorkGraphItem> {
    const params: Record<string, unknown> = { id };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc("meta/workgraph/get", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphReady(
    options?: WorkGraphReadyOptions,
  ): Promise<WorkGraphItem[]> {
    const result = parseWorkGraphItemsResult(
      await this._runtime._rpc(
        "meta/workgraph/ready",
        workGraphReadyOptionsToDict(options),
      ),
    );
    return [...result.items];
  }

  async workgraphEvents(
    options?: WorkGraphEventsOptions,
  ): Promise<WorkGraphEventEntry[]> {
    const raw = await this._runtime._rpc(
      "meta/workgraph/events",
      workGraphEventsOptionsToDict(options),
    );
    const events = asWireRecord(raw).events;
    return (Array.isArray(events) ? events : []).map(parseWorkGraphEventEntry);
  }

  async workgraphAttentionList(
    options?: WorkGraphAttentionListOptions,
  ): Promise<WorkGraphAttentionBinding[]> {
    const raw = await this._runtime._rpc(
      "meta/workgraph/attention/list",
      workGraphAttentionListOptionsToDict(options),
    );
    const attention = asWireRecord(raw).attention;
    return (Array.isArray(attention) ? attention : []).map(
      parseWorkGraphAttentionBinding,
    );
  }

  async workgraphGoalStatus(
    bindingId: string,
    options?: { namespace?: string },
  ): Promise<WorkGraphGoalResult> {
    const params: Record<string, unknown> = { binding_id: bindingId };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    return parseWorkGraphGoalResult(
      await this._runtime._rpc("meta/workgraph/goal/status", params),
    );
  }

  async workgraphCreate(
    title: string,
    options?: WorkGraphCreateOptions,
  ): Promise<WorkGraphItem> {
    const params = workGraphCreateOptionsToDict(options);
    params.title = title;
    const raw = await this._runtime._rpc("meta/workgraph/create", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphUpdate(
    id: string,
    expectedRevision: number,
    options?: WorkGraphUpdateOptions,
  ): Promise<WorkGraphItem> {
    const params = workGraphUpdateOptionsToDict(options);
    params.id = id;
    params.expected_revision = expectedRevision;
    const raw = await this._runtime._rpc("meta/workgraph/update", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphClaim(
    id: string,
    expectedRevision: number,
    owner: WorkGraphOwnerInput,
    options?: WorkGraphClaimOptions,
  ): Promise<WorkGraphItem> {
    const params = workGraphClaimOptionsToDict(options);
    params.id = id;
    params.expected_revision = expectedRevision;
    params.owner = workGraphOwnerInputToDict(owner);
    const raw = await this._runtime._rpc("meta/workgraph/claim", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphRelease(
    id: string,
    expectedRevision: number,
    options?: { namespace?: string },
  ): Promise<WorkGraphItem> {
    const params: Record<string, unknown> = {
      id,
      expected_revision: expectedRevision,
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc("meta/workgraph/release", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphClose(
    id: string,
    expectedRevision: number,
    options?: WorkGraphCloseOptions,
  ): Promise<WorkGraphItem> {
    const params = workGraphCloseOptionsToDict(options);
    params.id = id;
    params.expected_revision = expectedRevision;
    const raw = await this._runtime._rpc("meta/workgraph/close", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphBlock(
    id: string,
    expectedRevision: number,
    options?: { namespace?: string },
  ): Promise<WorkGraphItem> {
    const params: Record<string, unknown> = {
      id,
      expected_revision: expectedRevision,
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc("meta/workgraph/block", params);
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphLink(
    kind: string,
    fromId: string,
    toId: string,
    options?: { namespace?: string },
  ): Promise<WorkGraphEdge> {
    const params: Record<string, unknown> = {
      kind,
      from_id: fromId,
      to_id: toId,
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc("meta/workgraph/link", params);
    return parseWorkGraphEdge(asWireRecord(raw).edge);
  }

  async workgraphAddEvidence(
    id: string,
    expectedRevision: number,
    evidence: WorkGraphEvidenceInput,
    options?: { namespace?: string },
  ): Promise<WorkGraphItem> {
    const params: Record<string, unknown> = {
      id,
      expected_revision: expectedRevision,
      evidence: workGraphEvidenceInputToDict(evidence),
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc(
      "meta/workgraph/evidence/add",
      params,
    );
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphEscalatePolicy(
    bindingId: string,
    id: string,
    expectedRevision: number,
    completionPolicy: unknown,
    options?: { namespace?: string },
  ): Promise<WorkGraphItem> {
    const params: Record<string, unknown> = {
      binding_id: bindingId,
      id,
      expected_revision: expectedRevision,
      completion_policy: completionPolicy,
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc(
      "meta/workgraph/policy/escalate",
      params,
    );
    return parseWorkGraphItem(asWireRecord(raw).item);
  }

  async workgraphGoalCreate(
    title: string,
    target: WorkGraphGoalTarget,
    options?: WorkGraphGoalCreateOptions,
  ): Promise<WorkGraphGoalResult> {
    const params = workGraphGoalCreateOptionsToDict(options);
    params.title = title;
    params.target = workGraphGoalTargetToDict(target);
    return parseWorkGraphGoalResult(
      await this._runtime._rpc("meta/workgraph/goal/create", params),
    );
  }

  async workgraphGoalConfirm(
    bindingId: string,
    expectedRevision: number,
    options?: WorkGraphGoalConfirmOptions,
  ): Promise<WorkGraphGoalResult> {
    const params = workGraphGoalConfirmOptionsToDict(options);
    params.binding_id = bindingId;
    params.expected_revision = expectedRevision;
    return parseWorkGraphGoalResult(
      await this._runtime._rpc("meta/workgraph/goal/confirm", params),
    );
  }

  async workgraphGoalRequestClose(
    bindingId: string,
    expectedRevision: number,
    options?: WorkGraphGoalRequestCloseOptions,
  ): Promise<WorkGraphGoalResult> {
    const params = workGraphGoalRequestCloseOptionsToDict(options);
    params.binding_id = bindingId;
    params.expected_revision = expectedRevision;
    return parseWorkGraphGoalResult(
      await this._runtime._rpc("meta/workgraph/goal/request_close", params),
    );
  }

  async workgraphAttentionPause(
    bindingId: string,
    expectedRevision: number,
    options?: WorkGraphAttentionPauseOptions,
  ): Promise<WorkGraphAttentionBinding> {
    const params = workGraphAttentionPauseOptionsToDict(options);
    params.binding_id = bindingId;
    params.expected_revision = expectedRevision;
    const raw = await this._runtime._rpc(
      "meta/workgraph/attention/pause",
      params,
    );
    return parseWorkGraphAttentionBinding(asWireRecord(raw).attention);
  }

  async workgraphAttentionResume(
    bindingId: string,
    expectedRevision: number,
    options?: { namespace?: string },
  ): Promise<WorkGraphAttentionBinding> {
    const params: Record<string, unknown> = {
      binding_id: bindingId,
      expected_revision: expectedRevision,
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc(
      "meta/workgraph/attention/resume",
      params,
    );
    return parseWorkGraphAttentionBinding(asWireRecord(raw).attention);
  }

  async workgraphAttentionReassign(
    bindingId: string,
    expectedRevision: number,
    target: WorkGraphGoalTarget,
    options?: { namespace?: string },
  ): Promise<WorkGraphAttentionReassignResult> {
    const params: Record<string, unknown> = {
      binding_id: bindingId,
      expected_revision: expectedRevision,
      target: workGraphGoalTargetToDict(target),
    };
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    return parseWorkGraphAttentionReassignResult(
      await this._runtime._rpc("meta/workgraph/attention/reassign", params),
    );
  }

  /**
   * Prune TERMINAL (superseded/stopped) attention binding rows. The
   * workgraph event stream keeps the audit history; binding rows otherwise
   * grow monotonically with reassignment churn. Pass an RFC3339
   * `updatedBefore` to prune only rows last updated strictly before that
   * instant. Returns the number of rows pruned.
   */
  async workgraphAttentionPrune(options?: {
    updatedBefore?: string;
    namespace?: string;
  }): Promise<number> {
    const params: Record<string, unknown> = {};
    if (options?.updatedBefore !== undefined) {
      params.updated_before = options.updatedBefore;
    }
    if (options?.namespace !== undefined) params.namespace = options.namespace;
    const raw = await this._runtime._rpc(
      "meta/workgraph/attention/prune",
      params,
    );
    const pruned = asWireRecord(raw).pruned;
    return typeof pruned === "number" ? pruned : 0;
  }

  // -- Mob announcements ---------------------------------------------------
  //
  // `meta/mob_announcements/*`. Option objects are camelCase; the wire objects
  // a caller declares (a source's identity, observation, audience, routes,
  // ...) are sent as given, in wire (snake_case) form.

  private async _announcementRpc(
    method: string,
    params: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const full = `meta/mob_announcements/${method}`;
    let raw: unknown;
    try {
      raw = await this._runtime._rpc(full, definedParams(params));
    } catch (err) {
      if (
        isRpcError(err) &&
        err.code === JSONRPC_METHOD_NOT_FOUND &&
        !ANNOUNCEMENT_METHODS_BEFORE_SOURCES.has(full)
      ) {
        throw new RpcError(
          err.code,
          `Meta does not support sources: the gateway has no ${full}`,
          err.requestId,
          err.method || full,
          err.data,
        );
      }
      throw err;
    }
    return asWireRecord(raw);
  }

  /**
   * Announce as the host into a mob hosted here (this gateway's own by
   * default). `sourceKey` is the announcement's stable identity in its mob:
   * the same key with the same content replays (`replayed`), with other
   * content it is refused ({@link AnnouncementConflictError}).
   */
  async announce(
    sourceKey: string,
    topics: readonly string[],
    body: string,
    options: {
      readonly mobId?: string;
      readonly correlationId?: string;
      readonly causes?: readonly AnnouncementRef[];
    } = {},
  ): Promise<AnnounceResult> {
    return parseAnnounceResult(
      await this._announcementRpc("announce", {
        source_key: sourceKey,
        topics: [...topics],
        body,
        mob_id: options.mobId,
        correlation_id: options.correlationId,
        causes: options.causes?.map((cause) => ({ mob_id: cause.mobId, source_key: cause.sourceKey })),
      }),
    );
  }

  /** One announcement by its mob and source key, or `null`. */
  async announcementGet(mobId: string, sourceKey: string): Promise<Announcement | null> {
    const raw = await this._announcementRpc("get", { mob_id: mobId, source_key: sourceKey });
    return typeof raw.announcement === "object" && raw.announcement !== null
      ? parseAnnouncement(raw.announcement)
      : null;
  }

  /**
   * Create a subscription, or replace its selector at `expectedRevision`.
   * `mobs` are the mobs hosted here whose announcements it takes (this
   * gateway's own when absent); `start` is its start frontier, as the wire
   * takes it (now by default). `delivery` is `{await_handling: boolean}`:
   * whether the member's next batch waits until the turn of its previous one
   * has ended, for at most 15 minutes; left out, the gateway decides (on for
   * the triage member of a triage route that takes a selected topic, off
   * otherwise). `operationId` makes a retry safe.
   */
  async announcementSubscriptionPut(
    operationId: string,
    subscriptionId: string,
    member: AnnouncementMemberParam,
    topics: readonly string[],
    options: {
      readonly mobs?: readonly string[];
      readonly start?: unknown;
      readonly expectedRevision?: number;
      readonly delivery?: AnnouncementWireObject;
    } = {},
  ): Promise<AnnouncementSubscription> {
    const raw = await this._announcementRpc("subscriptions/put", {
      operation_id: operationId,
      subscription_id: subscriptionId,
      member: announcementMemberToWire(member),
      topics: [...topics],
      mobs: options.mobs === undefined ? undefined : [...options.mobs],
      start: options.start,
      expected_revision: options.expectedRevision,
      delivery: options.delivery === undefined ? undefined : { ...options.delivery },
    });
    return parseAnnouncementSubscription(raw.subscription);
  }

  /**
   * Create a role template, or replace its selector at `expectedRevision`:
   * `subscriptions/put` with `members` (`{mob_id, role}`) instead of one
   * member. Each member of the mob with the role captures through a child
   * subscription, made at the first announcement it matches. A template
   * starts now. Its id is at most 200 bytes, and a subscription and a role
   * template never share an id (`subscription_id_taken`). The subscription
   * verbs and `announcementSubscriptionGet` take its id. `delivery`
   * (`{await_handling: boolean}`) is every child's, as for
   * `announcementSubscriptionPut`; it is sent only when set.
   */
  async announcementRoleTemplatePut(
    operationId: string,
    templateId: string,
    members: AnnouncementRoleSelectorParam,
    topics: readonly string[],
    options: {
      readonly mobs?: readonly string[];
      readonly expectedRevision?: number;
      readonly delivery?: AnnouncementWireObject;
    } = {},
  ): Promise<AnnouncementRoleTemplate> {
    const raw = await this._announcementRpc("subscriptions/put", {
      operation_id: operationId,
      subscription_id: templateId,
      members: announcementRoleSelectorToWire(members),
      topics: [...topics],
      mobs: options.mobs === undefined ? undefined : [...options.mobs],
      expected_revision: options.expectedRevision,
      delivery: options.delivery === undefined ? undefined : { ...options.delivery },
    });
    return parseAnnouncementRoleTemplate(raw.subscription);
  }

  private async _announcementSubscriptionChange(
    change: "pause" | "resume" | "cancel",
    operationId: string,
    subscriptionId: string,
    expectedRevision: number,
  ): Promise<AnnouncementSubscriptionChange> {
    return parseAnnouncementSubscriptionChange(
      await this._announcementRpc(`subscriptions/${change}`, {
        operation_id: operationId,
        subscription_id: subscriptionId,
        expected_revision: expectedRevision,
      }),
    );
  }

  /** Pause a subscription: it keeps capturing and delivers nothing new until resumed. */
  async announcementSubscriptionPause(
    operationId: string,
    subscriptionId: string,
    expectedRevision: number,
  ): Promise<AnnouncementSubscriptionChange> {
    return this._announcementSubscriptionChange("pause", operationId, subscriptionId, expectedRevision);
  }

  /** Resume a paused subscription. */
  async announcementSubscriptionResume(
    operationId: string,
    subscriptionId: string,
    expectedRevision: number,
  ): Promise<AnnouncementSubscriptionChange> {
    return this._announcementSubscriptionChange("resume", operationId, subscriptionId, expectedRevision);
  }

  /**
   * Cancel a subscription; terminal. Sealed batches settle from their own
   * native evidence (`unresolvedBatches`).
   */
  async announcementSubscriptionCancel(
    operationId: string,
    subscriptionId: string,
    expectedRevision: number,
  ): Promise<AnnouncementSubscriptionChange> {
    return this._announcementSubscriptionChange("cancel", operationId, subscriptionId, expectedRevision);
  }

  /** A subscription (`null` when unknown) and why its member's deliveries wait, if they do. */
  async announcementSubscriptionGet(subscriptionId: string): Promise<AnnouncementSubscriptionView> {
    return parseAnnouncementSubscriptionView(
      await this._announcementRpc("subscriptions/get", { subscription_id: subscriptionId }),
    );
  }

  /** A page of subscriptions, optionally one member's, after a subscription id. */
  async announcementSubscriptionList(
    options: { readonly member?: AnnouncementMemberParam; readonly after?: string; readonly limit?: number } = {},
  ): Promise<AnnouncementSubscription[]> {
    const raw = await this._announcementRpc("subscriptions/list", {
      member: options.member === undefined ? undefined : announcementMemberToWire(options.member),
      after: options.after,
      limit: options.limit,
    });
    return Array.isArray(raw.subscriptions) ? raw.subscriptions.map(parseAnnouncementSubscription) : [];
  }

  /** A page of delivery batches, by member or subscription. */
  async announcementDeliveryList(
    options: {
      readonly member?: AnnouncementMemberParam;
      readonly subscriptionId?: string;
      readonly after?: string;
      readonly limit?: number;
    } = {},
  ): Promise<AnnouncementBatch[]> {
    const raw = await this._announcementRpc("deliveries/list", {
      member: options.member === undefined ? undefined : announcementMemberToWire(options.member),
      subscription_id: options.subscriptionId,
      after: options.after,
      limit: options.limit,
    });
    return Array.isArray(raw.deliveries) ? raw.deliveries.map(parseAnnouncementBatch) : [];
  }

  /** Read a batch and its native delivery status live, without changing anything. */
  async announcementDeliveryObserve(batchId: string): Promise<AnnouncementDeliveryObservation> {
    return parseAnnouncementDeliveryObservation(
      await this._announcementRpc("deliveries/observe", { batch_id: batchId }),
    );
  }

  /** Read a batch's native delivery status and settle the batch from that evidence. */
  async announcementDeliveryReconcile(batchId: string): Promise<AnnouncementDeliveryObservation> {
    return parseAnnouncementDeliveryObservation(
      await this._announcementRpc("deliveries/reconcile", { batch_id: batchId }),
    );
  }

  /**
   * Owe a batch's held items again, for the member's next batch, after a
   * fresh coverage check; only a batch whose handling is `unhandled` with
   * held items (`not_held` otherwise).
   */
  async announcementDeliveryRedeliver(operationId: string, batchId: string): Promise<AnnouncementBatchHandling> {
    const raw = await this._announcementRpc("deliveries/redeliver", { operation_id: operationId, batch_id: batchId });
    return parseAnnouncementBatchHandling(raw.handling);
  }

  /**
   * Settle a batch's held items as handled by an operator; only a batch
   * whose handling is `unhandled` with held items (`not_held` otherwise).
   */
  async announcementDeliveryDismiss(operationId: string, batchId: string): Promise<AnnouncementBatchHandling> {
    const raw = await this._announcementRpc("deliveries/dismiss", { operation_id: operationId, batch_id: batchId });
    return parseAnnouncementBatchHandling(raw.handling);
  }

  /**
   * Declare a source, replace its declaration at `expectedRevision`, or start
   * its next generation (`newGeneration`). `identity` is
   * `{provider, principal, scope}`, `observation` is `{baseline, deletions}`
   * and `audience` lists who may receive its items, all in wire form.
   * `routes` defaults to one direct route. `runner`
   * (`{host_runnable, every_seconds}`) names the host runnable a schedule
   * service runs to collect it.
   */
  async announcementSourcePut(
    operationId: string,
    sourceId: string,
    identity: AnnouncementWireObject,
    observation: AnnouncementWireObject,
    topics: readonly string[],
    audience: readonly AnnouncementWireObject[],
    options: {
      readonly mobId?: string;
      readonly expectedRevision?: number;
      readonly newGeneration?: boolean;
      readonly connection?: AnnouncementWireObject;
      readonly routes?: readonly AnnouncementWireObject[];
      readonly conversationMembers?: readonly AnnouncementMemberParam[];
      readonly start?: AnnouncementWireObject;
      readonly delivery?: AnnouncementWireObject;
      readonly healthPolicy?: AnnouncementWireObject;
      readonly runner?: AnnouncementWireObject;
    } = {},
  ): Promise<AnnouncementSourcePutResult> {
    return parseAnnouncementSourcePutResult(
      await this._announcementRpc("sources/put", {
        operation_id: operationId,
        source_id: sourceId,
        identity,
        observation,
        topics: [...topics],
        audience: [...audience],
        mob_id: options.mobId,
        expected_revision: options.expectedRevision,
        new_generation: options.newGeneration,
        connection: options.connection,
        routes: options.routes === undefined ? undefined : [...options.routes],
        conversation_members: options.conversationMembers?.map(announcementMemberToWire),
        start: options.start,
        delivery: options.delivery,
        health_policy: options.healthPolicy,
        runner: options.runner,
      }),
    );
  }

  private async _announcementSourceChange(
    change: "pause" | "resume" | "end",
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    extra: Record<string, unknown>,
  ): Promise<AnnouncementSource> {
    const raw = await this._announcementRpc(`sources/${change}`, {
      operation_id: operationId,
      source_id: sourceId,
      expected_revision: expectedRevision,
      ...extra,
    });
    return parseAnnouncementSource(raw.source);
  }

  /** Pause a source: it takes no collector step until resumed and keeps its cursor. */
  async announcementSourcePause(
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    options: { readonly mobId?: string } = {},
  ): Promise<AnnouncementSource> {
    return this._announcementSourceChange("pause", operationId, sourceId, expectedRevision, {
      mob_id: options.mobId,
    });
  }

  /** Resume a paused source from its kept cursor, or from a fresh baseline with `rebaseline`. */
  async announcementSourceResume(
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    options: { readonly mobId?: string; readonly rebaseline?: boolean } = {},
  ): Promise<AnnouncementSource> {
    return this._announcementSourceChange("resume", operationId, sourceId, expectedRevision, {
      mob_id: options.mobId,
      rebaseline: options.rebaseline,
    });
  }

  /** End a source's generation. A later declaration starts the next one. */
  async announcementSourceEnd(
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    options: { readonly mobId?: string } = {},
  ): Promise<AnnouncementSource> {
    return this._announcementSourceChange("end", operationId, sourceId, expectedRevision, {
      mob_id: options.mobId,
    });
  }

  /**
   * One source (`null` when the mob has none by that id), with its cursor
   * token and failure detail, and its streams when `includeStreams` is set.
   */
  async announcementSourceGet(
    sourceId: string,
    options: { readonly mobId?: string; readonly includeStreams?: boolean } = {},
  ): Promise<AnnouncementSourceView> {
    return parseAnnouncementSourceView(
      await this._announcementRpc("sources/get", {
        source_id: sourceId,
        mob_id: options.mobId,
        include_streams: options.includeStreams,
      }),
    );
  }

  /**
   * A page of a mob's sources. A list shows each principal as its kind and
   * digest only, and neither cursor tokens nor failure details.
   */
  async announcementSourceList(
    options: {
      readonly mobId?: string;
      readonly after?: string;
      readonly state?: "active" | "paused" | "ended" | (string & {});
      readonly limit?: number;
      /** Only sources with an open escalation. */
      readonly escalated?: boolean;
    } = {},
  ): Promise<AnnouncementSource[]> {
    const raw = await this._announcementRpc("sources/list", {
      mob_id: options.mobId,
      after: options.after,
      state: options.state,
      limit: options.limit,
      escalated: options.escalated,
    });
    return Array.isArray(raw.sources) ? raw.sources.map(parseAnnouncementSource) : [];
  }

  /**
   * One collector step: the items observed since the cursor, committed with
   * the next cursor at `expectedRevision` and `expectedVersion`, under
   * `principal` (wire form). The final page of a baseline, a backfill or a
   * stream onboarding says what it covered: `coverage: "complete"` or
   * `{gap: {reason}}`. A step that lost a race throws
   * {@link SourceVersionConflictError} or {@link SourceStaleRevisionError}
   * with the source's current position.
   */
  async announcementSourceAdvance(
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    expectedVersion: number,
    principal: AnnouncementWireObject,
    nextCursor: string | null,
    options: {
      readonly mobId?: string;
      readonly page?: AnnouncementWireObject;
      readonly coverage?: "complete" | AnnouncementWireObject;
      readonly items?: readonly AnnouncementWireObject[];
      readonly streams?: readonly AnnouncementWireObject[];
      readonly rebaseline?: AnnouncementWireObject;
      readonly observed?: AnnouncementWireObject;
      readonly collectedAtMs?: number;
    } = {},
  ): Promise<SourceAdvanceResult> {
    return parseSourceAdvanceResult(
      await this._announcementRpc("sources/advance", {
        operation_id: operationId,
        source_id: sourceId,
        expected_revision: expectedRevision,
        expected_version: expectedVersion,
        principal,
        next_cursor: nextCursor,
        mob_id: options.mobId,
        page: options.page,
        coverage: options.coverage,
        items: options.items === undefined ? undefined : [...options.items],
        streams: options.streams === undefined ? undefined : [...options.streams],
        rebaseline: options.rebaseline,
        observed: options.observed,
        collected_at_ms: options.collectedAtMs,
      }),
    );
  }

  /**
   * Record a collector failure (`{class, retry_after_ms?, detail?}`), fenced
   * like a step.
   */
  async announcementSourceRecordFailure(
    operationId: string,
    sourceId: string,
    expectedRevision: number,
    expectedVersion: number,
    principal: AnnouncementWireObject,
    failure: AnnouncementWireObject,
    options: { readonly mobId?: string } = {},
  ): Promise<AnnouncementSource> {
    const raw = await this._announcementRpc("sources/record_failure", {
      operation_id: operationId,
      source_id: sourceId,
      expected_revision: expectedRevision,
      expected_version: expectedVersion,
      principal,
      failure,
      mob_id: options.mobId,
    });
    return parseAnnouncementSource(raw.source);
  }

  /** A member's backlog, why its deliveries wait, and its open escalations. */
  async announcementMemberGet(member: AnnouncementMemberParam): Promise<AnnouncementMemberView> {
    return parseAnnouncementMemberView(
      await this._announcementRpc("members/get", { member: announcementMemberToWire(member) }),
    );
  }

  /**
   * A page of the members with an active or paused subscription, a suspend
   * marker or an open escalation, in member order after `after`, or with
   * `escalated: true` only those with an open escalation.
   */
  async announcementMemberList(
    options: { readonly after?: AnnouncementMemberParam; readonly limit?: number; readonly escalated?: boolean } = {},
  ): Promise<AnnouncementMemberView[]> {
    const raw = await this._announcementRpc("members/list", {
      after: options.after === undefined ? undefined : announcementMemberToWire(options.after),
      limit: options.limit,
      escalated: options.escalated,
    });
    return Array.isArray(raw.members) ? raw.members.map(parseAnnouncementMemberView) : [];
  }

  /**
   * The escalation events after the event cursor `after` (from the first one
   * kept by default), or with `open: true` the open escalations. After a
   * reconnect, read on from the last cursor an `onError` escalation event or
   * this page's `next` named.
   */
  async announcementEscalationsList(
    options: { readonly after?: number; readonly limit?: number; readonly open?: boolean } = {},
  ): Promise<AnnouncementEscalationsPage> {
    return parseAnnouncementEscalationsPage(
      await this._announcementRpc("escalations/list", {
        after: options.after,
        limit: options.limit,
        open: options.open,
      }),
    );
  }

  /**
   * Apply declared sources and subscriptions now, and resolve what is
   * applied. `declarations` is `{revision, sources?, subscriptions?}` in wire
   * form, for a gateway that declares none or declares them inline;
   * `options.revision` defaults to its own. Called with neither, a gateway
   * whose declarations come from a sidecar file reads the file again. Any
   * other combination is `-32602`. A lower revision than the applied one
   * rejects with `AnnouncementRefusedError` `declaration_rollback`, and the
   * applied revision with other declarations `declaration_digest_conflict`.
   */
  async announcementDeclarationsApply(
    declarations?: AnnouncementWireObject,
    options: { readonly revision?: number } = {},
  ): Promise<AnnouncementDeclarationsView> {
    const revision =
      options.revision ?? (declarations === undefined ? undefined : declarations.revision);
    return parseAnnouncementDeclarationsView(
      await this._announcementRpc("declarations/apply", {
        revision,
        declarations: declarations === undefined ? undefined : { ...declarations },
      }),
    );
  }

  /** The applied declarations' revision (0 before any) and digest, and where each record they manage stands. */
  async announcementDeclarationsGet(): Promise<AnnouncementDeclarationsView> {
    return parseAnnouncementDeclarationsView(await this._announcementRpc("declarations/get", {}));
  }

  /** A page of a mob's counted source audit entries, by source, member, action, or any of them. */
  async announcementAuditList(
    options: {
      readonly mobId?: string;
      readonly sourceId?: string;
      readonly member?: AnnouncementMemberParam;
      readonly action?: string;
      readonly after?: number;
      readonly limit?: number;
    } = {},
  ): Promise<AnnouncementAuditEntry[]> {
    const raw = await this._announcementRpc("audit/list", {
      mob_id: options.mobId,
      source_id: options.sourceId,
      member: options.member === undefined ? undefined : announcementMemberToWire(options.member),
      action: options.action,
      after: options.after,
      limit: options.limit,
    });
    return Array.isArray(raw.entries) ? raw.entries.map(parseAnnouncementAuditEntry) : [];
  }

  // -- Live (realtime) sessions — meta/live/* (mobkit 0.7.31) ------------

  /**
   * Open a realtime (live) channel on a member's session. Returns the
   * transport bootstrap `{channel_id, transport: {type: "websocket", url,
   * token}, capabilities, continuity}`. The token is single-use with a
   * short TTL — hand the URL to the client immediately.
   */
  async liveOpen(
    identity: string,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const raw = await this._runtime._rpc("meta/live/open", {
      identity,
      ...(options ?? {}),
    });
    return asWireRecord(raw);
  }

  /** Open a live channel and parse the strict typed channel handle. */
  async liveOpenTyped(
    identity: string,
    executionIdentity?: LiveExecutionIdentityV1,
    options?: Record<string, unknown>,
  ): Promise<LiveChannelHandle | PendingLiveChannelHandle> {
    let advertisedFeatureCapabilities: readonly string[] = [];
    if (executionIdentity !== undefined) {
      const forbidden = [
        "mode",
        "execution_mode",
        "profile",
        "profile_id",
        "execution_profile",
        "execution_profile_id",
        "delegation",
        "delegation_type",
        "delegation_model",
        "responses",
        "responses_model",
        "responses_tools",
        "responses_instructions",
        "bridge_model",
        "bridge_tools",
        "bridge_instructions",
        "auth_binding",
        "self_hosted_server_id",
        "provider_params",
        "tools",
        "instructions",
        "member_id",
        "session_id",
      ].find((field) => Object.prototype.hasOwnProperty.call(options ?? {}, field));
      if (forbidden !== undefined) {
        throw new TypeError(`experimental live/open does not accept ${forbidden}`);
      }
    }
    if (
      executionIdentity !== undefined &&
      (Object.prototype.hasOwnProperty.call(options ?? {}, "model") ||
        Object.prototype.hasOwnProperty.call(options ?? {}, "provider"))
    ) {
      throw new TypeError(
        "executionIdentity conflicts with legacy top-level model/provider",
      );
    }
    if (executionIdentity !== undefined) {
      const capabilities = await this.capabilities();
      advertisedFeatureCapabilities = capabilities.featureCapabilities;
      if (!supportsLiveExecutionIdentityV1(capabilities)) {
        throw new CapabilityUnavailableError(
          `capability ${LIVE_EXECUTION_IDENTITY_V1} is not available`,
          "",
          "meta/live/open",
          { capability: LIVE_EXECUTION_IDENTITY_V1 },
        );
      }
    }
    const params: Record<string, unknown> = {
      identity,
      ...(options ?? {}),
    };
    if (executionIdentity !== undefined) {
      Object.assign(
        params,
        liveOpenExecutionIdentityParams(executionIdentity),
      );
    }
    const raw = asWireRecord(
      await this._runtime._rpc("meta/live/open", params),
    );
    if (executionIdentity !== undefined) {
      const pending = parsePendingLiveChannelHandle(raw);
      if (pending.targetIdentity !== identity) {
        throw new TypeError(
          "strict meta/live/open returned a different target identity",
        );
      }
      if (!supportsLiveExecutionMode(
        { featureCapabilities: advertisedFeatureCapabilities },
        pending.executionMode,
      )) {
        throw new CapabilityUnavailableError(
          "resolved live execution mode is not advertised",
          "",
          "meta/live/open",
          { execution_mode: pending.executionMode },
        );
      }
      return pending;
    }
    return parseLiveChannelHandle({
      ...raw,
      target_identity: raw.target_identity ?? identity,
    });
  }

  async livePlaybackOwnerRegister(
    pending: PendingLiveChannelHandle,
  ): Promise<LivePlaybackOwnerReadiness> {
    const readiness = parseLivePlaybackOwnerReadiness(
      await this._runtime._rpc("meta/live/playback_owner/register", {
        identity: pending.targetIdentity,
        channel_id: pending.channelId,
        pending_receipt: pending.pendingReceipt,
      }),
    );
    if (readiness.channelId !== pending.channelId) {
      throw new TypeError("playback owner readiness channel mismatch");
    }
    return readiness;
  }

  async livePlaybackOwnerRevoke(
    pending: PendingLiveChannelHandle,
    readiness: LivePlaybackOwnerReadiness,
    active?: ActiveLiveChannelHandle,
  ): Promise<ExperimentalLiveChannelStatus> {
    if (readiness.channelId !== pending.channelId) {
      throw new TypeError("playback owner readiness channel mismatch");
    }
    const params: Record<string, unknown> = {
      identity: pending.targetIdentity,
      channel_id: pending.channelId,
      pending_receipt: pending.pendingReceipt,
      readiness_receipt: readiness.readinessReceipt,
    };
    if (active !== undefined) {
      activeLiveChannelHandleToWire(active);
      if (
        active.channelId !== pending.channelId ||
        active.targetIdentity !== pending.targetIdentity ||
        active.executionMode !== pending.executionMode
      ) {
        throw new TypeError("active playback owner does not match pending custody");
      }
      params.activation_receipt = active.activationReceipt;
    }
    const status = parseExperimentalLiveChannelStatus(
      await this._runtime._rpc("meta/live/playback_owner/revoke", params),
    );
    if (status.phase !== "revoked") {
      throw new TypeError("playback owner revoke did not return revoked custody");
    }
    return status;
  }

  async liveExperimentalStatus(
    handle: PendingLiveChannelHandle | ActiveLiveChannelHandle,
  ): Promise<ExperimentalLiveChannelStatus> {
    const params: Record<string, unknown> = {
      identity: handle.targetIdentity,
      channel_id: handle.channelId,
    };
    if ("pendingReceipt" in handle) {
      params.pending_receipt = handle.pendingReceipt;
    } else {
      params.activation_receipt = handle.activationReceipt;
    }
    const status = parseExperimentalLiveChannelStatus(
      await this._runtime._rpc("meta/live/status", params),
    );
    if (
      status.phase === "active" &&
      (status.handle.channelId !== handle.channelId ||
        status.handle.targetIdentity !== handle.targetIdentity ||
        status.handle.executionMode !== handle.executionMode)
    ) {
      throw new TypeError("active live handle does not match pending custody");
    }
    return status;
  }

  async liveConnect(
    identity: string,
    executionIdentity: LiveExecutionIdentityV1,
    playbackOwner: LivePlaybackOwner,
    options?: Record<string, unknown> & {
      readonly activationPollIntervalMs?: number;
      readonly activationAttempts?: number;
    },
  ): Promise<ActiveLiveChannelConnection> {
    const activationPollIntervalMs = options?.activationPollIntervalMs ?? 50;
    const activationAttempts = options?.activationAttempts ?? 200;
    if (!Number.isFinite(activationPollIntervalMs) || activationPollIntervalMs < 0) {
      throw new TypeError("activationPollIntervalMs must be non-negative");
    }
    if (!Number.isSafeInteger(activationAttempts) || activationAttempts <= 0) {
      throw new TypeError("activationAttempts must be a positive integer");
    }
    const openOptions: Record<string, unknown> = { ...(options ?? {}) };
    delete openOptions.activationPollIntervalMs;
    delete openOptions.activationAttempts;
    let pending: PendingLiveChannelHandle | undefined;
    try {
      const opened = await this.liveOpenTyped(
        identity,
        executionIdentity,
        openOptions,
      );
      if (!("pendingReceipt" in opened)) {
        throw new TypeError("experimental live/open did not return a pending handle");
      }
      pending = opened;
      if (pending.transport.transport !== "webrtc") {
        throw new TypeError("experimental live/open did not return a WebRTC bootstrap");
      }
      const offerSdp = await playbackOwner.prepare(pending);
      if (typeof offerSdp !== "string" || offerSdp.trim().length === 0) {
        throw new TypeError("playback owner returned an invalid SDP offer");
      }
      const readiness = await this.livePlaybackOwnerRegister(pending);
      const answerSdp = await this.liveWebrtcAnswerPending(
        pending,
        readiness,
        offerSdp,
      );
      await playbackOwner.acceptAnswer(answerSdp);
      for (let attempt = 0; attempt < activationAttempts; attempt += 1) {
        const status = await this.liveExperimentalStatus(pending);
        if (status.phase === "active") {
          await playbackOwner.activate(status.handle);
          const active = status.handle;
          const ownerPending = pending;
          let revocation: Promise<ExperimentalLiveChannelStatus> | undefined;
          const ownerLost = (): Promise<ExperimentalLiveChannelStatus> => {
            revocation ??= (async () => {
              const revoked = await this.livePlaybackOwnerRevoke(
                ownerPending,
                readiness,
                active,
              );
              try {
                await playbackOwner.abort();
              } catch {
                // Machine revocation has already removed active authority.
              }
              return revoked;
            })();
            return revocation;
          };
          const connection: ActiveLiveChannelConnection = {
            ...active,
            pendingReceipt: ownerPending.pendingReceipt,
            readinessReceipt: readiness.readinessReceipt,
            ownerLost,
            dispose: ownerLost,
          };
          if (playbackOwner.waitForLoss !== undefined) {
            void playbackOwner.waitForLoss().then(ownerLost, ownerLost).catch(() => {
              // The explicit connection lifecycle remains available for retry.
            });
          }
          return connection;
        }
        if (status.phase === "revoked" || status.phase === "closed") {
          throw new Error(
            `experimental live channel ${status.phase} before activation`,
          );
        }
        if (activationPollIntervalMs > 0) {
          await new Promise<void>((resolve) =>
            setTimeout(resolve, activationPollIntervalMs),
          );
        }
      }
      throw new Error("experimental live channel did not activate");
    } catch (error) {
      try {
        await playbackOwner.abort();
      } finally {
        if (pending !== undefined) {
          try {
            await this.liveClose(pending);
          } catch {
            // Preserve the activation error.
          }
        }
      }
      throw error;
    }
  }

  /** Complete the one-use WebRTC signaling bootstrap for a live channel. */
  async liveWebrtcAnswer(
    channelId: string,
    token: string,
    offerSdp: string,
  ): Promise<string> {
    const raw = asWireRecord(
      await this._runtime._rpc("live/webrtc/answer", {
        channel_id: channelId,
        token,
        offer_sdp: offerSdp,
      }),
    );
    if (typeof raw.answer_sdp !== "string") {
      throw new TypeError("live/webrtc/answer returned an invalid answer");
    }
    return raw.answer_sdp;
  }

  async liveWebrtcAnswerPending(
    pending: PendingLiveChannelHandle,
    readiness: LivePlaybackOwnerReadiness,
    offerSdp: string,
  ): Promise<string> {
    if (readiness.channelId !== pending.channelId) {
      throw new TypeError("playback owner readiness channel mismatch");
    }
    if (pending.transport.transport !== "webrtc") {
      throw new TypeError("pending handle is not a WebRTC bootstrap");
    }
    const raw = asWireRecord(
      await this._runtime._rpc("live/webrtc/answer", {
        identity: pending.targetIdentity,
        channel_id: pending.channelId,
        pending_receipt: pending.pendingReceipt,
        readiness_receipt: readiness.readinessReceipt,
        token: pending.transport.token,
        offer_sdp: offerSdp,
      }),
    );
    if (typeof raw.answer_sdp !== "string") {
      throw new TypeError("live/webrtc/answer returned an invalid answer");
    }
    return raw.answer_sdp;
  }

  /** Read retryably pending signaling without auto-renegotiating. */
  async liveReplacementRequired(
    active: ActiveLiveChannelHandle,
  ): Promise<LiveReplacementRequired> {
    activeLiveChannelHandleToWire(active);
    return parseLiveReplacementRequired(
      await this._runtime._rpc("meta/live/replacement_required", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
      }),
    );
  }

  /**
   * Stream opaque assistant outputs with bounded, loss-intolerant delivery.
   * Closing the iterator closes the exact live channel.
   */
  async *liveOutputs(
    active: ActiveLiveChannelHandle,
    options?: { readonly capacity?: number },
  ): AsyncGenerator<LiveAssistantOutputAddress, void, void> {
    activeLiveChannelHandleToWire(active);
    const capacity = options?.capacity ?? 16;
    if (!Number.isSafeInteger(capacity) || capacity <= 0) {
      throw new TypeError("live output queue capacity must be a positive integer");
    }
    const items: LiveAssistantOutputAddress[] = [];
    const waiters: Array<(output: LiveAssistantOutputAddress) => void> = [];
    const unregister = this._runtime._registerLiveOutputConsumer(
      active.channelId,
      (output) => {
        const waiter = waiters.shift();
        if (waiter !== undefined) {
          waiter(output);
          return;
        }
        if (items.length >= capacity) {
          throw new Error(`live output consumer queue is full for ${active.channelId}`);
        }
        items.push(output);
      },
    );
    try {
      while (true) {
        const output = items.shift() ?? await new Promise<LiveAssistantOutputAddress>(
          (resolve) => waiters.push(resolve),
        );
        yield output;
      }
    } finally {
      unregister();
      await this.liveClose(active);
    }
  }

  /** Report measured playback completion for an exact opaque output. */
  async livePlaybackComplete(
    active: ActiveLiveChannelHandle,
    output: LiveAssistantOutputAddress,
  ): Promise<LivePlaybackCompleteResult> {
    activeLiveChannelHandleToWire(active);
    if (output.channelId !== active.channelId) {
      throw new TypeError("assistant output does not belong to active channel");
    }
    return parseLivePlaybackCompleteResult(
      await this._runtime._rpc("meta/live/playback_complete", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
        output_id: output.outputId,
      }),
    );
  }

  async liveStatus(
    identity: string,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const raw = await this._runtime._rpc("meta/live/status", {
      identity,
      ...(options ?? {}),
    });
    return asWireRecord(raw);
  }

  async liveClose(
    handle: string | PendingLiveChannelHandle | ActiveLiveChannelHandle,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const params: Record<string, unknown> = {
      channel_id: typeof handle === "string" ? handle : handle.channelId,
      ...(options ?? {}),
    };
    if (typeof handle !== "string") {
      params.identity = handle.targetIdentity;
      if ("pendingReceipt" in handle) {
        params.pending_receipt = handle.pendingReceipt;
      } else {
        params.activation_receipt = handle.activationReceipt;
      }
    }
    const raw = await this._runtime._rpc("meta/live/close", params);
    return asWireRecord(raw);
  }

  /**
   * Send a still image into the member's open live channel (meerkat
   * 0.7.27+). `idempotencyKey` must be caller-stable within the session —
   * retries with the same key are exact-retry deduplicated.
   */
  async liveSendInputImage(
    identity: string,
    idempotencyKey: string,
    mime: string,
    dataBase64: string,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const raw = await this._runtime._rpc("meta/live/send_input", {
      identity,
      chunk: {
        kind: "image",
        idempotency_key: idempotencyKey,
        mime,
        data: dataBase64,
      },
      ...(options ?? {}),
    });
    return asWireRecord(raw);
  }

  /**
   * Push refreshed mutable config (instructions/tools/audio) into an open
   * live channel without rebuilding the transport. Model/provider swaps
   * require close + reopen.
   */
  async liveRefresh(
    identity: string,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const raw = await this._runtime._rpc("meta/live/refresh", {
      identity,
      ...(options ?? {}),
    });
    return asWireRecord(raw);
  }

  /** Truncate an exact opaque output at measured playback progress. */
  async liveTruncate(
    active: ActiveLiveChannelHandle,
    output: LiveAssistantOutputAddress,
    audioPlayedMs: number,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    activeLiveChannelHandleToWire(active);
    if (output.channelId !== active.channelId) {
      throw new TypeError("assistant output does not belong to active channel");
    }
    const raw = await this._runtime._rpc("meta/live/truncate", {
      identity: active.targetIdentity,
      channel_id: active.channelId,
      activation_receipt: active.activationReceipt,
      output_id: output.outputId,
      audio_played_ms: audioPlayedMs,
      ...(options ?? {}),
    });
    return asWireRecord(raw);
  }

  async liveRefreshActive(
    active: ActiveLiveChannelHandle,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    activeLiveChannelHandleToWire(active);
    return asWireRecord(
      await this._runtime._rpc("meta/live/refresh", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
        ...(options ?? {}),
      }),
    );
  }

  async liveSendInputImageActive(
    active: ActiveLiveChannelHandle,
    idempotencyKey: string,
    mime: string,
    dataBase64: string,
  ): Promise<Record<string, unknown>> {
    activeLiveChannelHandleToWire(active);
    return asWireRecord(
      await this._runtime._rpc("meta/live/send_input", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
        chunk: {
          kind: "image",
          idempotency_key: idempotencyKey,
          mime,
          data: dataBase64,
        },
      }),
    );
  }

  async liveCommitInputActive(
    active: ActiveLiveChannelHandle,
    options?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    activeLiveChannelHandleToWire(active);
    return asWireRecord(
      await this._runtime._rpc("meta/live/commit_input", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
        ...(options ?? {}),
      }),
    );
  }

  async liveInterruptActive(
    active: ActiveLiveChannelHandle,
  ): Promise<Record<string, unknown>> {
    activeLiveChannelHandleToWire(active);
    return asWireRecord(
      await this._runtime._rpc("meta/live/interrupt", {
        identity: active.targetIdentity,
        channel_id: active.channelId,
        activation_receipt: active.activationReceipt,
      }),
    );
  }

  // -- Topology -----------------------------------------------------------

  async rediscover(): Promise<RediscoverReport | null> {
    const raw = await this._runtime._rpc("meta/rediscover");
    if (
      typeof raw === "object" &&
      raw !== null &&
      "status" in (raw as Record<string, unknown>)
    ) {
      return null;
    }
    return parseRediscoverReport(raw);
  }

  async reconcileEdges(): Promise<ReconcileEdgesReport> {
    return parseReconcileEdgesReport(
      await this._runtime._rpc("meta/reconcile_edges"),
    );
  }

  // -- Cross-mob ----------------------------------------------------------

  async listExternalMobs(): Promise<CrossMobContactEntry[]> {
    const raw = await this._runtime._rpc("meta/cross_mob/directory");
    const mobs =
      typeof raw === "object" && raw !== null
        ? Array.isArray((raw as Record<string, unknown>).mobs)
          ? ((raw as Record<string, unknown>).mobs as unknown[])
          : []
        : [];
    return mobs.map(parseCrossMobContactEntry);
  }

  async peerInfo(memberId: string): Promise<Record<string, string>> {
    const raw = await this._runtime._rpc("meta/cross_mob/peer_info", {
      member_id: memberId,
    });
    const out: Record<string, string> = {};
    if (typeof raw === "object" && raw !== null) {
      for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
        out[k] = String(v);
      }
    }
    return out;
  }

  async peerPubkey(): Promise<string> {
    const raw = await this._runtime._rpc("meta/peer_pubkey");
    return typeof raw === "object" && raw !== null
      ? String((raw as Record<string, unknown>).pubkey_b64 ?? "")
      : "";
  }

  async wireLocal(
    localMemberId: string,
    remoteCommsName: string,
    remotePeerId: string,
    remoteAddress: string,
    options?: { remotePubkeyB64?: string },
  ): Promise<void> {
    const params: Record<string, unknown> = {
      local_member_id: localMemberId,
      remote_comms_name: remoteCommsName,
      remote_peer_id: remotePeerId,
      remote_address: remoteAddress,
    };
    if (options?.remotePubkeyB64) {
      params.remote_pubkey_b64 = options.remotePubkeyB64;
    }
    await this._runtime._rpc("meta/cross_mob/wire_local", params);
  }

  async unwireLocal(
    localMemberId: string,
    remoteCommsName: string,
    remotePeerId: string,
    remoteAddress: string,
    options?: { remotePubkeyB64?: string },
  ): Promise<void> {
    const params: Record<string, unknown> = {
      local_member_id: localMemberId,
      remote_comms_name: remoteCommsName,
      remote_peer_id: remotePeerId,
      remote_address: remoteAddress,
    };
    if (options?.remotePubkeyB64) {
      params.remote_pubkey_b64 = options.remotePubkeyB64;
    }
    await this._runtime._rpc("meta/cross_mob/unwire_local", params);
  }

  async wireCrossMob(
    localMemberId: string,
    remoteMemberId: string,
    remoteHandle: MobHandle,
  ): Promise<void> {
    const localInfo = await this.peerInfo(localMemberId);
    const remoteInfo = await remoteHandle.peerInfo(remoteMemberId);
    await this.wireLocal(
      localMemberId,
      remoteInfo.comms_name,
      remoteInfo.peer_id,
      remoteInfo.address,
    );
    try {
      await remoteHandle.wireLocal(
        remoteMemberId,
        localInfo.comms_name,
        localInfo.peer_id,
        localInfo.address,
      );
    } catch (err) {
      try {
        await this.unwireLocal(
          localMemberId,
          remoteInfo.comms_name,
          remoteInfo.peer_id,
          remoteInfo.address,
        );
      } catch {
        // Best-effort rollback; preserve the original remote error.
      }
      throw err;
    }
  }

  async sendCrossMob(
    remoteMemberId: string,
    remoteHandle: MobHandle,
    message: string | DispatchContentBlock[],
  ): Promise<SendMessageResult> {
    return remoteHandle.send(remoteMemberId, message);
  }

  // -- Helper members -----------------------------------------------------

  async spawnHelper(
    agentIdentity: string,
    task: string,
    resultLabel: string,
    maxTextBytes: number,
    options?: {
      role?: string;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<HelperResult> {
    const helperOptions: Record<string, unknown> = {};
    if (options?.role) helperOptions.role = options.role;
    if (options?.runtimeMode) helperOptions.runtime_mode = options.runtimeMode;
    if (options?.backend) helperOptions.backend = options.backend;
    const params: Record<string, unknown> = {
      agent_identity: agentIdentity,
      task,
      result_label: resultLabel,
      max_text_bytes: maxTextBytes,
    };
    if (Object.keys(helperOptions).length > 0) params.options = helperOptions;
    return parseHelperResult(
      await this._runtime._rpc("meta/spawn_helper", params),
    );
  }

  async forkHelper(
    sourceMemberId: string,
    agentIdentity: string,
    task: string,
    resultLabel: string,
    maxTextBytes: number,
    options?: {
      forkContext?: Record<string, unknown>;
      role?: string;
      runtimeMode?: string;
      backend?: string;
    },
  ): Promise<HelperResult> {
    const helperOptions: Record<string, unknown> = {};
    if (options?.role) helperOptions.role = options.role;
    if (options?.runtimeMode) helperOptions.runtime_mode = options.runtimeMode;
    if (options?.backend) helperOptions.backend = options.backend;
    const params: Record<string, unknown> = {
      source_member_id: sourceMemberId,
      agent_identity: agentIdentity,
      task,
      result_label: resultLabel,
      max_text_bytes: maxTextBytes,
    };
    if (options?.forkContext) params.fork_context = options.forkContext;
    if (Object.keys(helperOptions).length > 0) params.options = helperOptions;
    return parseHelperResult(
      await this._runtime._rpc("meta/fork_helper", params),
    );
  }

  async attachSession(
    role: string,
    agentIdentity: string,
    sessionId: string,
  ): Promise<RichMemberSnapshot> {
    return parseRichMemberSnapshot(
      await this._runtime._rpc("meta/attach_existing_session", {
        role,
        agent_identity: agentIdentity,
        session_id: sessionId,
      }),
    );
  }

  // -- Mob/run labels — meta-side sidecar metadata ----------------------

  /**
   * Replace the label set associated with this mob.
   *
   * Meta owns these labels — they are separate from meerkat-mob's
   * member-level labels. Replacement is wholesale; pass `{}` to clear.
   */
  async setMobLabels(labels: Record<string, string>): Promise<void> {
    await this._runtime._rpc("meta/mob_labels/set", { labels });
  }

  /** Return the label set associated with this mob (or `{}`). */
  async getMobLabels(): Promise<Record<string, string>> {
    const raw = await this._runtime._rpc("meta/mob_labels/get");
    if (typeof raw !== "object" || raw === null) return {};
    const labels = (raw as Record<string, unknown>).labels;
    if (typeof labels !== "object" || labels === null) return {};
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(labels as Record<string, unknown>)) {
      out[k] = String(v);
    }
    return out;
  }

  /** Remove the label set associated with this mob. */
  async deleteMobLabels(): Promise<void> {
    await this._runtime._rpc("meta/mob_labels/delete");
  }

  /**
   * Replace the label set associated with `runId` under this mob.
   * Replacement is wholesale (see {@link setMobLabels}).
   */
  async setRunLabels(
    runId: string,
    labels: Record<string, string>,
  ): Promise<void> {
    await this._runtime._rpc("meta/run_labels/set", {
      run_id: runId,
      labels,
    });
  }

  /** Return the label set for `runId` (or `{}`). */
  async getRunLabels(runId: string): Promise<Record<string, string>> {
    const raw = await this._runtime._rpc("meta/run_labels/get", {
      run_id: runId,
    });
    if (typeof raw !== "object" || raw === null) return {};
    const labels = (raw as Record<string, unknown>).labels;
    if (typeof labels !== "object" || labels === null) return {};
    const out: Record<string, string> = {};
    for (const [k, v] of Object.entries(labels as Record<string, unknown>)) {
      out[k] = String(v);
    }
    return out;
  }

  /** Remove the label set for `runId`. */
  async deleteRunLabels(runId: string): Promise<void> {
    await this._runtime._rpc("meta/run_labels/delete", { run_id: runId });
  }
}

// -- ToolCaller -----------------------------------------------------------

/**
 * Bound callable scoped to one MCP module.
 *
 * @example
 * ```ts
 * const gmail = handle.toolCaller("google-workspace");
 * const messages = await gmail.call("gmail_search", { query: "is:unread" });
 * ```
 */
export class ToolCaller {
  constructor(
    private readonly _mobHandle: MobHandle,
    private readonly _moduleId: string,
  ) {}

  async call(tool: string, args?: Record<string, unknown>): Promise<unknown> {
    const result = await this._mobHandle.callTool(this._moduleId, tool, args);
    return result.result;
  }
}

// -- SseBridge ------------------------------------------------------------

/**
 * Bridge for streaming SSE from the Rust backend's HTTP server.
 */
export class SseBridge {
  constructor(private readonly _runtime: MetaRuntime) {}

  private _baseUrl(): string {
    const base = this._runtime.rustHttpBaseUrl;
    if (base === null) {
      throw new NotConnectedError(
        "SSE bridge requires rustHttpBaseUrl — set it via " +
          "runtime.setRustHttpBase('http://127.0.0.1:8081') or " +
          "ensure the Rust binary reports it during bootstrap",
      );
    }
    return base;
  }

  async *agentEvents(
    agentId: string,
  ): AsyncGenerator<SseEvent, void, undefined> {
    const url = `${this._baseUrl()}/agents/${agentId}/events`;
    yield* this._streamSse(url);
  }

  async *mobEvents(): AsyncGenerator<SseEvent, void, undefined> {
    const url = `${this._baseUrl()}/mob/events`;
    yield* this._streamSse(url);
  }

  private async *_streamSse(
    url: string,
  ): AsyncGenerator<SseEvent, void, undefined> {
    // Pre-fix, breaking out of the consumer iterator left the
    // underlying http.ClientRequest open — sockets and Node refs
    // accumulated. Now: tie the request lifetime to the generator via
    // try/finally so consumer `break`/`throw`/`return` always destroys
    // the request.
    const { body, destroy } = await this._fetchSseStream(url);
    try {
      yield* parseSseStream(body);
    } finally {
      destroy();
    }
  }

  private _fetchSseStream(
    url: string,
  ): Promise<{ body: AsyncIterable<Uint8Array>; destroy: () => void }> {
    const parsed = new URL(url);
    const requester = parsed.protocol === "https:" ? httpsRequest : httpRequest;

    return new Promise((resolve, reject) => {
      const req = requester(
        url,
        { method: "GET", headers: { accept: "text/event-stream" } },
        (res) => {
          if (!res.statusCode || res.statusCode >= 400) {
            reject(
              new Error(
                `SSE request failed: ${res.statusCode} ${res.statusMessage}`,
              ),
            );
            res.resume();
            return;
          }

          // Convert Node readable stream to AsyncIterable<Uint8Array>
          const stream = (async function* () {
            for await (const chunk of res) {
              yield chunk instanceof Uint8Array
                ? chunk
                : new TextEncoder().encode(String(chunk));
            }
          })();

          resolve({
            body: stream,
            destroy: () => {
              req.destroy();
              res.destroy();
            },
          });
        },
      );

      req.on("error", reject);
      req.end();
    });
  }
}

/** One input that contributed to a stopped member run. */
export interface MemberRunStopContributor {
  input_id: string;
  completion:
    | "completed"
    | "completed_without_result"
    | "callback_pending"
    | "cancelled"
    | "abandoned"
    | "abandoned_with_error"
    | "completed_with_finalization_failure"
    | "runtime_terminated";
  terminal?: "completed" | "abandoned" | "superseded" | "coalesced" | "cancelled" | null;
}

/** meerkat's typed run-stop receipt, relayed verbatim by `meta/stop_member_run`. */
export type MemberRunStopReceipt =
  | { outcome: "stopped"; run_id: string; contributors: MemberRunStopContributor[] }
  | { outcome: "not_current"; run_id: string; current_run_id?: string | null }
  | { outcome: "not_stoppable"; run_id: string; state: MemberRuntimeState };

/** meerkat's public runtime state projection (`WireRuntimeState`). */
export type MemberRuntimeState =
  | "initializing"
  | "idle"
  | "attached"
  | "running"
  | "retired"
  | "stopped"
  | "destroyed";
