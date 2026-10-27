/**
 * Typed return models for Meta SDK RPC methods.
 *
 * All interfaces use `readonly` fields with camelCase naming. Parse functions
 * convert from the wire protocol's snake_case representation.
 */

// Type-only import (erased at runtime — no module cycle).
import type { ToolArtifactRef, ToolHandler } from "./models.js";
import {
  parseToolArtifactRef,
  toolArtifactRefToWire,
  validateToolArtifactRef,
} from "./models.js";
// errors.ts imports nothing, so this runtime import adds no cycle.
import { ContractMismatchError } from "./errors.js";

// -- Helpers (internal) ---------------------------------------------------

function asRecord(value: unknown): Record<string, unknown> {
  if (typeof value === "object" && value !== null) {
    return value as Record<string, unknown>;
  }
  return {};
}

function asStringArray(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value.filter((v): v is string => typeof v === "string");
  }
  return [];
}

function asRecordArray(value: unknown): Record<string, unknown>[] {
  if (Array.isArray(value)) {
    return value.filter(
      (v): v is Record<string, unknown> => typeof v === "object" && v !== null,
    );
  }
  return [];
}

function asStringRecord(value: unknown): Record<string, string> {
  const raw = asRecord(value);
  const result: Record<string, string> = {};
  for (const [k, v] of Object.entries(raw)) {
    if (typeof v === "string") {
      result[k] = v;
    }
  }
  return result;
}

function requiredStringField(
  record: Record<string, unknown>,
  field: string,
  label: string,
): string {
  const value = record[field];
  if (typeof value !== "string" || value.length === 0) {
    throw new Error(`${label}.${field} must be a non-empty string`);
  }
  return value;
}

function requiredNumberField(
  record: Record<string, unknown>,
  field: string,
  label: string,
): number {
  const value = record[field];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${label}.${field} must be a finite number`);
  }
  return value;
}

function validateAgentIdentity(identity: string): string {
  if (
    identity.length === 0 ||
    identity.trim() !== identity ||
    /\s/.test(identity) ||
    identity.includes("/")
  ) {
    throw new Error(`invalid agent identity: ${identity}`);
  }
  return identity;
}

// -- Constants ------------------------------------------------------------

export const MEMBER_STATE_ACTIVE = "active" as const;
export const MEMBER_STATE_RETIRING = "retiring" as const;
// meerkat 0.7.x emits three additional member states beyond active/retiring.
// `MobMemberStatus` is `#[non_exhaustive]` on the Rust side, so branch on the
// known values and tolerate future ones rather than assuming a closed set.
export const MEMBER_STATE_BROKEN = "broken" as const;
export const MEMBER_STATE_COMPLETED = "completed" as const;
export const MEMBER_STATE_UNKNOWN = "unknown" as const;

// -- SessionCreatedContext ------------------------------------------------

/** Context delivered to SessionAgentBuilder.afterCreate after a session
 *  is successfully created. */
export interface SessionCreatedContext {
  readonly model: string;
  readonly labels: Record<string, string>;
  readonly systemPrompt: string | null;
}

// -- Storage census (H1/H2 + M4) ------------------------------------------

/**
 * One composed storage slot's durability record (M4 census): the domain
 * (`"sessions"`, `"runtime"`, `"blobs"`, ...), meerkat's durability
 * vocabulary (`durabilityClass` is the wire field `class`: `"durable"` /
 * `"scratch"`; `resolution`: `"persistent"` / `"declared_ephemeral"` /
 * `"non_persistent"`), the concrete backend, whether the slot is a
 * sanctioned boot-without degradation, and an optional detail note.
 * Parsing is forward-tolerant: unknown fields are ignored and unknown
 * vocabulary values pass through as strings.
 */
export interface StorageSlotSummary {
  readonly domain: string;
  readonly durabilityClass: string;
  readonly resolution: string;
  readonly backend: string;
  readonly degraded: boolean;
  readonly detail?: string;
}

/**
 * The runtime's composition-time storage census — the `storage` object
 * shared by `meta/status`, `meta/capabilities`, and
 * `meta/storage/doctor`: `blobDurability` / `blobStorePersistent` record
 * the blob slot's resolution (H1), `sessionStoreIncremental` the
 * incremental persistence probe (H2, `null` when no sessions are
 * persisted), and `slots` the per-slot durability census (M4).
 */
export interface StorageSummary {
  readonly blobDurability: string;
  readonly blobStorePersistent: boolean;
  readonly sessionStoreIncremental: boolean | null;
  readonly slots: readonly StorageSlotSummary[];
}

export function parseStorageSummary(raw: unknown): StorageSummary {
  const d = asRecord(raw);
  const slots = asRecordArray(d.slots).map((entry) => ({
    domain: String(entry.domain ?? ""),
    durabilityClass: String(entry.class ?? ""),
    resolution: String(entry.resolution ?? ""),
    backend: String(entry.backend ?? ""),
    degraded: Boolean(entry.degraded ?? false),
    ...(entry.detail !== undefined ? { detail: String(entry.detail) } : {}),
  }));
  return {
    blobDurability: String(d.blob_durability ?? ""),
    blobStorePersistent: Boolean(d.blob_store_persistent ?? false),
    sessionStoreIncremental:
      typeof d.session_store_incremental === "boolean"
        ? d.session_store_incremental
        : null,
    slots,
  };
}

function parseOptionalStorageSummary(raw: unknown): StorageSummary | undefined {
  return raw !== null && raw !== undefined && typeof raw === "object"
    ? parseStorageSummary(raw)
    : undefined;
}

// -- StatusResult ---------------------------------------------------------

export interface StatusResult {
  readonly contractVersion: string;
  readonly running: boolean;
  readonly loadedModules: readonly string[];
  readonly storage?: StorageSummary;
  /**
   * The gateway's mob announcement counts: absent when mob announcements are
   * disabled or the gateway predates the object, and `null` when their store
   * could not be read.
   */
  readonly mobAnnouncements?: MobAnnouncementsStatus | null;
}

export function parseStatusResult(raw: unknown): StatusResult {
  const d = asRecord(raw);
  const storage = parseOptionalStorageSummary(d.storage);
  return {
    contractVersion: String(d.contract_version ?? ""),
    running: Boolean(d.running),
    loadedModules: asStringArray(d.loaded_modules),
    ...(storage !== undefined ? { storage } : {}),
    ...(d.mob_announcements !== undefined
      ? { mobAnnouncements: parseOptionalMobAnnouncementsStatus(d.mob_announcements) }
      : {}),
  };
}

// -- StorageDoctorResult ----------------------------------------------------

/** One storage-doctor finding (stable kebab-case `code`; consumers must
 *  tolerate unknown codes). */
export interface StorageDoctorFinding {
  readonly severity: string;
  readonly code: string;
  readonly message: string;
  readonly path?: string;
  readonly realm?: string;
}

/** `meta/storage/doctor` result: the shape-stable StorageDiagnosis plus
 *  the live H1/H2 storage summary when the gateway resolved one. `storage`
 *  stays the raw wire record for compatibility; apply
 *  {@link parseStorageSummary} to get the typed census. */
export interface StorageDoctorResult {
  readonly stateDir: string;
  readonly findings: readonly StorageDoctorFinding[];
  readonly inventory: readonly Record<string, unknown>[];
  readonly storage: Record<string, unknown> | null;
}

export function parseStorageDoctorResult(raw: unknown): StorageDoctorResult {
  const d = asRecord(raw);
  const diagnosis = asRecord(d.diagnosis);
  const findings = asRecordArray(diagnosis.findings).map((entry) => ({
    severity: String(entry.severity ?? ""),
    code: String(entry.code ?? ""),
    message: String(entry.message ?? ""),
    ...(entry.path !== undefined ? { path: String(entry.path) } : {}),
    ...(entry.realm !== undefined ? { realm: String(entry.realm) } : {}),
  }));
  return {
    stateDir: String(d.state_dir ?? ""),
    findings,
    inventory: asRecordArray(diagnosis.inventory),
    storage:
      d.storage === null || d.storage === undefined ? null : asRecord(d.storage),
  };
}

// -- RuntimeCapabilities --------------------------------------------------

export interface ProfileCapabilities {
  readonly instanceCount: number;
  readonly addressable: boolean;
  readonly hasWiring: boolean;
}

export interface RuntimeCapabilities {
  readonly canSpawnMembers: boolean;
  readonly canSendMessages: boolean;
  readonly canWireMembers: boolean;
  readonly canRetireMembers: boolean;
  readonly availableSpawnModes: readonly string[];
  readonly profileCapabilities?: Readonly<Record<string, ProfileCapabilities>>;
}

function parseProfileCapabilities(
  raw: unknown,
): Record<string, ProfileCapabilities> | undefined {
  if (raw == null || typeof raw !== "object") return undefined;
  const d = raw as Record<string, Record<string, unknown>>;
  const result: Record<string, ProfileCapabilities> = {};
  for (const [key, val] of Object.entries(d)) {
    if (val && typeof val === "object") {
      result[key] = {
        instanceCount: Number(val.instance_count ?? 0),
        addressable: Boolean(val.addressable ?? true),
        hasWiring: Boolean(val.has_wiring ?? false),
      };
    }
  }
  return Object.keys(result).length > 0 ? result : undefined;
}

function parseRuntimeCapabilities(raw: unknown): RuntimeCapabilities | undefined {
  if (raw == null || typeof raw !== "object") return undefined;
  const d = raw as Record<string, unknown>;
  return {
    canSpawnMembers: Boolean(d.can_spawn_members ?? false),
    canSendMessages: Boolean(d.can_send_messages ?? false),
    canWireMembers: Boolean(d.can_wire_members ?? false),
    canRetireMembers: Boolean(d.can_retire_members ?? false),
    availableSpawnModes: asStringArray(d.available_spawn_modes),
    profileCapabilities: parseProfileCapabilities(d.profile_capabilities),
  };
}

// -- CapabilitiesResult ---------------------------------------------------

export interface CapabilitiesResult {
  readonly contractVersion: string;
  readonly featureCapabilities: readonly string[];
  readonly methods: readonly string[];
  readonly loadedModules: readonly string[];
  readonly runtimeCapabilities?: RuntimeCapabilities;
  readonly workgraph: boolean;
  readonly storage?: StorageSummary;
  /**
   * What the gateway's mob announcements offer; `null` when they are
   * disabled or the gateway predates the object.
   */
  readonly mobAnnouncements?: MobAnnouncementsCapability | null;
}

export function parseCapabilitiesResult(raw: unknown): CapabilitiesResult {
  const d = asRecord(raw);
  const storage = parseOptionalStorageSummary(d.storage);
  return {
    contractVersion: String(d.contract_version ?? ""),
    featureCapabilities: asStringArray(d.feature_capabilities),
    methods: asStringArray(d.methods),
    loadedModules: asStringArray(d.loaded_modules),
    runtimeCapabilities: parseRuntimeCapabilities(d.runtime_capabilities),
    workgraph: Boolean(d.workgraph ?? false),
    ...(storage !== undefined ? { storage } : {}),
    mobAnnouncements: parseOptionalMobAnnouncementsCapability(d.mob_announcements),
  };
}

// -- ReconcileResult ------------------------------------------------------

export interface ReconcileResult {
  readonly accepted: boolean;
  readonly reconciledModules: readonly string[];
  readonly added: number;
}

export function parseReconcileResult(raw: unknown): ReconcileResult {
  const d = asRecord(raw);
  return {
    accepted: Boolean(d.accepted),
    reconciledModules: asStringArray(d.reconciled_modules),
    added: Number(d.added ?? 0),
  };
}

// -- SpawnResult ----------------------------------------------------------

export interface SpawnResult {
  readonly accepted: boolean;
  readonly moduleId: string;
  readonly agentIdentity: string | null;
  readonly role: string | null;
}

/** Alias for backward compatibility. */
export type SpawnMemberResult = SpawnResult;

export function parseSpawnResult(raw: unknown): SpawnResult {
  const d = asRecord(raw);
  return {
    accepted: Boolean(d.accepted),
    moduleId: String(d.module_id ?? ""),
    agentIdentity:
      typeof d.agent_identity === "string" ? d.agent_identity : null,
    role: typeof d.role === "string" ? d.role : null,
  };
}

// -- KeepAliveConfig ------------------------------------------------------

export interface KeepAliveConfig {
  readonly intervalMs: number;
  readonly event: string;
}

export function parseKeepAliveConfig(raw: unknown): KeepAliveConfig {
  const d = asRecord(raw);
  return {
    intervalMs: Number(d.interval_ms ?? 0),
    event: String(d.event ?? ""),
  };
}

// -- EventEnvelope --------------------------------------------------------

export interface EventEnvelope {
  readonly eventId: string;
  readonly source: string;
  readonly timestampMs: number;
  readonly event: unknown;
}

export function parseEventEnvelope(raw: unknown): EventEnvelope {
  const d = asRecord(raw);
  return {
    eventId: String(d.event_id ?? ""),
    source: String(d.source ?? ""),
    timestampMs: Number(d.timestamp_ms ?? 0),
    event: d.event,
  };
}

// -- SubscribeResult ------------------------------------------------------

export interface SubscribeResult {
  readonly scope: string;
  readonly replayFromEventId: string | null;
  readonly keepAlive: KeepAliveConfig;
  readonly keepAliveComment: string;
  readonly eventFrames: readonly string[];
  readonly events: readonly EventEnvelope[];
}

export function parseSubscribeResult(raw: unknown): SubscribeResult {
  const d = asRecord(raw);
  // Pre-fix this mapped every entry — including null/string/number —
  // through `parseEventEnvelope`, where `asRecord` quietly returned
  // `{}` and produced silently-empty envelopes. Filter to objects so
  // non-object entries are dropped instead of becoming data-loss.
  const eventsRaw = Array.isArray(d.events)
    ? d.events.filter(
        (entry): entry is Record<string, unknown> =>
          typeof entry === "object" && entry !== null && !Array.isArray(entry),
      )
    : [];
  return {
    scope: String(d.scope ?? ""),
    replayFromEventId:
      typeof d.replay_from_event_id === "string"
        ? d.replay_from_event_id
        : null,
    keepAlive: parseKeepAliveConfig(d.keep_alive),
    keepAliveComment: String(d.keep_alive_comment ?? ""),
    eventFrames: asStringArray(d.event_frames),
    events: eventsRaw.map(parseEventEnvelope),
  };
}

// -- SendMessageResult ----------------------------------------------------

export interface SendMessageResult {
  readonly accepted: boolean;
  readonly memberId: string;
  readonly sessionId: string;
}

export function parseSendMessageResult(raw: unknown): SendMessageResult {
  const d = asRecord(raw);
  return {
    accepted: Boolean(d.accepted),
    memberId: String(d.member_id ?? ""),
    sessionId: String(d.session_id ?? ""),
  };
}

// -- RoutingResolution ----------------------------------------------------

export interface RoutingResolution {
  readonly recipient: string;
  readonly route: Record<string, unknown>;
}

export function parseRoutingResolution(raw: unknown): RoutingResolution {
  const d = asRecord(raw);
  return {
    recipient: String(d.recipient ?? ""),
    route: asRecord(d.route ?? d),
  };
}

// -- DeliveryResult -------------------------------------------------------

export interface DeliveryResult {
  readonly delivered: boolean;
  readonly deliveryId: string;
}

export function parseDeliveryResult(raw: unknown): DeliveryResult {
  const d = asRecord(raw);
  return {
    delivered: Boolean(d.delivered),
    deliveryId: String(d.delivery_id ?? ""),
  };
}

// -- DeliveryHistoryResult ------------------------------------------------

export interface DeliveryHistoryResult {
  readonly deliveries: readonly Record<string, unknown>[];
}

export function parseDeliveryHistoryResult(
  raw: unknown,
): DeliveryHistoryResult {
  const d = asRecord(raw);
  return {
    deliveries: asRecordArray(d.deliveries),
  };
}

// -- MemoryQueryResult ----------------------------------------------------

export interface MemoryAssertion {
  readonly assertionId: string;
  readonly entity: string;
  readonly topic: string;
  readonly store: string;
  readonly fact: string;
  readonly metadata: unknown | null;
  readonly indexedAtMs: number;
}

export interface MemoryConflictSignal {
  readonly entity: string;
  readonly topic: string;
  readonly store: string;
  readonly reason: string | null;
  readonly updatedAtMs: number;
}

export interface MemoryQueryResult {
  readonly assertions: readonly MemoryAssertion[];
  readonly conflicts: readonly MemoryConflictSignal[];
  /** Legacy flattened alias retained for older callers. */
  readonly results: readonly Record<string, unknown>[];
}

export function parseMemoryQueryResult(raw: unknown): MemoryQueryResult {
  const d = asRecord(raw);
  const assertions = asRecordArray(d.assertions).map((entry) => ({
    assertionId: String(entry.assertion_id ?? ""),
    entity: String(entry.entity ?? ""),
    topic: String(entry.topic ?? ""),
    store: String(entry.store ?? ""),
    fact: String(entry.fact ?? ""),
    metadata: entry.metadata ?? null,
    indexedAtMs: Number(entry.indexed_at_ms ?? 0),
  }));
  const conflicts = asRecordArray(d.conflicts).map((entry) => ({
    entity: String(entry.entity ?? ""),
    topic: String(entry.topic ?? ""),
    store: String(entry.store ?? ""),
    reason: typeof entry.reason === "string" ? entry.reason : null,
    updatedAtMs: Number(entry.updated_at_ms ?? 0),
  }));
  const legacyResults = asRecordArray(d.results);
  return {
    assertions,
    conflicts,
    results: legacyResults.length > 0
      ? legacyResults
      : [
          ...assertions.map((assertion) => ({ ...assertion })),
          ...conflicts.map((conflict) => ({ ...conflict, conflict: true })),
        ],
  };
}

// -- AgentMemoryRecord -----------------------------------------------------

export interface AgentMemoryRecord {
  readonly memoryId: string;
  readonly title: string;
  readonly body: string;
  readonly tags: readonly string[];
  readonly createdAtMs: number;
  readonly updatedAtMs: number;
}

export interface AgentMemoryForgetResult {
  readonly memoryId: string;
  readonly deleted: boolean;
}

export interface AgentMemoryRecallResult {
  readonly records: readonly AgentMemoryRecord[];
}

export function parseAgentMemoryRecord(raw: unknown): AgentMemoryRecord {
  const d = asRecord(raw);
  const memoryId = requiredStringField(d, "memory_id", "agent_memory_record");
  const title = requiredStringField(d, "title", "agent_memory_record");
  const body = requiredStringField(d, "body", "agent_memory_record");
  const createdAtMs = requiredNumberField(d, "created_at_ms", "agent_memory_record");
  const updatedAtMs = requiredNumberField(d, "updated_at_ms", "agent_memory_record");
  const tags = d.tags;
  if (!Array.isArray(tags) || tags.some((tag) => typeof tag !== "string")) {
    throw new Error("agent_memory_record.tags must be an array of strings");
  }
  return {
    memoryId,
    title,
    body,
    tags: tags,
    createdAtMs,
    updatedAtMs,
  };
}

export function parseAgentMemoryRecallResult(raw: unknown): AgentMemoryRecallResult {
  const d = asRecord(raw);
  if (!Array.isArray(d.records)) {
    throw new Error("agent_memory_recall_result.records must be an array");
  }
  return {
    records: d.records.map(parseAgentMemoryRecord),
  };
}

export function parseAgentMemoryForgetResult(raw: unknown): AgentMemoryForgetResult {
  const d = asRecord(raw);
  const memoryId = requiredStringField(d, "memory_id", "agent_memory_forget_result");
  if (typeof d.deleted !== "boolean") {
    throw new Error("agent_memory_forget_result.deleted must be a boolean");
  }
  return {
    memoryId,
    deleted: d.deleted,
  };
}

/** Result of meta/agent_memory/update: the superseding record's id. */
export interface AgentMemoryUpdateResult {
  readonly memoryId: string;
  readonly supersedes: string;
}

/** Manifest row: record metadata without the body (an index, not a dump). */
export interface AgentMemoryRecordMeta {
  readonly id: string;
  readonly kind: string;
  readonly title: string;
  readonly description: string;
  readonly ageDays: number;
  readonly rank: number | null;
}

export interface AgentMemoryManifestResult {
  readonly records: readonly AgentMemoryRecordMeta[];
}

export function parseAgentMemoryUpdateResult(raw: unknown): AgentMemoryUpdateResult {
  const d = asRecord(raw);
  return {
    memoryId: requiredStringField(d, "memory_id", "agent_memory_update_result"),
    supersedes: requiredStringField(d, "supersedes", "agent_memory_update_result"),
  };
}

export function parseAgentMemoryRecordMeta(raw: unknown): AgentMemoryRecordMeta {
  const d = asRecord(raw);
  const id = requiredStringField(d, "id", "agent_memory_record_meta");
  const kind = requiredStringField(d, "kind", "agent_memory_record_meta");
  const title = requiredStringField(d, "title", "agent_memory_record_meta");
  const description = d.description ?? "";
  if (typeof description !== "string") {
    throw new Error("agent_memory_record_meta.description must be a string");
  }
  const ageDays = requiredNumberField(d, "age_days", "agent_memory_record_meta");
  const rank = d.rank ?? null;
  if (rank !== null && typeof rank !== "number") {
    throw new Error("agent_memory_record_meta.rank must be a number");
  }
  return { id, kind, title, description, ageDays, rank };
}

export function parseAgentMemoryManifestResult(raw: unknown): AgentMemoryManifestResult {
  const d = asRecord(raw);
  if (!Array.isArray(d.records)) {
    throw new Error("agent_memory_manifest_result.records must be an array");
  }
  return {
    records: d.records.map(parseAgentMemoryRecordMeta),
  };
}

// -- MemoryStoreInfo ------------------------------------------------------

export interface MemoryStoreInfo {
  readonly store: string;
  readonly recordCount: number;
}

export function parseMemoryStoreInfo(raw: unknown): MemoryStoreInfo {
  const d = asRecord(raw);
  return {
    store: String(d.store ?? ""),
    recordCount: Number(d.record_count ?? 0),
  };
}

// -- MemoryIndexResult ----------------------------------------------------

export interface MemoryIndexResult {
  readonly entity: string;
  readonly topic: string;
  readonly store: string;
  readonly assertionId: string | null;
  readonly conflictActive: boolean;
}

export function parseMemoryIndexResult(raw: unknown): MemoryIndexResult {
  const d = asRecord(raw);
  return {
    entity: String(d.entity ?? ""),
    topic: String(d.topic ?? ""),
    store: String(d.store ?? ""),
    assertionId: typeof d.assertion_id === "string" ? d.assertion_id : null,
    conflictActive: d.conflict_active === true,
  };
}

// -- CallToolResult -------------------------------------------------------

export interface CallToolResult {
  readonly moduleId: string;
  readonly tool: string;
  readonly result: unknown;
}

export function parseCallToolResult(raw: unknown): CallToolResult {
  const d = asRecord(raw);
  return {
    moduleId: String(d.module_id ?? ""),
    tool: String(d.tool ?? ""),
    result: d.result,
  };
}

// -- MemberSnapshot -------------------------------------------------------

export interface MemberSnapshot {
  readonly agentIdentity: string;
  readonly role: string;
  /**
   * One of {@link MEMBER_STATE_ACTIVE}, {@link MEMBER_STATE_RETIRING},
   * {@link MEMBER_STATE_BROKEN}, {@link MEMBER_STATE_COMPLETED}, or
   * {@link MEMBER_STATE_UNKNOWN}. The underlying `MobMemberStatus` is
   * `#[non_exhaustive]`, so branch on the known values and tolerate future ones.
   */
  readonly state: string;
  readonly wiredTo: readonly string[];
  readonly labels: Readonly<Record<string, string>>;
}

export function parseMemberSnapshot(raw: unknown): MemberSnapshot {
  const d = asRecord(raw);
  return {
    agentIdentity: String(d.agent_identity ?? ""),
    role: String(d.role ?? ""),
    state: String(d.state ?? ""),
    wiredTo: asStringArray(d.wired_to),
    labels: asStringRecord(d.labels),
  };
}

// -- MemberReloadResult / MemberHealth --------------------------------------

/**
 * Result of `meta/reload_member` (non-destructive cold reload).
 *
 * `disposition` is `"discarded"` (the live registration was replaced by a
 * re-materialization of the SAME durable session), `"not_degraded"` (a
 * success no-op; only produced once meerkat exposes durability state),
 * `"reattached"` (the registration had lost its runtime attachment and was
 * re-attached to the same durable session) or
 * `"not_current"` (nothing live to reload). `sessionId` and `generation` are
 * the binding AFTER the reload; a reload never advances the generation.
 * Tolerate future disposition strings.
 */
export interface MemberReloadResult {
  readonly reloaded: boolean;
  readonly disposition: string;
  readonly sessionId: string | null;
  readonly generation: number | null;
  readonly identity: string | null;
}

export function parseMemberReloadResult(raw: unknown): MemberReloadResult {
  const d = asRecord(raw);
  return {
    reloaded: Boolean(d.reloaded ?? false),
    disposition: String(d.disposition ?? ""),
    sessionId: typeof d.session_id === "string" ? d.session_id : null,
    generation: typeof d.generation === "number" ? d.generation : null,
    identity: typeof d.identity === "string" ? d.identity : null,
  };
}

/**
 * Result of `meta/member_health`: lifecycle and delivery health read
 * in-process, so it answers while the mob actor loop is stalled.
 *
 * `actorLoop.state` is `"live" | "stalled" | "terminated" | "unobserved"`;
 * `openStallId` is set while a stall is open. `durability` is `"ready"` or
 * `{ reload_required: { operation, reason } }` once the gateway can observe
 * it, and `null` before that. `generation` is Core's member generation.
 */
export interface MemberHealth {
  readonly identity: string;
  readonly memberId: string | null;
  readonly state: string;
  readonly bootstrapState: string | null;
  readonly materializationInFlight: boolean;
  readonly sessionId: string | null;
  readonly generation: number | null;
  readonly actorLoop: Readonly<Record<string, unknown>>;
  readonly openStallId: number | null;
  readonly lastDeliveryError: Readonly<Record<string, unknown>> | null;
  /** Most recent reload attempt: `{outcome: "discarded" | "reattached" | "not_degraded" |
   *  "not_current" | "refused" | "timed_out" | "failed", detail?, at_unix_ms}`;
   *  `refused` carries meerkat's reason (store not healthy). */
  readonly lastReload: Readonly<Record<string, unknown>> | null;
  readonly durability: unknown | null;
  /**
   * The identity's last refused wake, as on `IdentityStatus`; `null` when its
   * last wake was not refused.
   */
  readonly wakeRefusal: WakeRefusal | null;
}

export function parseMemberHealth(raw: unknown): MemberHealth {
  const d = asRecord(raw);
  const actorLoop =
    d.actor_loop !== null && typeof d.actor_loop === "object"
      ? (d.actor_loop as Record<string, unknown>)
      : { state: "unobserved" };
  const lastError =
    d.last_delivery_error !== null && typeof d.last_delivery_error === "object"
      ? (d.last_delivery_error as Record<string, unknown>)
      : null;
  return {
    identity: String(d.identity ?? ""),
    memberId: typeof d.member_id === "string" ? d.member_id : null,
    state: String(d.state ?? ""),
    bootstrapState: typeof d.bootstrap_state === "string" ? d.bootstrap_state : null,
    materializationInFlight: Boolean(d.materialization_in_flight ?? false),
    sessionId: typeof d.session_id === "string" ? d.session_id : null,
    generation: typeof d.generation === "number" ? d.generation : null,
    actorLoop,
    openStallId: typeof d.open_stall_id === "number" ? d.open_stall_id : null,
    lastDeliveryError: lastError,
    lastReload:
      d.last_reload !== null && typeof d.last_reload === "object"
        ? (d.last_reload as Record<string, unknown>)
        : null,
    durability: d.durability === undefined ? null : d.durability,
    wakeRefusal:
      d.wake_refusal !== null && typeof d.wake_refusal === "object"
        ? parseWakeRefusal(d.wake_refusal)
        : null,
  };
}

// -- RuntimeRouteResult ---------------------------------------------------

export interface RuntimeRouteResult {
  readonly routeKey: string;
  readonly recipient: string;
  readonly channel: string | null;
  readonly sink: string;
  readonly targetModule: string;
}

export function parseRuntimeRouteResult(raw: unknown): RuntimeRouteResult {
  const d = asRecord(raw);
  return {
    routeKey: String(d.route_key ?? ""),
    recipient: String(d.recipient ?? ""),
    channel: typeof d.channel === "string" ? d.channel : null,
    sink: String(d.sink ?? ""),
    targetModule: String(d.target_module ?? ""),
  };
}

// -- GatingEvaluateResult -------------------------------------------------

export interface GatingEvaluateResult {
  readonly actionId: string;
  readonly action: string;
  readonly actorId: string;
  readonly riskTier: string | null;
  readonly outcome: string;
  readonly pendingId: string | null;
}

export function parseGatingEvaluateResult(
  raw: unknown,
): GatingEvaluateResult {
  const d = asRecord(raw);
  return {
    actionId: String(d.action_id ?? ""),
    action: String(d.action ?? ""),
    actorId: String(d.actor_id ?? ""),
    riskTier: typeof d.risk_tier === "string" ? d.risk_tier : null,
    outcome: String(d.outcome ?? ""),
    pendingId: typeof d.pending_id === "string" ? d.pending_id : null,
  };
}

// -- GatingDecisionResult -------------------------------------------------

export interface GatingDecisionResult {
  readonly pendingId: string;
  readonly actionId: string;
  readonly decision: string;
  readonly approverId?: string | null;
  readonly outcome?: string | null;
  readonly decidedAtMs?: number | null;
  readonly reason?: string | null;
  readonly nextPendingId?: string | null;
}

export function parseGatingDecisionResult(
  raw: unknown,
): GatingDecisionResult {
  const d = asRecord(raw);
  return {
    pendingId: String(d.pending_id ?? ""),
    actionId: String(d.action_id ?? ""),
    decision: String(d.decision ?? ""),
    approverId: typeof d.approver_id === "string" ? d.approver_id : null,
    outcome: typeof d.outcome === "string" ? d.outcome : null,
    decidedAtMs: typeof d.decided_at_ms === "number"
      && Number.isSafeInteger(d.decided_at_ms) && d.decided_at_ms >= 0
      ? d.decided_at_ms : null,
    reason: typeof d.reason === "string" ? d.reason : null,
    nextPendingId: typeof d.next_pending_id === "string" ? d.next_pending_id : null,
  };
}

// -- GatingAuditEntry -----------------------------------------------------

export interface GatingAuditEntry {
  readonly auditId: string;
  readonly timestampMs: number;
  readonly eventType: string;
  readonly actionId: string;
  readonly actorId: string;
  readonly riskTier: string | null;
  readonly outcome: string;
  /** The pending entry this record belongs to, when the owner recorded one. */
  readonly pendingId?: string | null;
  /** The owner's event detail as returned; empty when absent or malformed. */
  readonly detail?: Readonly<Record<string, unknown>>;
}

export function parseGatingAuditEntry(raw: unknown): GatingAuditEntry {
  const d = asRecord(raw);
  const detail = d.detail;
  return {
    auditId: String(d.audit_id ?? ""),
    timestampMs: Number(d.timestamp_ms ?? 0),
    eventType: String(d.event_type ?? ""),
    actionId: String(d.action_id ?? ""),
    actorId: String(d.actor_id ?? ""),
    riskTier: typeof d.risk_tier === "string" ? d.risk_tier : null,
    outcome: String(d.outcome ?? ""),
    pendingId: typeof d.pending_id === "string" && d.pending_id.trim() ? d.pending_id : null,
    detail: typeof detail === "object" && detail !== null && !Array.isArray(detail)
      ? { ...(detail as Record<string, unknown>) } : {},
  };
}

// -- GatingOrigin ---------------------------------------------------------

/** Where a gated action was requested from, as recorded by the owner. */
export interface GatingOrigin {
  readonly identity: string;
  readonly conversationId: string | null;
  readonly interactionId: string | null;
}

/** Returns the origin, or null when it is absent or has no identity. */
export function parseGatingOrigin(raw: unknown): GatingOrigin | null {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    return null;
  }
  const d = raw as Record<string, unknown>;
  const text = (value: unknown): string | null =>
    typeof value === "string" && value.trim() ? value : null;
  const identity = text(d.identity);
  if (identity === null) {
    return null;
  }
  return {
    identity,
    conversationId: text(d.conversation_id),
    interactionId: text(d.interaction_id),
  };
}

// -- GatingPendingEntry ---------------------------------------------------

export interface GatingPendingEntry {
  readonly pendingId: string;
  readonly actionId: string;
  readonly action: string;
  readonly actorId: string;
  readonly riskTier: string | null;
  readonly createdAtMs: number;
  readonly requestedApprover?: string | null;
  readonly approvalRecipient?: string | null;
  readonly approvalChannel?: string | null;
  readonly approvalRouteId?: string | null;
  readonly approvalDeliveryId?: string | null;
  readonly deadlineAtMs?: number | null;
  readonly rationale?: string | null;
  readonly origin?: GatingOrigin | null;
}

export function parseGatingPendingEntry(raw: unknown): GatingPendingEntry {
  const d = asRecord(raw);
  const text = (value: unknown): string | null =>
    typeof value === "string" && value.trim() ? value : null;
  return {
    pendingId: String(d.pending_id ?? ""),
    actionId: String(d.action_id ?? ""),
    action: String(d.action ?? ""),
    actorId: String(d.actor_id ?? ""),
    riskTier: typeof d.risk_tier === "string" ? d.risk_tier : null,
    createdAtMs: Number(d.created_at_ms ?? 0),
    requestedApprover: text(d.requested_approver),
    approvalRecipient: text(d.approval_recipient),
    approvalChannel: text(d.approval_channel),
    approvalRouteId: text(d.approval_route_id),
    approvalDeliveryId: text(d.approval_delivery_id),
    deadlineAtMs: typeof d.deadline_at_ms === "number"
      && Number.isSafeInteger(d.deadline_at_ms) && d.deadline_at_ms >= 0
      ? d.deadline_at_ms : null,
    rationale: text(d.rationale),
    origin: parseGatingOrigin(d.origin),
  };
}

// -- ReconcileEdgesReport -------------------------------------------------

export interface ReconcileEdgesReport {
  readonly desiredEdges: readonly Record<string, unknown>[];
  readonly wiredEdges: readonly Record<string, unknown>[];
  readonly unwiredEdges: readonly Record<string, unknown>[];
  readonly retainedEdges: readonly Record<string, unknown>[];
  readonly preexistingEdges: readonly Record<string, unknown>[];
  readonly skippedMissingMembers: readonly Record<string, unknown>[];
  readonly prunedStaleManagedEdges: readonly Record<string, unknown>[];
  readonly failures: readonly Record<string, unknown>[];
  readonly isComplete: boolean;
  /**
   * False for a candidate launch's dry run: nothing was wired, unwired or
   * pruned, and the edge lists name what would be. Optional, so a report a
   * caller builds still type-checks: a missing field means applied (`true`),
   * and `parseReconcileEdgesReport` always sets it.
   */
  readonly applied?: boolean;
}

export function parseReconcileEdgesReport(
  raw: unknown,
): ReconcileEdgesReport {
  const d = asRecord(raw);
  const failures = asRecordArray(d.failures);
  const skipped = asRecordArray(d.skipped_missing_members);
  return {
    desiredEdges: asRecordArray(d.desired_edges),
    wiredEdges: asRecordArray(d.wired_edges),
    unwiredEdges: asRecordArray(d.unwired_edges),
    retainedEdges: asRecordArray(d.retained_edges),
    preexistingEdges: asRecordArray(d.preexisting_edges),
    skippedMissingMembers: skipped,
    prunedStaleManagedEdges: asRecordArray(d.pruned_stale_managed_edges),
    failures,
    isComplete: failures.length === 0 && skipped.length === 0,
    applied: d.applied !== false,
  };
}

// -- RediscoverReport -----------------------------------------------------

export interface RediscoverReport {
  readonly spawned: readonly string[];
  readonly edges: ReconcileEdgesReport;
}

export function parseRediscoverReport(raw: unknown): RediscoverReport {
  const d = asRecord(raw);
  return {
    spawned: asStringArray(d.spawned),
    edges: parseReconcileEdgesReport(d.edges),
  };
}

// -- Unified events (persisted event log) ---------------------------------

export interface UnifiedAgentEvent {
  readonly kind: "agent";
  readonly agentId: string;
  readonly eventType: string;
  readonly payload?: Record<string, unknown> | null;
}

export interface UnifiedModuleEvent {
  readonly kind: "module";
  readonly module: string;
  readonly eventType: string;
  readonly payload: Record<string, unknown>;
}

export type UnifiedEvent = UnifiedAgentEvent | UnifiedModuleEvent;

function parseUnifiedEvent(raw: unknown): UnifiedEvent {
  const d = asRecord(raw);
  if (d.kind === "agent") {
    return {
      kind: "agent",
      agentId: String(d.agent_id ?? d.agentId ?? ""),
      eventType: String(d.event_type ?? d.eventType ?? ""),
      payload:
        typeof d.payload === "object" && d.payload !== null
          ? asRecord(d.payload)
          : null,
    };
  }
  if (d.kind === "module") {
    return {
      kind: "module",
      module: String(d.module ?? ""),
      eventType: String(d.event_type ?? d.eventType ?? ""),
      payload: asRecord(d.payload),
    };
  }
  if ("Agent" in d) {
    const agent = asRecord(d.Agent);
    return {
      kind: "agent",
      agentId: String(agent.agent_id ?? ""),
      eventType: String(agent.event_type ?? ""),
      payload:
        typeof agent.payload === "object" && agent.payload !== null
          ? asRecord(agent.payload)
          : null,
    };
  }
  if ("Module" in d) {
    const mod = asRecord(d.Module);
    return {
      kind: "module",
      module: String(mod.module ?? ""),
      eventType: String(mod.event_type ?? ""),
      payload: asRecord(mod.payload),
    };
  }
  return {
    kind: "module",
    module: "unknown",
    eventType: "unknown",
    payload: asRecord(raw),
  };
}

// -- PersistedEvent -------------------------------------------------------

export interface PersistedEvent {
  readonly id: string;
  readonly seq: number;
  readonly timestampMs: number;
  readonly memberId: string | null;
  readonly event: UnifiedEvent;
}

export function parsePersistedEvent(raw: unknown): PersistedEvent {
  const d = asRecord(raw);
  const rawEvent = d.event;
  const event =
    typeof rawEvent === "object" && rawEvent !== null
      ? parseUnifiedEvent(rawEvent)
      : ({ kind: "module", module: "unknown", eventType: "unknown", payload: {} } as UnifiedModuleEvent);
  return {
    id: String(d.id ?? ""),
    seq: Number(d.seq ?? 0),
    timestampMs: Number(d.timestamp_ms ?? 0),
    memberId: typeof d.member_id === "string" ? d.member_id : null,
    event,
  };
}

// -- EventQuery -----------------------------------------------------------

/**
 * Query parameters for historical event retrieval. `afterSeq` is the
 * pagination cursor — pass the highest seen `seq`/`cursor` to receive
 * only strictly-newer events. `mobId`, `runId`, `stepId`, `identity`
 * filter the structural mob-events surface (`meta/mob_events/query`).
 */
export interface EventQuery {
  readonly sinceMs?: number;
  readonly untilMs?: number;
  readonly memberId?: string;
  readonly identity?: string;
  readonly mobId?: string;
  readonly runId?: string;
  readonly stepId?: string;
  readonly eventTypes?: readonly string[];
  readonly limit?: number;
  readonly afterSeq?: number;
}

export function eventQueryToDict(query: EventQuery): Record<string, unknown> {
  const d: Record<string, unknown> = {};
  if (query.sinceMs !== undefined) d.since_ms = query.sinceMs;
  if (query.untilMs !== undefined) d.until_ms = query.untilMs;
  if (query.memberId !== undefined) d.member_id = query.memberId;
  if (query.identity !== undefined) d.identity = query.identity;
  if (query.mobId !== undefined) d.mob_id = query.mobId;
  if (query.runId !== undefined) d.run_id = query.runId;
  if (query.stepId !== undefined) d.step_id = query.stepId;
  if (query.eventTypes !== undefined && query.eventTypes.length > 0) {
    d.event_types = [...query.eventTypes];
  }
  if (query.limit !== undefined) d.limit = query.limit;
  if (query.afterSeq !== undefined) d.after_seq = query.afterSeq;
  return d;
}

// -- MobStructuralEvent ---------------------------------------------------

/**
 * Structural mob event projected from `MobEventKind`. Preserves
 * `mobId`/`runId`/`stepId`/`agentIdentity` that the legacy lossy
 * `UnifiedEvent::Agent` projection discards. Use `cursor` as the
 * `EventQuery.afterSeq` pagination token on the next request.
 */
export interface MobStructuralEvent {
  readonly eventId: string;
  readonly cursor: number;
  readonly mobId: string;
  readonly timestampMs: number;
  readonly kind: string;
  readonly runId: string | null;
  readonly stepId: string | null;
  readonly agentIdentity: string | null;
  readonly mobLabels: Readonly<Record<string, string>>;
  readonly runLabels: Readonly<Record<string, string>>;
  readonly data: Record<string, unknown>;
}

export function parseMobStructuralEvent(raw: unknown): MobStructuralEvent {
  const d = asRecord(raw);
  const data = d.data;
  return {
    eventId: String(d.event_id ?? ""),
    cursor: Number(d.cursor ?? 0),
    mobId: String(d.mob_id ?? ""),
    timestampMs: Number(d.timestamp_ms ?? 0),
    kind: String(d.kind ?? ""),
    runId: typeof d.run_id === "string" ? d.run_id : null,
    stepId: typeof d.step_id === "string" ? d.step_id : null,
    agentIdentity:
      typeof d.agent_identity === "string" ? d.agent_identity : null,
    mobLabels: asStringRecord(d.mob_labels),
    runLabels: asStringRecord(d.run_labels),
    data:
      typeof data === "object" && data !== null
        ? (data as Record<string, unknown>)
        : {},
  };
}

// -- MobRun (full ledger projection) --------------------------------------

/** Lifecycle states for a flow run; mirrors meerkat's `MobRunStatus`. */
export type MobRunStatus =
  | "pending"
  | "running"
  | "completed"
  | "failed"
  | "canceled";

/** Per-target step execution ledger entry; mirrors `StepLedgerEntry`. */
export interface StepRecord {
  readonly stepId: string;
  readonly agentIdentity: string;
  readonly status: string;
  readonly output: unknown;
  readonly timestamp: string;
}

export function parseStepRecord(raw: unknown): StepRecord {
  const d = asRecord(raw);
  return {
    stepId: String(d.step_id ?? ""),
    agentIdentity: String(d.agent_identity ?? ""),
    status: String(d.status ?? ""),
    output: d.output,
    timestamp: String(d.timestamp ?? ""),
  };
}

/** Flow-level failure log entry; mirrors `FailureLedgerEntry`. */
export interface FailureRecord {
  readonly stepId: string;
  readonly reason: string;
  readonly timestamp: string;
}

export function parseFailureRecord(raw: unknown): FailureRecord {
  const d = asRecord(raw);
  return {
    stepId: String(d.step_id ?? ""),
    reason: String(d.reason ?? ""),
    timestamp: String(d.timestamp ?? ""),
  };
}

/** Per-frame kernel snapshot; mirrors `FrameSnapshot`.
 *
 * `kernelState` is meerkat-internal `flow_frame::State` and passes
 * through as `unknown`. */
export interface FrameRecord {
  readonly kernelState: unknown;
}

export function parseFrameRecord(raw: unknown): FrameRecord {
  const d = asRecord(raw);
  return { kernelState: d.kernel_state };
}

/** Per-loop kernel snapshot; mirrors `LoopSnapshot`.
 *
 * `kernelState` is meerkat-internal `loop_iteration::State` and passes
 * through as `unknown`. */
export interface LoopRecord {
  readonly kernelState: unknown;
}

export function parseLoopRecord(raw: unknown): LoopRecord {
  const d = asRecord(raw);
  return { kernelState: d.kernel_state };
}

/** Loop-iteration → body-frame ledger entry; mirrors `LoopIterationLedgerEntry`. */
export interface LoopIterationRecord {
  readonly loopInstanceId: string;
  readonly iteration: number;
  readonly frameId: string;
}

export function parseLoopIterationRecord(raw: unknown): LoopIterationRecord {
  const d = asRecord(raw);
  return {
    loopInstanceId: String(d.loop_instance_id ?? ""),
    iteration: Number(d.iteration ?? 0),
    frameId: String(d.frame_id ?? ""),
  };
}

/**
 * Persisted flow run aggregate returned by `MobHandle.listRuns`.
 *
 * Carries the full meerkat ledger projection. Meerkat-internal
 * sub-shapes (`flowState`, `activationParams`, `StepRecord.output`,
 * `rootStepOutputs` / `loopIterationOutputs` value blobs, frame /
 * loop `kernelState`) pass through as `unknown` rather than being
 * re-typed in the SDK. `frames` and `loops` are **maps**, not arrays.
 */
export interface MobRun {
  readonly runId: string;
  readonly mobId: string;
  readonly flowId: string;
  readonly status: MobRunStatus;
  readonly flowState: unknown;
  readonly activationParams: unknown;
  readonly createdAt: string;
  readonly completedAt: string | null;
  readonly stepLedger: StepRecord[];
  readonly failureLedger: FailureRecord[];
  readonly frames: Readonly<Record<string, FrameRecord>>;
  readonly loops: Readonly<Record<string, LoopRecord>>;
  readonly loopIterationLedger: LoopIterationRecord[];
  readonly schemaVersion: number;
  readonly rootStepOutputs: Readonly<Record<string, unknown>>;
  readonly loopIterationOutputs: Readonly<Record<string, unknown>>;
  /**
   * The members each role of a role-targeted run dispatches to, as the
   * runtime recorded them; absent for a role-wide run, and for a run read
   * from a gateway or runtime that predates role targets. A gate that needs
   * a bound run must treat its absence as not authorized.
   */
  readonly roleTargets?: Readonly<Record<string, readonly string[]>>;
}

function parseMobRunStatus(raw: unknown): MobRunStatus {
  const known: readonly MobRunStatus[] = [
    "pending",
    "running",
    "completed",
    "failed",
    "canceled",
  ];
  if (typeof raw === "string" && (known as readonly string[]).includes(raw)) {
    return raw as MobRunStatus;
  }
  return "pending";
}

export function parseMobRun(raw: unknown): MobRun {
  const d = asRecord(raw);
  const stepLedger = Array.isArray(d.step_ledger)
    ? d.step_ledger.map(parseStepRecord)
    : [];
  const failureLedger = Array.isArray(d.failure_ledger)
    ? d.failure_ledger.map(parseFailureRecord)
    : [];
  const iterations = Array.isArray(d.loop_iteration_ledger)
    ? d.loop_iteration_ledger.map(parseLoopIterationRecord)
    : [];
  const framesIn =
    typeof d.frames === "object" && d.frames !== null
      ? (d.frames as Record<string, unknown>)
      : {};
  const frames: Record<string, FrameRecord> = {};
  for (const [k, v] of Object.entries(framesIn)) {
    if (typeof v === "object" && v !== null) {
      frames[k] = parseFrameRecord(v);
    }
  }
  const loopsIn =
    typeof d.loops === "object" && d.loops !== null
      ? (d.loops as Record<string, unknown>)
      : {};
  const loops: Record<string, LoopRecord> = {};
  for (const [k, v] of Object.entries(loopsIn)) {
    if (typeof v === "object" && v !== null) {
      loops[k] = parseLoopRecord(v);
    }
  }
  const rootOutputs =
    typeof d.root_step_outputs === "object" && d.root_step_outputs !== null
      ? (d.root_step_outputs as Record<string, unknown>)
      : {};
  const iterOutputs =
    typeof d.loop_iteration_outputs === "object" &&
    d.loop_iteration_outputs !== null
      ? (d.loop_iteration_outputs as Record<string, unknown>)
      : {};
  return {
    runId: String(d.run_id ?? ""),
    mobId: String(d.mob_id ?? ""),
    flowId: String(d.flow_id ?? ""),
    status: parseMobRunStatus(d.status),
    flowState: d.flow_state,
    activationParams: d.activation_params,
    createdAt: String(d.created_at ?? ""),
    completedAt:
      d.completed_at === null || d.completed_at === undefined
        ? null
        : String(d.completed_at),
    stepLedger,
    failureLedger,
    frames,
    loops,
    loopIterationLedger: iterations,
    schemaVersion: Number(d.schema_version ?? 0),
    rootStepOutputs: rootOutputs,
    loopIterationOutputs: iterOutputs,
    ...roleTargetsField(d.role_targets),
  };
}

/**
 * A run record's `role_targets` as an optional `roleTargets` field: present
 * only for a role-targeted run.
 */
function roleTargetsField(
  raw: unknown,
): { roleTargets?: Readonly<Record<string, readonly string[]>> } {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
  const roleTargets: Record<string, readonly string[]> = {};
  for (const [role, members] of Object.entries(raw as Record<string, unknown>)) {
    if (Array.isArray(members)) {
      roleTargets[role] = members.map((member) => String(member));
    }
  }
  return { roleTargets };
}

// -- Rich member / helper / catalog / cross-mob results --------------------

export type MobMemberStatus =
  | "active"
  | "retiring"
  | "broken"
  | "completed"
  | "unknown";

export interface MobUnreachablePeer {
  readonly peer: string;
  readonly reason: string | null;
}

export function parseMobUnreachablePeer(raw: unknown): MobUnreachablePeer {
  const d = asRecord(raw);
  return {
    peer: String(d.peer ?? ""),
    reason: typeof d.reason === "string" ? d.reason : null,
  };
}

/**
 * Live connectivity for a member's wired peers.
 *
 * meerkat 0.7.x projects this as a tri-state, internally-tagged object:
 * `{"status": "known", "snapshot": {...}}` carries the counts, while
 * `{"status": "not_applicable"}` (no bridge session backs the member) and
 * `{"status": "probe_timed_out"}` (the live probe did not resolve in time)
 * carry no counts. `status` distinguishes the three; the counts are only
 * meaningful when `status === "known"`. The legacy flat shape (counts at the
 * top level, no `status`) is still accepted for backward compatibility.
 */
export interface PeerConnectivitySnapshot {
  readonly status: string;
  readonly reachablePeerCount: number;
  readonly unknownPeerCount: number;
  readonly unreachablePeers: readonly MobUnreachablePeer[];
}

export function parsePeerConnectivitySnapshot(raw: unknown): PeerConnectivitySnapshot {
  const d = asRecord(raw);
  // 0.7.x tri-state: read counts from `.snapshot` behind the `status`
  // discriminator. `not_applicable` / `probe_timed_out` carry no snapshot.
  const status = String(d.status ?? "known");
  const counts =
    typeof d.snapshot === "object" && d.snapshot !== null
      ? asRecord(d.snapshot)
      : d;
  return {
    status,
    reachablePeerCount: Number(counts.reachable_peer_count ?? 0),
    unknownPeerCount: Number(counts.unknown_peer_count ?? 0),
    unreachablePeers: asRecordArray(counts.unreachable_peers).map(parseMobUnreachablePeer),
  };
}

/**
 * Machine-owned live execution/progress projection (meerkat 0.7.29).
 *
 * `runState` is `idle`/`run_open`/`unknown`; `health` is
 * `healthy`/`degraded`/`wedged`/`unknown`; `lastProgressEvent` is
 * `execution_advanced`/`became_idle`/`unchanged`. All three are open
 * vocabularies — tolerate future values.
 */
export interface MemberProgressSnapshot {
  readonly runState: string;
  readonly inFlightWork: number;
  readonly lastProgressAtMs: number;
  readonly lastProgressEvent: string;
  readonly health: string;
}

export function parseMemberProgressSnapshot(raw: unknown): MemberProgressSnapshot {
  const d = asRecord(raw);
  return {
    runState: String(d.run_state ?? "unknown"),
    inFlightWork: Number(d.in_flight_work ?? 0),
    lastProgressAtMs: Number(d.last_progress_at_ms ?? 0),
    lastProgressEvent: String(d.last_progress_event ?? "unchanged"),
    health: String(d.health ?? "unknown"),
  };
}

export interface RichMemberSnapshot {
  readonly status: string;
  readonly outputPreview: string | null;
  readonly error: string | null;
  readonly tokensUsed: number;
  readonly isFinal: boolean;
  readonly currentSessionId: string | null;
  readonly peerConnectivity: PeerConnectivitySnapshot | null;
  readonly progress: MemberProgressSnapshot | null;
}

export interface IdentityResolvedToolsResult {
  readonly identity: string;
  readonly sessionId: string;
  readonly tools: readonly string[];
}

export function parseIdentityResolvedToolsResult(raw: unknown): IdentityResolvedToolsResult {
  const d = asRecord(raw);
  return {
    identity: String(d.identity ?? ""),
    sessionId: String(d.session_id ?? ""),
    tools: asStringArray(d.tools),
  };
}

/**
 * Model routing status from `meta/identity/routing_status`.
 *
 * Relays meerkat's `SessionModelRoutingStatus` unchanged. The override
 * summaries stay as opaque records on purpose: re-declaring meerkat's
 * `ScopedModelOverrideSummary` here would create a second owner of a shape this
 * SDK only forwards.
 */
export interface IdentityRoutingStatusResult {
  readonly identity: string;
  readonly sessionId: string;
  readonly baselineModel: string;
  readonly effectiveModel: string;
  /**
   * Typed provider of the session's current LLM identity.
   *
   * `null` means the runtime machine has no hydrated session LLM identity yet
   * (pre-hydration). It is NOT a signal to re-derive a provider from
   * `effectiveModel`: meerkat documents that re-derivation as silently wrong
   * for models owned by a custom `ModelRegistry`, which is precisely why this
   * field is carried typed rather than inferred.
   */
  readonly sessionProvider: string | null;
  readonly activeTurnOverride: Record<string, unknown> | null;
  readonly activeOperationOverride: Record<string, unknown> | null;
  readonly pendingSwitchTurn: Record<string, unknown> | null;
}

export function parseIdentityRoutingStatusResult(
  raw: unknown,
): IdentityRoutingStatusResult {
  const d = asRecord(raw);
  const optObject = (key: string): Record<string, unknown> | null => {
    const value = d[key];
    return value !== null && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : null;
  };
  return {
    identity: String(d.identity ?? ""),
    sessionId: String(d.session_id ?? ""),
    baselineModel: String(d.baseline_model ?? ""),
    effectiveModel: String(d.effective_model ?? ""),
    sessionProvider:
      typeof d.session_provider === "string" ? d.session_provider : null,
    activeTurnOverride: optObject("active_turn_override"),
    activeOperationOverride: optObject("active_operation_override"),
    pendingSwitchTurn: optObject("pending_switch_turn"),
  };
}

export function parseRichMemberSnapshot(raw: unknown): RichMemberSnapshot {
  const d = asRecord(raw);
  return {
    status: String(d.status ?? "unknown"),
    outputPreview: typeof d.output_preview === "string" ? d.output_preview : null,
    error: typeof d.error === "string" ? d.error : null,
    tokensUsed: Number(d.tokens_used ?? 0),
    isFinal: Boolean(d.is_final),
    currentSessionId:
      typeof d.current_session_id === "string" ? d.current_session_id : null,
    peerConnectivity:
      typeof d.peer_connectivity === "object" && d.peer_connectivity !== null
        ? parsePeerConnectivitySnapshot(d.peer_connectivity)
        : null,
    progress:
      typeof d.progress === "object" && d.progress !== null
        ? parseMemberProgressSnapshot(d.progress)
        : null,
  };
}

export interface HelperResult {
  readonly output: string | null;
  readonly tokensUsed: number;
  readonly sessionId: string | null;
}

export function parseHelperResult(raw: unknown): HelperResult {
  const d = asRecord(raw);
  return {
    output: typeof d.output === "string" ? d.output : null,
    tokensUsed: Number(d.tokens_used ?? 0),
    sessionId: typeof d.session_id === "string" ? d.session_id : null,
  };
}

export interface MobRunSnapshot {
  readonly runId: string;
  readonly mobId: string;
  readonly flowId: string;
  readonly status: string;
  readonly stepLedger: readonly Record<string, unknown>[];
  readonly failureLedger: readonly Record<string, unknown>[];
  /**
   * The members each role of a role-targeted run dispatches to, as the
   * runtime recorded them; absent for a role-wide run.
   */
  readonly roleTargets?: Readonly<Record<string, readonly string[]>>;
}

export function parseMobRunSnapshot(raw: unknown): MobRunSnapshot {
  const d = asRecord(raw);
  return {
    runId: String(d.run_id ?? ""),
    mobId: String(d.mob_id ?? ""),
    flowId: String(d.flow_id ?? ""),
    status: String(d.status ?? "unknown"),
    stepLedger: asRecordArray(d.step_ledger),
    failureLedger: asRecordArray(d.failure_ledger),
    ...roleTargetsField(d.role_targets),
  };
}

export interface CrossMobContactEntry {
  readonly mobId: string;
  readonly transport: string;
}

export function parseCrossMobContactEntry(raw: unknown): CrossMobContactEntry {
  const d = asRecord(raw);
  let transport = d.transport;
  if (typeof transport === "object" && transport !== null) {
    const t = asRecord(transport);
    if (typeof t.Tcp === "string") transport = `tcp://${t.Tcp}`;
    else if (typeof t.Uds === "string") transport = `uds://${t.Uds}`;
    else transport = "inproc";
  } else if (transport === "Inproc") {
    transport = "inproc";
  }
  return {
    mobId: String(d.mob_id ?? ""),
    transport: String(transport ?? ""),
  };
}

export interface CatalogEntry {
  readonly id: string;
  readonly displayName: string;
  readonly provider: string;
  readonly tier: string;
  readonly contextWindow: number | null;
  readonly maxOutputTokens: number | null;
  readonly vision: boolean;
  readonly imageToolResults: boolean;
}

export function parseCatalogEntry(raw: unknown): CatalogEntry {
  const d = asRecord(raw);
  const profile = asRecord(d.profile);
  return {
    id: String(d.id ?? ""),
    displayName: String(d.display_name ?? ""),
    provider: String(d.provider ?? ""),
    tier: String(d.tier ?? ""),
    contextWindow: d.context_window == null ? null : Number(d.context_window),
    maxOutputTokens: d.max_output_tokens == null ? null : Number(d.max_output_tokens),
    vision: Boolean(profile.vision),
    imageToolResults: Boolean(profile.image_tool_results),
  };
}

export interface ProviderDefaults {
  readonly provider: string;
  readonly defaultModelId: string;
  readonly models: readonly CatalogEntry[];
}

export function parseProviderDefaults(raw: unknown): ProviderDefaults {
  const d = asRecord(raw);
  return {
    provider: String(d.provider ?? ""),
    defaultModelId: String(d.default_model_id ?? ""),
    models: asRecordArray(d.models).map(parseCatalogEntry),
  };
}

export interface ModelsCatalogResult {
  readonly models: readonly CatalogEntry[];
  readonly providerDefaults: readonly ProviderDefaults[];
}

export function parseModelsCatalogResult(raw: unknown): ModelsCatalogResult {
  const d = asRecord(raw);
  return {
    models: asRecordArray(d.models).map(parseCatalogEntry),
    providerDefaults: asRecordArray(d.provider_defaults).map(parseProviderDefaults),
  };
}

// -- Schedule tool definitions ---------------------------------------------

/** One of Meerkat's durable schedule tools, as `meta/schedule/tools` lists it. */
export interface ScheduleToolDefinition {
  readonly name: string;
  readonly description: string;
  /** The tool's JSON input schema, exactly as Meerkat defines it. */
  readonly inputSchema: Record<string, unknown>;
}

/**
 * Result of `meta/schedule/tools`: Meerkat's `ScheduleToolsResult`, the base
 * definitions of the `meerkat_schedule_*` tools a member whose profile sets
 * `tools.schedule = true` is given. Inside that member's session,
 * `meerkat_schedule_create` and `meerkat_schedule_update` also accept a
 * session target of `type: "current_session"`.
 */
export interface ScheduleToolsResult {
  readonly tools: readonly ScheduleToolDefinition[];
}

function isScheduleToolObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Parse a `meta/schedule/tools` result, rejecting any other shape. */
export function parseScheduleToolsResult(raw: unknown): ScheduleToolsResult {
  const invalid = () =>
    new Error(`invalid meta/schedule/tools result: ${JSON.stringify(raw)}`);
  if (!isScheduleToolObject(raw) || !Array.isArray(raw.tools)) {
    throw invalid();
  }
  const tools = raw.tools.map((tool: unknown): ScheduleToolDefinition => {
    if (
      !isScheduleToolObject(tool) ||
      typeof tool.name !== "string" ||
      typeof tool.description !== "string" ||
      !isScheduleToolObject(tool.inputSchema)
    ) {
      throw invalid();
    }
    return {
      name: tool.name,
      description: tool.description,
      inputSchema: tool.inputSchema,
    };
  });
  return { tools };
}

// -- Mobpack editor catalogs ----------------------------------------------

export interface MobpackToolsCatalogResult {
  readonly schemaVersion: string;
  readonly runtimeBacked: boolean;
  readonly source: string;
  readonly authoringProvider: Record<string, unknown>;
  readonly runtimeUnavailableReason: string | null;
  readonly toolCatalog: readonly Record<string, unknown>[];
}

export function parseMobpackToolsCatalogResult(
  raw: unknown,
): MobpackToolsCatalogResult {
  const d = asRecord(raw);
  return {
    schemaVersion: String(d.schema_version ?? ""),
    runtimeBacked: Boolean(d.runtime_backed),
    source: String(d.source ?? ""),
    authoringProvider: asRecord(d.authoring_provider),
    runtimeUnavailableReason:
      d.runtime_unavailable_reason == null
        ? null
        : String(d.runtime_unavailable_reason),
    toolCatalog: asRecordArray(d.tool_catalog),
  };
}

export interface MobpackSkillsCatalogResult {
  readonly schemaVersion: string;
  readonly runtimeBacked: boolean;
  readonly source: string;
  readonly authoringProvider: Record<string, unknown>;
  readonly runtimeUnavailableReason: string | null;
  readonly skillRealms: readonly Record<string, unknown>[];
}

export function parseMobpackSkillsCatalogResult(
  raw: unknown,
): MobpackSkillsCatalogResult {
  const d = asRecord(raw);
  return {
    schemaVersion: String(d.schema_version ?? ""),
    runtimeBacked: Boolean(d.runtime_backed),
    source: String(d.source ?? ""),
    authoringProvider: asRecord(d.authoring_provider),
    runtimeUnavailableReason:
      d.runtime_unavailable_reason == null
        ? null
        : String(d.runtime_unavailable_reason),
    skillRealms: asRecordArray(d.skill_realms),
  };
}

export interface MobpackAgentDefinitionsResult {
  readonly schemaVersion: string;
  readonly runtimeBacked: boolean;
  readonly source: string;
  readonly authoringProvider: Record<string, unknown>;
  readonly runtimeUnavailableReason: string | null;
  readonly agentDefinitions: readonly Record<string, unknown>[];
}

export function parseMobpackAgentDefinitionsResult(
  raw: unknown,
): MobpackAgentDefinitionsResult {
  const d = asRecord(raw);
  return {
    schemaVersion: String(d.schema_version ?? ""),
    runtimeBacked: Boolean(d.runtime_backed),
    source: String(d.source ?? ""),
    authoringProvider: asRecord(d.authoring_provider),
    runtimeUnavailableReason:
      d.runtime_unavailable_reason == null
        ? null
        : String(d.runtime_unavailable_reason),
    agentDefinitions: asRecordArray(d.agent_definitions),
  };
}

export interface MobpackTemplatesResult {
  readonly schemaVersion: string;
  readonly source: string;
  readonly authoringProvider: Record<string, unknown>;
  readonly runtimeUnavailableReason: string | null;
  readonly blankMobpack: Record<string, unknown> | null;
  readonly sampleMobpacks: readonly Record<string, unknown>[];
  readonly sampleAgentDefinitions: readonly Record<string, unknown>[];
  readonly templates: Record<string, unknown>;
}

export function parseMobpackTemplatesResult(raw: unknown): MobpackTemplatesResult {
  const d = asRecord(raw);
  return {
    schemaVersion: String(d.schema_version ?? ""),
    source: String(d.source ?? ""),
    authoringProvider: asRecord(d.authoring_provider),
    runtimeUnavailableReason:
      d.runtime_unavailable_reason == null
        ? null
        : String(d.runtime_unavailable_reason),
    blankMobpack: d.blank_mobpack == null ? null : asRecord(d.blank_mobpack),
    sampleMobpacks: asRecordArray(d.sample_mobpacks),
    sampleAgentDefinitions: asRecordArray(d.sample_agent_definitions),
    templates: asRecord(d.templates),
  };
}

export interface MobpackCatalogsResult {
  readonly schemaVersion: string;
  readonly runtimeBacked: boolean;
  readonly authoringProvider: Record<string, unknown>;
  readonly runtimeUnavailableReason: string | null;
  readonly sources: Record<string, unknown>;
  readonly templates: Record<string, unknown>;
  readonly toolCatalog: readonly Record<string, unknown>[];
  readonly skillRealms: readonly Record<string, unknown>[];
  readonly blankMobpack: Record<string, unknown> | null;
  readonly sampleMobpacks: readonly Record<string, unknown>[];
  readonly agentDefinitions: readonly Record<string, unknown>[];
  readonly sampleAgentDefinitions: readonly Record<string, unknown>[];
  readonly models: readonly CatalogEntry[];
  readonly providerDefaults: readonly ProviderDefaults[];
}

export function parseMobpackCatalogsResult(raw: unknown): MobpackCatalogsResult {
  const d = asRecord(raw);
  return {
    schemaVersion: String(d.schema_version ?? ""),
    runtimeBacked: Boolean(d.runtime_backed),
    authoringProvider: asRecord(d.authoring_provider),
    runtimeUnavailableReason:
      d.runtime_unavailable_reason == null
        ? null
        : String(d.runtime_unavailable_reason),
    sources: asRecord(d.sources),
    templates: asRecord(d.templates),
    toolCatalog: asRecordArray(d.tool_catalog),
    skillRealms: asRecordArray(d.skill_realms),
    blankMobpack: d.blank_mobpack == null ? null : asRecord(d.blank_mobpack),
    sampleMobpacks: asRecordArray(d.sample_mobpacks),
    agentDefinitions: asRecordArray(d.agent_definitions),
    sampleAgentDefinitions: asRecordArray(d.sample_agent_definitions),
    models: asRecordArray(d.models).map(parseCatalogEntry),
    providerDefaults: asRecordArray(d.provider_defaults).map(parseProviderDefaults),
  };
}

// -- Mobpack authoring ------------------------------------------------------

export interface MobpackDiagnostic {
  readonly severity: string;
  readonly code: string;
  readonly message: string;
  readonly path: string | null;
}

export function parseMobpackDiagnostic(raw: unknown): MobpackDiagnostic {
  const d = asRecord(raw);
  return {
    severity: String(d.severity ?? ""),
    code: String(d.code ?? ""),
    message: String(d.message ?? ""),
    path: d.path == null ? null : String(d.path),
  };
}

export interface MobpackDisplayRow {
  readonly kind: string;
  readonly glyph: string;
  readonly head: string;
  readonly sub: string;
  readonly meta: string;
}

export function parseMobpackDisplayRow(raw: unknown): MobpackDisplayRow {
  const d = asRecord(raw);
  return {
    kind: String(d.kind ?? ""),
    glyph: String(d.glyph ?? ""),
    head: String(d.head ?? ""),
    sub: String(d.sub ?? ""),
    meta: String(d.meta ?? ""),
  };
}

export interface MobpackValidationResult {
  readonly ok: boolean;
  readonly diagnostics: readonly MobpackDiagnostic[];
  readonly displayRows: readonly MobpackDisplayRow[];
  readonly mobId: string | null;
  readonly flowIds: readonly string[];
  readonly validationSource: string;
  readonly deployCommand: string;
}

export function parseMobpackValidationResult(
  raw: unknown,
): MobpackValidationResult {
  const d = asRecord(raw);
  return {
    ok: Boolean(d.ok),
    diagnostics: asRecordArray(d.diagnostics).map(parseMobpackDiagnostic),
    displayRows: asRecordArray(d.display_rows).map(parseMobpackDisplayRow),
    mobId: d.mob_id == null ? null : String(d.mob_id),
    flowIds: asStringArray(d.flow_ids),
    validationSource: String(d.validation_source ?? ""),
    deployCommand: String(d.deploy_command ?? ""),
  };
}

export interface MobpackSourceFile {
  readonly path: string;
  readonly mediaType: string;
  readonly sizeBytes: number;
  readonly contentBase64: string;
  readonly sha256: string;
  readonly text: string | null;
}

export function parseMobpackSourceFile(raw: unknown): MobpackSourceFile {
  const d = asRecord(raw);
  return {
    path: String(d.path ?? ""),
    mediaType: String(d.media_type ?? ""),
    sizeBytes: Number(d.size_bytes ?? 0),
    contentBase64: String(d.content_base64 ?? ""),
    sha256: String(d.sha256 ?? ""),
    text: d.text == null ? null : String(d.text),
  };
}

export interface MobpackSourceResult {
  readonly filename: string;
  readonly mediaType: string;
  readonly mobToml: string;
  readonly sourceFiles: readonly MobpackSourceFile[];
  readonly validation: MobpackValidationResult;
  readonly source: string;
}

export function parseMobpackSourceResult(raw: unknown): MobpackSourceResult {
  const d = asRecord(raw);
  return {
    filename: String(d.filename ?? ""),
    mediaType: String(d.media_type ?? ""),
    mobToml: String(d.mob_toml ?? ""),
    sourceFiles: asRecordArray(d.source_files).map(parseMobpackSourceFile),
    validation: parseMobpackValidationResult(d.validation),
    source: String(d.source ?? ""),
  };
}

export interface MobpackExportResult {
  readonly filename: string;
  readonly mediaType: string;
  readonly contentBase64: string;
  readonly mobToml: string;
  readonly sourceFiles: readonly MobpackSourceFile[];
  readonly validation: MobpackValidationResult;
}

export function parseMobpackExportResult(raw: unknown): MobpackExportResult {
  const d = asRecord(raw);
  return {
    filename: String(d.filename ?? ""),
    mediaType: String(d.media_type ?? ""),
    contentBase64: String(d.content_base64 ?? ""),
    mobToml: String(d.mob_toml ?? ""),
    sourceFiles: asRecordArray(d.source_files).map(parseMobpackSourceFile),
    validation: parseMobpackValidationResult(d.validation),
  };
}

export interface MobpackImportResult {
  readonly document: Record<string, unknown>;
  readonly validation: MobpackValidationResult;
  readonly source: string;
  readonly sourceLabel: string;
  readonly sourceMediaType: string;
}

export function parseMobpackImportResult(raw: unknown): MobpackImportResult {
  const d = asRecord(raw);
  return {
    document: asRecord(d.document),
    validation: parseMobpackValidationResult(d.validation),
    source: String(d.source ?? ""),
    sourceLabel: String(d.source_label ?? ""),
    sourceMediaType: String(d.source_media_type ?? ""),
  };
}

/**
 * A row from the mobpack draft registry. The `document` and `validation`
 * payloads are passed through as permissive records — the mobpack document
 * schema is opaque to the SDK.
 */
export interface MobpackDraftRow {
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly stage: string;
  readonly trigger: string;
  readonly source: string;
  readonly revision: number;
  readonly etag: string;
  readonly updatedAtUnixMs: number;
  readonly document: Record<string, unknown>;
  readonly validation: Record<string, unknown>;
  readonly canUndo: boolean | null;
  readonly canRedo: boolean | null;
}

export function parseMobpackDraftRow(raw: unknown): MobpackDraftRow {
  const d = asRecord(raw);
  return {
    id: String(d.id ?? ""),
    name: String(d.name ?? ""),
    version: String(d.version ?? ""),
    stage: String(d.stage ?? ""),
    trigger: String(d.trigger ?? ""),
    source: String(d.source ?? ""),
    revision: Number(d.revision ?? 0),
    etag: String(d.etag ?? ""),
    updatedAtUnixMs: Number(d.updated_at_unix_ms ?? 0),
    document: asRecord(d.document),
    validation: asRecord(d.validation),
    canUndo: d.can_undo == null ? null : Boolean(d.can_undo),
    canRedo: d.can_redo == null ? null : Boolean(d.can_redo),
  };
}

/**
 * A draft as draft results list it: its id, revision and etag and what the
 * Flow Editor library shows of it, without its document, archive or
 * validation. `meta/mobpacks/get` returns a draft's document.
 */
export interface MobpackDraftSummary {
  readonly id: string;
  readonly name: string;
  readonly version: string;
  readonly stage: string;
  readonly trigger: string;
  readonly source: string;
  readonly description: string;
  readonly revision: number;
  readonly etag: string;
  readonly updatedAtUnixMs: number;
  readonly canUndo: boolean;
  readonly canRedo: boolean;
}

export function parseMobpackDraftSummary(raw: unknown): MobpackDraftSummary {
  const d = asRecord(raw);
  return {
    id: String(d.id ?? ""),
    name: String(d.name ?? ""),
    version: String(d.version ?? ""),
    stage: String(d.stage ?? ""),
    trigger: String(d.trigger ?? ""),
    source: String(d.source ?? ""),
    description: String(d.description ?? ""),
    revision: Number(d.revision ?? 0),
    etag: String(d.etag ?? ""),
    updatedAtUnixMs: Number(d.updated_at_unix_ms ?? 0),
    canUndo: Boolean(d.can_undo),
    canRedo: Boolean(d.can_redo),
  };
}

export interface MobpackDraftListResult {
  readonly source: string;
  readonly storePath: string | null;
  readonly runtimeBacked: boolean;
  readonly rows: readonly MobpackDraftSummary[];
}

export function parseMobpackDraftListResult(
  raw: unknown,
): MobpackDraftListResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    storePath: d.store_path == null ? null : String(d.store_path),
    runtimeBacked: Boolean(d.runtime_backed),
    rows: asRecordArray(d.rows).map(parseMobpackDraftSummary),
  };
}

export interface MobpackDraftGetResult {
  readonly source: string;
  readonly storePath: string | null;
  readonly runtimeBacked: boolean;
  readonly row: MobpackDraftRow;
}

export function parseMobpackDraftGetResult(
  raw: unknown,
): MobpackDraftGetResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    storePath: d.store_path == null ? null : String(d.store_path),
    runtimeBacked: Boolean(d.runtime_backed),
    row: parseMobpackDraftRow(d.row),
  };
}

/** Result shape shared by meta/mobpacks/create and meta/mobpacks/save. */
export interface MobpackDraftSaveResult {
  readonly source: string;
  readonly storePath: string | null;
  readonly row: MobpackDraftRow;
  readonly rows: readonly MobpackDraftSummary[];
  /**
   * Set when Meta wrote the draft store but could not sync its directory:
   * the write took effect and may not survive a crash.
   */
  readonly durabilityWarning: string | null;
}

export function parseMobpackDraftSaveResult(
  raw: unknown,
): MobpackDraftSaveResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    storePath: d.store_path == null ? null : String(d.store_path),
    row: parseMobpackDraftRow(d.row),
    rows: asRecordArray(d.rows).map(parseMobpackDraftSummary),
    durabilityWarning: d.durability_warning == null ? null : String(d.durability_warning),
  };
}

/**
 * Result shape shared by meta/mobpacks/undo and meta/mobpacks/redo.
 * `stepped` is false (with a `reason`) when there is no history or future
 * entry to step to; the draft is left untouched in that case.
 */
export interface MobpackDraftHistoryResult {
  readonly source: string;
  readonly storePath: string | null;
  readonly stepped: boolean;
  readonly reason: string | null;
  readonly row: MobpackDraftRow;
  readonly rows: readonly MobpackDraftSummary[];
  /**
   * Set when Meta wrote the draft store but could not sync its directory:
   * the write took effect and may not survive a crash.
   */
  readonly durabilityWarning: string | null;
}

export function parseMobpackDraftHistoryResult(
  raw: unknown,
): MobpackDraftHistoryResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    storePath: d.store_path == null ? null : String(d.store_path),
    stepped: Boolean(d.stepped),
    reason: d.reason == null ? null : String(d.reason),
    row: parseMobpackDraftRow(d.row),
    rows: asRecordArray(d.rows).map(parseMobpackDraftSummary),
    durabilityWarning: d.durability_warning == null ? null : String(d.durability_warning),
  };
}

export interface MobpackDraftDeleteResult {
  readonly source: string;
  readonly storePath: string | null;
  readonly id: string;
  readonly deleted: boolean;
  readonly rows: readonly MobpackDraftSummary[];
  /**
   * Set when Meta wrote the draft store but could not sync its directory:
   * the write took effect and may not survive a crash.
   */
  readonly durabilityWarning: string | null;
}

export function parseMobpackDraftDeleteResult(
  raw: unknown,
): MobpackDraftDeleteResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    storePath: d.store_path == null ? null : String(d.store_path),
    id: String(d.id ?? ""),
    deleted: Boolean(d.deleted),
    rows: asRecordArray(d.rows).map(parseMobpackDraftSummary),
    durabilityWarning: d.durability_warning == null ? null : String(d.durability_warning),
  };
}

export interface MobpackApplyOperationResult {
  readonly source: string;
  readonly operation: string;
  readonly ok: boolean;
  readonly document: Record<string, unknown>;
  readonly selection: Record<string, unknown> | null;
  readonly validation: MobpackValidationResult;
}

export function parseMobpackApplyOperationResult(
  raw: unknown,
): MobpackApplyOperationResult {
  const d = asRecord(raw);
  return {
    source: String(d.source ?? ""),
    operation: String(d.operation ?? ""),
    ok: Boolean(d.ok),
    document: asRecord(d.document),
    selection: d.selection == null ? null : asRecord(d.selection),
    validation: parseMobpackValidationResult(d.validation),
  };
}

export interface MobpackDeployCommandResult {
  readonly command: string;
  readonly argv: readonly string[];
  readonly deployCommand: string;
  readonly filename: string;
  readonly validation: MobpackValidationResult;
  readonly source: string;
}

export function parseMobpackDeployCommandResult(
  raw: unknown,
): MobpackDeployCommandResult {
  const d = asRecord(raw);
  return {
    command: String(d.command ?? ""),
    argv: asStringArray(d.argv),
    deployCommand: String(d.deploy_command ?? ""),
    filename: String(d.filename ?? ""),
    validation: parseMobpackValidationResult(d.validation),
    source: String(d.source ?? ""),
  };
}

export interface MobpackDeployResult {
  readonly filename: string;
  readonly packPath: string;
  readonly packSha256: string;
  readonly command: string;
  readonly argv: readonly string[];
  readonly planTrace: readonly Record<string, unknown>[];
  readonly executed: boolean;
  readonly success: boolean;
  readonly statusCode: number | null;
  readonly stdout: string | null;
  readonly stderr: string | null;
  readonly validation: MobpackValidationResult;
  readonly displayRows: readonly MobpackDisplayRow[];
}

export function parseMobpackDeployResult(raw: unknown): MobpackDeployResult {
  const d = asRecord(raw);
  return {
    filename: String(d.filename ?? ""),
    packPath: String(d.pack_path ?? ""),
    packSha256: String(d.pack_sha256 ?? ""),
    command: String(d.command ?? ""),
    argv: asStringArray(d.argv),
    planTrace: asRecordArray(d.plan_trace),
    executed: Boolean(d.executed),
    success: Boolean(d.success),
    statusCode: d.status_code == null ? null : Number(d.status_code),
    stdout: d.stdout == null ? null : String(d.stdout),
    stderr: d.stderr == null ? null : String(d.stderr),
    validation: parseMobpackValidationResult(d.validation),
    displayRows: asRecordArray(d.display_rows).map(parseMobpackDisplayRow),
  };
}

// -- ErrorCategory / ErrorEvent -------------------------------------------

// Every Rust `ErrorEvent` variant must appear here with its serde
// `snake_case` wire tag. The Rust test
// `crates/meerkat-meta/tests/sdk_error_category_parity.rs` fails the build if this
// set drifts from the Rust enum.
export const ErrorCategory = {
  SPAWN_FAILURE: "spawn_failure",
  RECONCILE_INCOMPLETE: "reconcile_incomplete",
  CHECKPOINT_FAILURE: "checkpoint_failure",
  COMPACTION_PERSISTENCE_REJECTED: "compaction_persistence_rejected",
  ACTOR_LOOP_STALLED: "actor_loop_stalled",
  ACTOR_LOOP_RECOVERED: "actor_loop_recovered",
  ACTOR_LOOP_TERMINATED: "actor_loop_terminated",
  HOST_LOOP_CRASH: "host_loop_crash",
  REDISCOVER_FAILURE: "rediscover_failure",
  EVENT_LOG_FLUSH_FAILURE: "event_log_flush_failure",
  IDENTITY_MATERIALIZATION_FAILURE: "identity_materialization_failure",
  MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT: "mob_stop_proceeded_without_interrupt",
  RETIRED_MEMBER_RELEASE_FAILED: "retired_member_release_failed",
  MOB_ANNOUNCEMENT_ESCALATED: "mob_announcement_escalated",
  MOB_ANNOUNCEMENT_ESCALATION_RESOLVED: "mob_announcement_escalation_resolved",
} as const;

/** Categories that report a failure ENDING rather than starting. Mirrors the
 *  Rust runtime's `log_error_event`, which logs these at INFO instead of
 *  ERROR. A receiver that pages on every `onError` call would otherwise raise
 *  a second incident for a stall that already resolved. */
export const RESOLUTION_ERROR_CATEGORIES: ReadonlySet<string> = new Set([
  ErrorCategory.ACTOR_LOOP_RECOVERED,
  ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED,
]);

/** A mob announcement escalation's subject as the runtime words it. */
function escalationSubject(raw: unknown): string {
  if (typeof raw !== "object" || raw === null) return String(raw ?? "");
  const subject = raw as Record<string, unknown>;
  switch (subject.kind) {
    case "source":
      return `source ${String(subject.source_id ?? "")} of mob ${String(subject.mob_id ?? "")}`;
    case "member": {
      const member = asRecord(subject.member);
      return `member ${String(member.identity ?? "")} of mob ${String(member.mob_id ?? "")}`;
    }
    case "declaration":
      return `declared ${String(subject.record_kind ?? "")} ${String(subject.id ?? "")}`;
    default:
      return JSON.stringify(subject);
  }
}

/** True when the event reports a failure ENDING rather than starting, so a
 *  host can close an incident instead of opening a second one. */
export function isResolutionErrorEvent(event: ErrorEvent): boolean {
  return RESOLUTION_ERROR_CATEGORIES.has(event.category);
}

export type ErrorCategoryValue =
  (typeof ErrorCategory)[keyof typeof ErrorCategory];

export interface ErrorEvent {
  readonly category: string;
  readonly message: string;
  readonly context: Record<string, unknown>;
}

export function parseErrorEvent(raw: unknown): ErrorEvent {
  const d = asRecord(raw);
  const category = String(d.category ?? "unknown");
  const context: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(d)) {
    if (k !== "category") context[k] = v;
  }

  const error = String(context.error ?? "");
  const memberId = String(context.member_id ?? "");
  let message: string;

  switch (category) {
    case ErrorCategory.SPAWN_FAILURE:
      message = memberId ? `${memberId}: ${error}` : error;
      break;
    case ErrorCategory.RECONCILE_INCOMPLETE: {
      const failures = Number(context.failures ?? 0);
      const skipped = Number(context.skipped ?? 0);
      message = `${failures} failures, ${skipped} skipped`;
      break;
    }
    case ErrorCategory.CHECKPOINT_FAILURE: {
      const sessionId = String(context.session_id ?? "");
      message = sessionId ? `${sessionId}: ${error}` : error;
      break;
    }
    case ErrorCategory.COMPACTION_PERSISTENCE_REJECTED: {
      const identity = String(context.identity ?? "");
      const sessionId = String(context.session_id ?? "");
      message = `${identity} (${sessionId}): ${error}`;
      break;
    }
    case ErrorCategory.ACTOR_LOOP_STALLED: {
      const waited = Number(context.probe_waited_secs ?? 0);
      const detail = String(context.detail ?? "");
      message = `probe unanswered for ${waited}s: ${detail}`;
      break;
    }
    case ErrorCategory.ACTOR_LOOP_RECOVERED: {
      const stallId = Number(context.stall_id ?? 0);
      const stalledFor = Number(context.stalled_for_secs ?? 0);
      message = `stall ${stallId} resolved after ${stalledFor}s`;
      break;
    }
    case ErrorCategory.ACTOR_LOOP_TERMINATED: {
      const detail = String(context.detail ?? "");
      const observed =
        context.stall_id === undefined || context.stall_id === null
          ? ""
          : ` (observed on stall ${Number(context.stall_id)})`;
      message =
        `the mob actor loop is gone${observed}; this is NOT a recovery, ` +
        `the process must restart: ${detail}`;
      break;
    }
    case ErrorCategory.HOST_LOOP_CRASH:
      message = memberId ? `${memberId}: ${error}` : error;
      break;
    case ErrorCategory.REDISCOVER_FAILURE:
      message = error;
      break;
    case ErrorCategory.EVENT_LOG_FLUSH_FAILURE:
      message = error;
      break;
    case ErrorCategory.IDENTITY_MATERIALIZATION_FAILURE: {
      const identity = String(context.identity ?? "");
      const initiator = String(context.initiator ?? "");
      const operation = String(context.operation ?? "");
      const target = initiator ? `${identity} for ${initiator}` : identity;
      message = [target, operation, error].filter(Boolean).join(": ");
      break;
    }
    case ErrorCategory.MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT: {
      const waitedMs = Number(context.waited_ms ?? 0);
      const member = String(context.member ?? "");
      const subject = member ? ` on ${member}` : "";
      message =
        `teardown proceeded after ${waitedMs}ms WITHOUT a successful ` +
        `interrupt${subject}; a turn may have been admitted and may ` +
        `still be running: ${error}`;
      break;
    }
    case ErrorCategory.RETIRED_MEMBER_RELEASE_FAILED: {
      const identity = String(context.identity ?? "");
      const runtimeId = String(context.agent_runtime_id ?? "");
      const cause = String(context.cause ?? "");
      message =
        `${identity} (${runtimeId}) was retired (${cause}), and the host's ` +
        `release failed: ${error}`;
      break;
    }
    case ErrorCategory.MOB_ANNOUNCEMENT_ESCALATED: {
      const escalation = asRecord(context.escalation);
      message =
        `${String(context.event ?? "")} ${String(escalation.escalation_id ?? "")} ` +
        `${String(escalation.cause ?? "")} on ${escalationSubject(escalation.subject)} ` +
        `(notified again ${Number(context.renotify_count ?? 0)} times, ` +
        `cursor ${Number(context.cursor ?? 0)})`;
      break;
    }
    case ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED:
      message =
        `${String(context.escalation_id ?? "")} ${String(context.cause ?? "")} on ` +
        `${escalationSubject(context.subject)} (cursor ${Number(context.cursor ?? 0)})`;
      break;
    default:
      message = JSON.stringify(d);
      break;
  }

  return { category, message, context };
}

// =========================================================================
// Identity-First Types
// =========================================================================

// -- DurableAgentSpec (REQ-46) --------------------------------------------

export interface DurableAgentSpec {
  readonly identity: string;
  readonly profile: string;
  readonly addressability: string;
  readonly displayName: string | null;
  readonly labels: Readonly<Record<string, string>>;
  readonly context: unknown | null;
  readonly additionalInstructions: readonly string[];
  /**
   * Pins meerkat's runtime mode for this identity. `null`/omitted defers to
   * the profile's `runtime_mode` in mob.toml (meerkat default:
   * `autonomous_host`).
   */
  readonly runtimeModeOverride?: "autonomous_host" | "turn_driven" | null;
  /**
   * First turn a fresh `autonomous_host` member runs on (Rust `ContentInput`:
   * plain text or content blocks). `null`/omitted leaves meerkat's fallback
   * spawn prompt in place.
   */
  readonly initialMessage?: string | readonly DispatchContentBlock[] | null;
  readonly backend?: "session" | "external" | null;
  readonly binding?: Readonly<Record<string, unknown>> | null;
  /** Exact canonical Meerkat host ref. Never interpreted as a local fallback. */
  readonly placement?: string | null;
}

export function parseDurableAgentSpec(raw: unknown): DurableAgentSpec {
  const d = asRecord(raw);
  return {
    identity: validateAgentIdentity(String(d.identity ?? "")),
    profile: String(d.profile ?? ""),
    addressability: String(d.addressability ?? "addressable"),
    displayName: typeof d.display_name === "string" ? d.display_name : null,
    labels: asStringRecord(d.labels),
    context: d.context !== undefined && d.context !== null ? d.context : null,
    additionalInstructions: asStringArray(d.additional_instructions),
    runtimeModeOverride:
      d.runtime_mode_override === "autonomous_host" || d.runtime_mode_override === "turn_driven"
        ? d.runtime_mode_override
        : null,
    initialMessage: parseContentInput(d.initial_message),
    backend:
      d.backend === "session" || d.backend === "external" ? d.backend : null,
    binding:
      d.binding != null && typeof d.binding === "object"
        ? { ...(d.binding as Record<string, unknown>) }
        : null,
    placement: typeof d.placement === "string" ? d.placement : null,
  };
}

export function durableAgentSpecToDict(
  spec: DurableAgentSpec,
): Record<string, unknown> {
  const result: Record<string, unknown> = {
    identity: spec.identity,
    profile: spec.profile,
    addressability: spec.addressability,
  };
  if (spec.displayName !== null) result.display_name = spec.displayName;
  if (Object.keys(spec.labels).length > 0) result.labels = { ...spec.labels };
  if (spec.context !== null) result.context = spec.context;
  if (spec.additionalInstructions.length > 0) {
    result.additional_instructions = [...spec.additionalInstructions];
  }
  if (spec.runtimeModeOverride) {
    result.runtime_mode_override = spec.runtimeModeOverride;
  }
  if (spec.initialMessage !== undefined && spec.initialMessage !== null) {
    result.initial_message =
      typeof spec.initialMessage === "string"
        ? spec.initialMessage
        : spec.initialMessage.map(contentBlockToDict);
  }
  if (spec.backend) result.backend = spec.backend;
  if (spec.binding) result.binding = { ...spec.binding };
  // Presence, not truthiness, owns placement. Even an invalid empty ref must
  // reach Rust and fail typed rather than disappear into local `None`.
  if (spec.placement !== undefined && spec.placement !== null) {
    result.placement = spec.placement;
  }
  return result;
}

// -- DispatchContentBlock + DispatchInput (REQ-49) ------------------------

export interface TextContentBlock {
  readonly type: "text";
  readonly text: string;
}

export interface InlineImageContentBlock {
  readonly type: "image";
  readonly mediaType: string;
  readonly source?: "inline";
  readonly data: string;
}

export interface BlobImageContentBlock {
  readonly type: "image";
  readonly mediaType: string;
  readonly source: "blob";
  readonly blobId: string;
}

export type ImageContentBlock = InlineImageContentBlock | BlobImageContentBlock;

export interface BlobGetResult {
  readonly blobId: string;
  readonly mediaType: string;
  readonly size: number;
  readonly data: string;
}

export function parseBlobGetResult(raw: unknown): BlobGetResult {
  const d = asRecord(raw);
  return {
    blobId: String(d.blob_id ?? d.blobId ?? ""),
    mediaType: String(d.media_type ?? d.mediaType ?? ""),
    size: Number(d.size ?? 0) || 0,
    data: String(d.data ?? ""),
  };
}

export interface BlobUploadResult {
  readonly blobId: string;
  readonly mediaType: string;
  readonly size: number;
}

export function parseBlobUploadResult(raw: unknown): BlobUploadResult {
  const d = asRecord(raw);
  return {
    blobId: String(d.blob_id ?? d.blobId ?? ""),
    mediaType: String(d.media_type ?? d.mediaType ?? ""),
    size: Number(d.size ?? 0) || 0,
  };
}

export type DispatchContentBlock = TextContentBlock | ImageContentBlock;

export type DispatchOrigin =
  | "connector"
  | "scheduler"
  | "policy"
  | "flow"
  | "system";

export interface DispatchInput {
  readonly content: string | DispatchContentBlock[];
  readonly origin: DispatchOrigin;
  readonly correlationId?: string;
  readonly idempotencyKey?: string;
}

function parseContentBlock(raw: unknown): DispatchContentBlock {
  const d = asRecord(raw);
  if (d.type === "image") {
    const mediaType = String(d.media_type ?? d.mediaType ?? "");
    if (d.source === "blob") {
      return {
        type: "image",
        mediaType,
        source: "blob",
        blobId: String(d.blob_id ?? d.blobId ?? ""),
      };
    }
    return {
      type: "image",
      mediaType,
      source: "inline",
      data: String(d.data ?? ""),
    };
  }
  return { type: "text", text: String(d.text ?? "") };
}

/**
 * Rust `ContentInput` is untagged: a plain string or a list of content blocks.
 * Anything else (absent, null, wrong shape) reads as `null`.
 */
function parseContentInput(raw: unknown): string | DispatchContentBlock[] | null {
  if (typeof raw === "string") return raw;
  if (Array.isArray(raw)) return raw.map(parseContentBlock);
  return null;
}

export function contentBlockToDict(
  block: DispatchContentBlock,
): Record<string, unknown> {
  if (block.type === "image") {
    if (block.source === "blob") {
      return {
        type: "image",
        media_type: block.mediaType,
        source: "blob",
        blob_id: block.blobId,
      };
    }
    return {
      type: "image",
      media_type: block.mediaType,
      source: "inline",
      data: block.data,
    };
  }
  return { type: "text", text: block.text };
}

const DISPATCH_ORIGIN_VALUES: readonly DispatchOrigin[] = [
  "connector",
  "scheduler",
  "policy",
  "flow",
  "system",
];

function parseDispatchOrigin(raw: unknown): DispatchOrigin {
  // Pre-fix this was `String(d.origin ?? "system") as DispatchOrigin`
  // — an unchecked cast that admitted arbitrary strings, so a
  // consumer's `switch (input.origin)` over the closed set hit no
  // branch silently. Validate against the union and fall back to
  // "system" for anything else.
  if (typeof raw === "string" && (DISPATCH_ORIGIN_VALUES as readonly string[]).includes(raw)) {
    return raw as DispatchOrigin;
  }
  return "system";
}

export function parseDispatchInput(raw: unknown): DispatchInput {
  const d = asRecord(raw);
  let content: string | DispatchContentBlock[];
  if (typeof d.content === "string") {
    content = d.content;
  } else if (Array.isArray(d.content)) {
    content = d.content.map(parseContentBlock);
  } else {
    content = "";
  }
  return {
    content,
    origin: parseDispatchOrigin(d.origin),
    correlationId: typeof d.correlation_id === "string" ? d.correlation_id : undefined,
    idempotencyKey: typeof d.idempotency_key === "string" ? d.idempotency_key : undefined,
  };
}

export function dispatchInputToDict(
  input: DispatchInput,
): Record<string, unknown> {
  const result: Record<string, unknown> = {
    origin: input.origin,
  };
  if (typeof input.content === "string") {
    result.content = input.content;
  } else {
    result.content = input.content.map(contentBlockToDict);
  }
  if (input.correlationId !== undefined) {
    result.correlation_id = input.correlationId;
  }
  if (input.idempotencyKey !== undefined) {
    result.idempotency_key = input.idempotencyKey;
  }
  return result;
}

// -- ManagedPeerEdge (REQ-49a) --------------------------------------------

export interface ManagedPeerEdge {
  readonly a: string;
  readonly b: string;
}

export function parseManagedPeerEdge(raw: unknown): ManagedPeerEdge {
  const d = asRecord(raw);
  const a = validateAgentIdentity(String(d.a ?? ""));
  const b = validateAgentIdentity(String(d.b ?? ""));
  if (a === b) {
    throw new Error(`managed peer edge cannot connect an identity to itself: ${a}`);
  }
  return a < b ? { a, b } : { a: b, b: a };
}

export function managedPeerEdgeToDict(edge: ManagedPeerEdge): Record<string, string> {
  const parsed = parseManagedPeerEdge(edge);
  return { a: parsed.a, b: parsed.b };
}

// -- ExternalToolDef (REQ-49a) --------------------------------------------

export interface ExternalToolDef {
  readonly name: string;
  readonly description: string;
  readonly inputSchema: Record<string, unknown>;
  /** Installed artifact this handler came from; verified by the gateway. */
  readonly artifact?: ToolArtifactRef;
}

export function parseExternalToolDef(raw: unknown): ExternalToolDef {
  const d = asRecord(raw);
  const tool: ExternalToolDef = {
    name: String(d.name ?? ""),
    description: String(d.description ?? ""),
    inputSchema: asRecord(d.input_schema),
  };
  return d.artifact !== undefined && d.artifact !== null
    ? { ...tool, artifact: parseToolArtifactRef(d.artifact) }
    : tool;
}

export function externalToolDefToDict(
  tool: ExternalToolDef,
): Record<string, unknown> {
  const out: Record<string, unknown> = {
    name: tool.name,
    description: tool.description,
    input_schema: tool.inputSchema,
  };
  if (tool.artifact !== undefined) out.artifact = toolArtifactRefToWire(tool.artifact);
  return out;
}

// -- AgentBuildContext (REQ-49a) -------------------------------------------

export interface AgentBuildContext {
  readonly identity: string;
  readonly activePeers: readonly string[];
  readonly managedEdges: readonly ManagedPeerEdge[];
}

export function parseAgentBuildContext(raw: unknown): AgentBuildContext {
  const d = asRecord(raw);
  const rawEdges = Array.isArray(d.managed_edges) ? d.managed_edges : [];
  return {
    identity: String(d.identity ?? ""),
    activePeers: asStringArray(d.active_peers),
    managedEdges: rawEdges.map(parseManagedPeerEdge),
  };
}

// -- RosterContext ----------------------------------------------------------

/**
 * Read-only context the gateway hands to `RosterProvider.roster`.
 *
 * Mirrors the Rust `RosterContext` one for one; it arrives on the wire as
 * `callback/roster_provider/roster` `params.context`.
 *
 * `mobDefinition` is the booted mob's compiled definition as a plain JSON
 * object (`[mob]` id, profiles, ...). It is `null` only when the runtime
 * resolving the roster has no mob definition, which the bundled `rpc_gateway`
 * never does; the field is nullable because in-process Rust embedders and test
 * harnesses can call the provider without one.
 *
 * `previousIdentities` is call-site-dependent prior-identity context, not an
 * authoritative membership snapshot on every callback. Topology queries and
 * edge reconciliation supply currently registered identities. Bootstrap, full
 * roster refresh/reconcile, and reset-time spec adoption currently pass an
 * empty list, even if identities are already registered.
 */
export interface RosterContext {
  readonly mobDefinition: Record<string, unknown> | null;
  readonly previousIdentities: readonly string[];
}

export function parseRosterContext(raw: unknown): RosterContext {
  const d = asRecord(raw);
  const definition = d.mob_definition;
  if (definition === undefined || definition === null) {
    return { mobDefinition: null, previousIdentities: asStringArray(d.previous_identities) };
  }
  if (typeof definition !== "object" || Array.isArray(definition)) {
    throw new TypeError("RosterContext.mob_definition must be an object or null");
  }
  return {
    mobDefinition: definition as Record<string, unknown>,
    previousIdentities: asStringArray(d.previous_identities),
  };
}

// -- AgentBuildDraft (REQ-49a) --------------------------------------------

/** Options for {@link AgentBuildDraft.registerTool}. */
export interface AgentBuildDraftToolOptions {
  description?: string;
  inputSchema?: Record<string, unknown>;
  /** Installed artifact this handler came from; verified by the gateway. */
  artifact?: ToolArtifactRef;
}

export interface AgentBuildDraft {
  model: string | null;
  systemPrompt: string | null;
  additionalInstructions: string[];
  labels: Record<string, string>;
  appContext: unknown | null;
  externalTools: ExternalToolDef[];
  /**
   * Per-identity provider parameter overrides, carried verbatim to the
   * gateway. Mirrors meerkat's `ProviderParamsOverride` (the type behind
   * `AgentBuildConfig.provider_params`): provider knobs with no Meta
   * vocabulary of their own, such as OpenAI `prompt_cache_key` /
   * `prompt_cache_options` / `prompt_cache_retention` and Anthropic
   * `cache_control`. Kept opaque here because the Rust boundary parses it
   * fail-closed — an unknown or mistyped knob is rejected there, not dropped.
   */
  providerParams: Record<string, unknown> | null;
  /**
   * Register a callable external tool on this build. Parity with
   * {@link SessionBuildOptions.registerTool}, but available inside
   * {@link AgentCustomizer.customizeBuild} — which runs on BOTH fresh create
   * and restore/reconcile — so resumed agents keep identity-scoped tools
   * (MCP, comms, etc.). The handler is dispatched in-process when the agent
   * invokes the tool.
   *
   * @example
   * ```ts
   * async customizeBuild(ctx, spec, draft) {
   *   draft.registerTool("send_to_im", async (args) => sendIm(args), "Send an IM");
   * }
   * ```
   */
  registerTool(
    name: string,
    handler: ToolHandler,
    description?: string,
    inputSchema?: Record<string, unknown>,
    artifact?: ToolArtifactRef,
  ): void;
  /** Options form; same semantics as the positional form. */
  registerTool(
    name: string,
    handler: ToolHandler,
    options: AgentBuildDraftToolOptions,
  ): void;
  /** Handlers registered via {@link registerTool} (in-process only). */
  readonly toolHandlers: ReadonlyMap<string, ToolHandler>;
}

export function parseAgentBuildDraft(raw: unknown): AgentBuildDraft {
  const d = asRecord(raw);
  const rawTools = Array.isArray(d.external_tools) ? d.external_tools : [];
  const externalTools = rawTools.map(parseExternalToolDef);
  const toolHandlers = new Map<string, ToolHandler>();
  const draft: AgentBuildDraft = {
    model: typeof d.model === "string" ? d.model : null,
    systemPrompt: typeof d.system_prompt === "string" ? d.system_prompt : null,
    additionalInstructions: asStringArray(d.additional_instructions),
    labels: asStringRecord(d.labels),
    appContext: d.app_context !== undefined && d.app_context !== null ? d.app_context : null,
    externalTools,
    providerParams:
      d.provider_params !== undefined && d.provider_params !== null
        ? asRecord(d.provider_params)
        : null,
    registerTool(
      name: string,
      handler: ToolHandler,
      descriptionOrOptions?: string | AgentBuildDraftToolOptions,
      positionalSchema?: Record<string, unknown>,
      positionalArtifact?: ToolArtifactRef,
    ): void {
      const options: AgentBuildDraftToolOptions =
        typeof descriptionOrOptions === "object" && descriptionOrOptions !== null
          ? descriptionOrOptions
          : {
              description: descriptionOrOptions,
              inputSchema: positionalSchema,
              artifact: positionalArtifact,
            };
      const description = options.description ?? "";
      const inputSchema = options.inputSchema ?? { type: "object" };
      const artifact = options.artifact;
      if (typeof name !== "string") {
        throw new TypeError(
          `tool name must be a string, got ${typeof name}: ${String(name)}`,
        );
      }
      if (typeof handler !== "function") {
        throw new TypeError(
          `handler must be callable, got ${typeof handler}: ${String(handler)}`,
        );
      }
      // Build and validate the replacement first: a refused registration
      // leaves the previous definition, artifact and handler untouched.
      const replacement: ExternalToolDef =
        artifact !== undefined
          ? { name, description, inputSchema, artifact: validateToolArtifactRef(artifact) }
          : { name, description, inputSchema };
      // Registering a name again replaces its definition wholesale: a
      // registration without an artifact never inherits the previous one's.
      const existing = externalTools.findIndex((tool) => tool.name === name);
      if (existing >= 0) externalTools.splice(existing, 1);
      externalTools.push(replacement);
      toolHandlers.set(name, handler);
    },
    get toolHandlers(): ReadonlyMap<string, ToolHandler> {
      return new Map(toolHandlers);
    },
  };
  return draft;
}

export function agentBuildDraftToDict(
  draft: AgentBuildDraft,
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (draft.model !== null) result.model = draft.model;
  if (draft.systemPrompt !== null) result.system_prompt = draft.systemPrompt;
  if (draft.additionalInstructions.length > 0) {
    result.additional_instructions = [...draft.additionalInstructions];
  }
  if (Object.keys(draft.labels).length > 0) result.labels = { ...draft.labels };
  if (draft.appContext !== null) result.app_context = draft.appContext;
  if (draft.externalTools.length > 0) {
    result.external_tools = draft.externalTools.map(externalToolDefToDict);
  }
  // The gateway replaces the draft wholesale with what it gets back, so a
  // field omitted here is a field cleared on every customized build.
  if (draft.providerParams !== null) result.provider_params = draft.providerParams;
  return result;
}

// -- CompletionCursor -----------------------------------------------------

/**
 * Comparable completion identity for one identity's stream of turns.
 *
 * Waiting for an agent's next answer must never compare output TEXT: two
 * consecutive turns can legitimately produce byte-identical output, and a text
 * comparison then reports "no new turn" for the whole configured wait. This
 * cursor is the comparable atom instead — never derived from output content, a
 * content hash, a timestamp, or a per-poll uuid.
 *
 * `epoch` is Core's fence for the identity's live member (the runtime
 * incarnation this count belongs to); `turns` counts turns observed as
 * completed within it.
 * Turn counts are not comparable across incarnations, so classify with
 * {@link completionProgressSince} rather than comparing `turns` directly.
 */
export interface CompletionCursor {
  readonly epoch: number;
  readonly turns: number;
}

/** How an observed cursor relates to a baseline captured at dispatch time. */
export type CompletionProgress =
  | "pending"
  | "completed"
  | "incarnation_changed";

export function parseCompletionCursor(raw: unknown): CompletionCursor {
  const d = asRecord(raw);
  return {
    epoch: Number(d.epoch ?? 0),
    turns: Number(d.turns ?? 0),
  };
}

/** Optional cursor field: absent (older gateway) stays `null`. */
export function parseOptionalCompletionCursor(
  raw: unknown,
): CompletionCursor | null {
  return typeof raw === "object" && raw !== null
    ? parseCompletionCursor(raw)
    : null;
}

export function completionCursorToDict(
  cursor: CompletionCursor,
): Record<string, number> {
  return { epoch: cursor.epoch, turns: cursor.turns };
}

export function completionProgressSince(
  cursor: CompletionCursor,
  baseline: CompletionCursor,
): CompletionProgress {
  if (cursor.epoch !== baseline.epoch) return "incarnation_changed";
  if (cursor.turns > baseline.turns) return "completed";
  return "pending";
}

// -- Identity delivery + inspection results -------------------------------

/** The `turn.ticket` a tracked delivery returned, if any. */
function parseTurnTicket(d: Record<string, unknown>): string | null {
  const turn = d.turn;
  if (typeof turn !== "object" || turn === null) return null;
  const ticket = (turn as Record<string, unknown>).ticket;
  return typeof ticket === "string" ? ticket : null;
}

/**
 * Why a delivery that asked for `trackTurn` could not be tracked, and whether
 * it was delivered at all.
 *
 * `code` is stable. Every code but `not_delivered` means the delivery
 * happened, exactly once, untracked: `autonomous_host` (the member runs in
 * `autonomous_host` mode, read from the runtime's live roster entry; its
 * inbox delivery reports no per-turn completion), `runtime_refused` (the
 * runtime refused per-turn completion for the member's live mode for another
 * reason), `externally_bound`, `host_human_input`,
 * `bridge_cannot_report_output` or `session_rotated` (a re-dispatch that
 * landed on a new session after a repair and ran as its own turn). `not_delivered` means NOTHING was delivered
 * (no session bridge or no bound runtime), so there is no turn to wait for.
 * `delivered` says which; `reason` is the human-readable detail.
 */
export interface TurnUnavailable {
  readonly code: string;
  readonly reason: string;
  readonly delivered: boolean;
}

function parseTurnUnavailable(raw: unknown): TurnUnavailable | null {
  if (typeof raw !== "object" || raw === null) return null;
  const d = raw as Record<string, unknown>;
  const code = String(d.code ?? "");
  return {
    code,
    reason: String(d.reason ?? ""),
    delivered:
      typeof d.delivered === "boolean" ? d.delivered : code !== "not_delivered",
  };
}

function turnTrackingToDict(
  out: Record<string, unknown>,
  turnTicket: string | null | undefined,
  turnUnavailable: TurnUnavailable | null | undefined,
): void {
  if (turnTicket != null) {
    out.turn = { ticket: turnTicket };
  } else if (turnUnavailable != null) {
    out.turn = null;
    out.turn_unavailable = {
      code: turnUnavailable.code,
      reason: turnUnavailable.reason,
      delivered: turnUnavailable.delivered,
    };
  }
}

/** Typed result from identity-first `send()`. */
/** An identity's registered roster row (`meta/registered_spec`). */
export interface RegisteredSpec {
  readonly identity: string;
  /** The full registered row, wire-shaped, including `additional_instructions`. */
  readonly spec: Record<string, unknown>;
  /** Canonical digest of `spec`; pass it to `send` as `expectedSpecDigest`. */
  readonly specDigest: string;
}

export interface SendResult {
  readonly fencingToken: number;
  /**
   * Cursor read before delivery: an IDENTITY-WIDE barrier. Another
   * delivery's completion also passes it, so it does not name this turn; use
   * {@link SendResult.turnTicket} for that. `null` when the gateway predates
   * the field.
   */
  readonly completionBaseline: CompletionCursor | null;
  /**
   * The ticket naming THIS turn, when the send asked for `trackTurn` and the
   * gateway could track it. Wait on it with `waitForTurn`.
   */
  readonly turnTicket?: string | null;
  /** Why a requested ticket is unavailable (the turn was still delivered). */
  readonly turnUnavailable?: TurnUnavailable | null;
}

export function parseSendResult(raw: unknown): SendResult {
  const d = asRecord(raw);
  return {
    fencingToken: Number(d.fencing_token ?? 0),
    completionBaseline: parseOptionalCompletionCursor(d.completion_baseline),
    turnTicket: parseTurnTicket(d),
    turnUnavailable: parseTurnUnavailable(d.turn_unavailable),
  };
}

export function sendResultToDict(result: SendResult): Record<string, unknown> {
  const out: Record<string, unknown> = { fencing_token: result.fencingToken };
  if (result.completionBaseline !== null) {
    out.completion_baseline = completionCursorToDict(result.completionBaseline);
  }
  turnTrackingToDict(out, result.turnTicket, result.turnUnavailable);
  return out;
}

/**
 * Where a keyed send's delivery stands, read for each answer. `state` is
 * `pending` (committed, not admitted yet: replay the same key to read it
 * again; a `pending` that survives a gateway restart is not proof of loss, so
 * do not resend it under a new key), `admitted` (queued in the member's
 * session: `sessionId` and `inputId` name the input, which `sendStatus`
 * follows; not proof that a turn ran), `stranded` (final: never admitted,
 * because the member incarnation it was bound to was replaced; resend under a
 * new key) or `refused` (final, never admitted; `reason` says why). Tolerate
 * other values: the vocabulary may grow.
 */
export interface ContinuationStatus {
  readonly state: string;
  readonly sessionId?: string;
  readonly inputId?: string;
  readonly reason?: string;
}

export function parseContinuationStatus(raw: unknown): ContinuationStatus {
  const d = asRecord(raw);
  const out: {
    state: string;
    sessionId?: string;
    inputId?: string;
    reason?: string;
  } = { state: String(d.state ?? "") };
  if (typeof d.session_id === "string") out.sessionId = d.session_id;
  if (typeof d.input_id === "string") out.inputId = d.input_id;
  if (typeof d.reason === "string") out.reason = d.reason;
  return out;
}

export function continuationStatusToDict(
  status: ContinuationStatus,
): Record<string, unknown> {
  const out: Record<string, unknown> = { state: status.state };
  if (status.sessionId !== undefined) out.session_id = status.sessionId;
  if (status.inputId !== undefined) out.input_id = status.inputId;
  if (status.reason !== undefined) out.reason = status.reason;
  return out;
}

/**
 * Typed result of a keyed send (`send(identity, content, { idempotencyKey })`):
 * the member continuation the content was committed as. A replay of the key
 * answers the same `deliveryId` and `committedAtMs`; `status` is read for each
 * answer and may advance.
 */
export interface KeyedSendResult {
  readonly deliveryId: string;
  readonly committedAtMs: number;
  readonly status: ContinuationStatus;
}

export function parseKeyedSendResult(raw: unknown): KeyedSendResult {
  const continuation = asRecord(asRecord(raw).continuation);
  if (typeof continuation.delivery_id !== "string") {
    throw new TypeError("a keyed meta/send result carries no continuation receipt");
  }
  return {
    deliveryId: continuation.delivery_id,
    committedAtMs: Number(continuation.committed_at_ms ?? 0),
    status: parseContinuationStatus(continuation.status),
  };
}

export function keyedSendResultToDict(result: KeyedSendResult): Record<string, unknown> {
  return {
    continuation: {
      delivery_id: result.deliveryId,
      committed_at_ms: result.committedAtMs,
      status: continuationStatusToDict(result.status),
    },
  };
}

/**
 * The runtime's state of one input, as meerkat's `session/input_status`
 * reports it. `currentState` is `accepted`, `queued`, `staged`, `applied`,
 * `applied_pending_consumption`, `consumed`, `superseded`, `coalesced` or
 * `abandoned`; `terminalOutcome` is set once the input is terminal
 * (`completed`, `abandoned`, `superseded`, `coalesced`, `cancelled`).
 * Tolerate other values. `raw` is the whole wire object.
 */
export interface InputState {
  readonly inputId: string;
  readonly currentState: string;
  readonly terminalOutcome: string | null;
  readonly raw: Record<string, unknown>;
}

/** Typed result of `sendStatus`: `state` is `null` when the session holds no such input. */
export interface SendStatusResult {
  readonly state: InputState | null;
}

export function parseSendStatusResult(raw: unknown): SendStatusResult {
  const state = asRecord(raw).state;
  if (typeof state !== "object" || state === null) {
    return { state: null };
  }
  const d = state as Record<string, unknown>;
  return {
    state: {
      inputId: String(d.input_id ?? ""),
      currentState: String(d.current_state ?? ""),
      terminalOutcome: typeof d.terminal_outcome === "string" ? d.terminal_outcome : null,
      raw: { ...d },
    },
  };
}

/** Typed result from identity-first `dispatch()`. */
export interface DispatchResult {
  readonly fencingToken: number;
  readonly durable: boolean;
  /** See {@link SendResult.completionBaseline}. */
  readonly completionBaseline: CompletionCursor | null;
  /** See {@link SendResult.turnTicket}. */
  readonly turnTicket?: string | null;
  /** See {@link SendResult.turnUnavailable}. */
  readonly turnUnavailable?: TurnUnavailable | null;
}

export function parseDispatchResult(raw: unknown): DispatchResult {
  const d = asRecord(raw);
  return {
    fencingToken: Number(d.fencing_token ?? 0),
    durable: Boolean(d.durable ?? false),
    completionBaseline: parseOptionalCompletionCursor(d.completion_baseline),
    turnTicket: parseTurnTicket(d),
    turnUnavailable: parseTurnUnavailable(d.turn_unavailable),
  };
}

/** Serialized format version of {@link DeliveryScope} this SDK reads. */
export const DELIVERY_SCOPE_VERSION = 1 as const;

/**
 * One identity's exact delivery scope, from `status()`.
 *
 * Persist it ({@link deliveryScopeToDict}) before a scoped dispatch, and pass
 * the same scope to `dispatch(..., { expectedScope })` and
 * `recoverDelivery`. It pins the identity's live member as Core holds it
 * (runtime id, Core's member generation, and Core's fence token in
 * `leaseFencingToken`) and meerkat's native member scope (`member`, opaque
 * and versioned on its own). A selector and stale-binding guard: the gateway
 * re-validates every atom and refuses a moved scope with `StaleScopeError`.
 * A scope persisted by Meta 0.9.3 holds Meta's own atoms and is refused as
 * stale.
 */
export interface DeliveryScope {
  readonly version: number;
  readonly identity: string;
  readonly agentRuntimeId: string;
  readonly generation: number;
  readonly leaseFencingToken: number;
  readonly member: Readonly<Record<string, unknown>>;
  /** The member session the scope pins. */
  readonly sessionId: string | null;
}

function nonNegativeInteger(value: unknown, name: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw new TypeError(`delivery scope ${name} must be a non-negative integer`);
  }
  return value;
}

/**
 * Decode a persisted scope. Throws `ContractMismatchError` for a scope
 * version this SDK does not read and `TypeError` for a malformed scope;
 * neither is ever read as a guess.
 */
export function parseDeliveryScope(raw: unknown): DeliveryScope {
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) {
    throw new TypeError("delivery scope must be an object");
  }
  const d = raw as Record<string, unknown>;
  if (typeof d.version !== "number" || !Number.isInteger(d.version)) {
    throw new TypeError("delivery scope has no integer version");
  }
  if (d.version !== DELIVERY_SCOPE_VERSION) {
    throw new ContractMismatchError(
      `unsupported delivery scope version ${d.version} (this SDK reads version ${DELIVERY_SCOPE_VERSION})`,
    );
  }
  if (typeof d.identity !== "string" || d.identity === "") {
    throw new TypeError("delivery scope identity must be a non-empty string");
  }
  if (typeof d.agent_runtime_id !== "string" || d.agent_runtime_id === "") {
    throw new TypeError("delivery scope agent_runtime_id must be a non-empty string");
  }
  const member = d.member;
  if (member === null || typeof member !== "object" || Array.isArray(member)) {
    throw new TypeError("delivery scope member must be an object");
  }
  const memberCopy = JSON.parse(JSON.stringify(member)) as Record<string, unknown>;
  return {
    version: d.version,
    identity: d.identity,
    agentRuntimeId: d.agent_runtime_id,
    generation: nonNegativeInteger(d.generation, "generation"),
    leaseFencingToken: nonNegativeInteger(d.lease_fencing_token, "lease_fencing_token"),
    member: memberCopy,
    sessionId: typeof memberCopy.session_id === "string" ? memberCopy.session_id : null,
  };
}

/** The exact persisted form the gateway expects back. */
export function deliveryScopeToDict(scope: DeliveryScope): Record<string, unknown> {
  return {
    version: scope.version,
    identity: scope.identity,
    agent_runtime_id: scope.agentRuntimeId,
    generation: scope.generation,
    lease_fencing_token: scope.leaseFencingToken,
    member: JSON.parse(JSON.stringify(scope.member)) as Record<string, unknown>,
  };
}

/**
 * What a scoped dispatch's admission proved. `stage` is the admission stage
 * the receipt proves (`ingress_accepted` for this path: the member's work
 * lane accepted the delivery; not a durable runtime-input claim).
 * `sessionId` is the scope's session, validated inside meerkat's admission.
 */
export interface DeliveryReceipt {
  readonly workRef: string;
  readonly stage: string;
  readonly sessionId: string;
}

/** Result of `dispatch(..., { expectedScope })`. */
export interface ScopedDispatchResult {
  readonly receipt: DeliveryReceipt;
  readonly deliveryScope: DeliveryScope;
  readonly fencingToken: number;
}

export function parseScopedDispatchResult(raw: unknown): ScopedDispatchResult {
  const d = asRecord(raw);
  const receipt = asRecord(d.receipt);
  return {
    receipt: {
      workRef: String(receipt.work_ref ?? ""),
      stage: String(receipt.stage ?? ""),
      sessionId: String(receipt.session_id ?? ""),
    },
    deliveryScope: parseDeliveryScope(d.delivery_scope),
    fencingToken: Number(d.fencing_token ?? 0),
  };
}

/**
 * What `recoverDelivery` established, from the ORIGINAL session only.
 * `absent` is an authoritative point-in-time miss and `unresolved` an
 * unreachable original owner: neither is permission to retry.
 */
export type ScopedRecoveryState =
  | "absent"
  | "in_flight"
  | "completed"
  | "failed"
  | "terminal_without_run"
  | "broken"
  | "unresolved";

const SCOPED_RECOVERY_STATES: ReadonlySet<string> = new Set([
  "absent",
  "in_flight",
  "completed",
  "failed",
  "terminal_without_run",
  "broken",
  "unresolved",
]);

/** Result of `recoverDelivery`. Only `state` is always set. */
export interface ScopedRecovery {
  readonly state: ScopedRecoveryState;
  readonly deliveryScope: DeliveryScope | null;
  readonly inputId: string | null;
  /** `in_flight`: the runtime input phase. */
  readonly phase: unknown;
  /** `in_flight`: whether a committed store row backs it. */
  readonly durableWitness: boolean | null;
  /** `completed`: as for {@link TurnResult}. */
  readonly outputStatus: string | null;
  readonly output: string | null;
  readonly outputTruncated: boolean;
  /** `failed`. */
  readonly error: string | null;
  /** `terminal_without_run`. */
  readonly terminal: unknown;
  readonly lastRunId: string | null;
  /** `broken`. */
  readonly reason: string | null;
  /**
   * `unresolved`: `runtime_adapter_unavailable`, `original_session_unknown`,
   * `original_owner_unavailable`, `evidence_read_timed_out` or
   * `scope_binding_unproven`.
   */
  readonly cause: string | null;
  readonly detail: string | null;
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

export function parseScopedRecovery(raw: unknown): ScopedRecovery {
  const d = asRecord(raw);
  const r = asRecord(d.recovery ?? d);
  const rawState = String(r.state ?? "unresolved");
  // A state this SDK does not know is unknown, never absence.
  const known = SCOPED_RECOVERY_STATES.has(rawState);
  const state = (known ? rawState : "unresolved") as ScopedRecoveryState;
  return {
    state,
    deliveryScope:
      d.delivery_scope !== null && typeof d.delivery_scope === "object"
        ? parseDeliveryScope(d.delivery_scope)
        : null,
    inputId: optionalString(r.input_id),
    phase: r.phase ?? null,
    durableWitness: typeof r.durable_witness === "boolean" ? r.durable_witness : null,
    outputStatus: optionalString(r.output_status),
    output: optionalString(r.output),
    outputTruncated: Boolean(r.output_truncated ?? false),
    error: optionalString(r.error),
    terminal: r.terminal ?? null,
    lastRunId: optionalString(r.last_run_id),
    reason: optionalString(r.reason),
    cause: known ? optionalString(r.cause) : `unknown_state:${rawState}`,
    detail: optionalString(r.detail),
  };
}

export function dispatchResultToDict(
  result: DispatchResult,
): Record<string, unknown> {
  const out: Record<string, unknown> = {
    fencing_token: result.fencingToken,
    durable: result.durable,
  };
  if (result.completionBaseline !== null) {
    out.completion_baseline = completionCursorToDict(result.completionBaseline);
  }
  turnTrackingToDict(out, result.turnTicket, result.turnUnavailable);
  return out;
}

/** Where a ticketed turn stands (`meta/turn_result`). */
export type TurnState = "pending" | "completed" | "failed" | "unknown";

/**
 * What a completed ticketed turn yielded (`output_status`):
 * - `text`: the turn's own final text is in `output` (see `outputTruncated`);
 * - `empty`: the turn ran and committed a run result with no text;
 * - `no_own_result`: the admission completed without a run result of its
 *   own (the runtime folded it into a run already in progress, or
 *   deduplicated it onto an earlier admission whose turn had already ended);
 * - `unavailable`: the gateway cannot report per-turn output (also used for
 *   a status this SDK does not know).
 */
export type TurnOutputStatus = "text" | "empty" | "no_own_result" | "unavailable";

const TURN_OUTPUT_STATUSES: readonly TurnOutputStatus[] = [
  "text",
  "empty",
  "no_own_result",
  "unavailable",
];

/**
 * `meta/turn_result`: one ticketed turn's state and its OWN output, from
 * that turn's committed run result (never the session's latest text).
 */
/**
 * What an `*AndWaitOutcome` call observed after its delivery.
 *
 * `attributed` says whether `text` provably belongs to THIS delivery's turn.
 * `true`: the turn was tracked by `ticket` and `text` is its own committed
 * output (`outputStatus` says how; `null` text means it committed none).
 * `false`: the delivery could not be tracked (`untrackedCode` is the
 * gateway's `turnUnavailable.code`, `null` for a gateway predating turn
 * tickets), so the call waited on the identity-wide completion cursor and
 * `text` is the session's latest output, which another delivery's turn (a
 * peer message, a scheduled turn) may have produced. `admission` is always
 * the send/dispatch result.
 */
export interface AwaitedTurn {
  text: string | null;
  attributed: boolean;
  admission: SendResult | DispatchResult;
  ticket: string | null;
  outputStatus: TurnOutputStatus | null;
  untrackedCode: string | null;
}

export interface TurnResult {
  readonly identity: string;
  readonly ticket: string;
  readonly state: TurnState;
  /** What a completed turn yielded; `null` in any other state. */
  readonly outputStatus: TurnOutputStatus | null;
  /** The turn's own final text; set only when `outputStatus` is `text`. */
  readonly output: string | null;
  /**
   * The text was cut at the gateway's bound (the runtime also appends a
   * truncation marker to it).
   */
  readonly outputTruncated: boolean;
  /** The failure detail when `state` is `failed`. */
  readonly error: string | null;
  /** The identity-wide cursor at read time (diagnostic only). */
  readonly completionCursor: CompletionCursor | null;
}

const TURN_STATES: readonly TurnState[] = [
  "pending",
  "completed",
  "failed",
  "unknown",
];

export function parseTurnResult(raw: unknown): TurnResult {
  const d = asRecord(raw);
  const state = TURN_STATES.find((known) => known === d.state) ?? "unknown";
  const outputStatus =
    state === "completed"
      ? (TURN_OUTPUT_STATUSES.find((known) => known === d.output_status) ??
        "unavailable")
      : null;
  return {
    identity: String(d.identity ?? ""),
    ticket: String(d.ticket ?? ""),
    state,
    outputStatus,
    output:
      outputStatus === "text" && typeof d.output === "string" ? d.output : null,
    outputTruncated: d.output_truncated === true,
    error: typeof d.error === "string" ? d.error : null,
    completionCursor: parseOptionalCompletionCursor(d.completion_cursor),
  };
}

/** Execution-level inspection of an identity (`meta/inspect_identity`). */
export interface IdentityInspection {
  readonly identity: string;
  readonly outputPreview: string | null;
  /**
   * Why `outputPreview` is not an observation of the member's session
   * (`observation_deadline`, `read_failed`, `session_absent`,
   * `not_observed_while_retiring`; unknown future values pass through). When
   * set, a missing preview is not an empty member. `null` when the preview
   * was observed, or the gateway predates the field.
   */
  readonly previewUnavailable: string | null;
  readonly isFinal: boolean;
  readonly peerReachableCount: number;
  /**
   * Live completion cursor. `null` for non-identity-first live aliases (no
   * identity authority tracks their turns) and for gateways predating the
   * field — in both cases absence must not be read as "no turns yet".
   */
  readonly completionCursor: CompletionCursor | null;
  /** As on `IdentityStatus`. */
  readonly agentRuntimeId: string | null;
  /** As on `IdentityStatus`. */
  readonly sessionId: string | null;
  /** As on `IdentityStatus`. */
  readonly memberId?: string | null;
  /** As on `IdentityStatus`. */
  readonly intentManaged: boolean;
  /** As on `IdentityStatus`. */
  readonly convergence: IdentityConvergence | null;
  /** As on `IdentityStatus`. */
  readonly wakeRefusal: WakeRefusal | null;
  /** As on `IdentityStatus`. */
  readonly replacedSession: ReplacedSession | null;
}

export function parseIdentityInspection(raw: unknown): IdentityInspection {
  const d = asRecord(raw);
  return {
    identity: String(d.identity ?? ""),
    outputPreview:
      typeof d.output_preview === "string" ? d.output_preview : null,
    previewUnavailable:
      typeof d.preview_unavailable === "string" ? d.preview_unavailable : null,
    isFinal: Boolean(d.is_final ?? false),
    peerReachableCount: Number(d.peer_reachable_count ?? 0),
    completionCursor: parseOptionalCompletionCursor(d.completion_cursor),
    agentRuntimeId: optionalString(d.agent_runtime_id),
    sessionId: optionalString(d.session_id),
    memberId: optionalString(d.member_id),
    ...parseIdentityViewParts(d),
  };
}

export function identityInspectionToDict(
  inspection: IdentityInspection,
): Record<string, unknown> {
  const out: Record<string, unknown> = {
    identity: inspection.identity,
    is_final: inspection.isFinal,
    peer_reachable_count: inspection.peerReachableCount,
  };
  if (inspection.outputPreview !== null) {
    out.output_preview = inspection.outputPreview;
  }
  if (inspection.previewUnavailable != null) {
    out.preview_unavailable = inspection.previewUnavailable;
  }
  if (inspection.completionCursor !== null) {
    out.completion_cursor = completionCursorToDict(inspection.completionCursor);
  }
  if (inspection.agentRuntimeId !== null) {
    out.agent_runtime_id = inspection.agentRuntimeId;
  }
  if (inspection.sessionId !== null) {
    out.session_id = inspection.sessionId;
  }
  if (inspection.memberId != null) {
    out.member_id = inspection.memberId;
  }
  out.intent_managed = inspection.intentManaged;
  if (inspection.convergence !== null) {
    out.convergence = identityConvergenceToDict(inspection.convergence);
  }
  if (inspection.wakeRefusal !== null) {
    out.wake_refusal = {
      code: inspection.wakeRefusal.code,
      session_id: inspection.wakeRefusal.sessionId,
      remedy: inspection.wakeRefusal.remedy,
    };
  }
  if (inspection.replacedSession !== null) {
    out.replaced_session = {
      session_id: inspection.replacedSession.sessionId,
      reason: inspection.replacedSession.reason,
    };
  }
  return out;
}

// -- IdentityStatus (REQ-49b) ---------------------------------------------

/**
 * An identity's view state: Core holds a live member for it (`active`), holds
 * none (`dormant`), or is `broken`: it holds none and the last wake was
 * refused for good (see `IdentityStatus.wakeRefusal`), or it holds the
 * identity's live member Broken because it could not restore it
 * (`agentRuntimeId` is then set and there is no `wakeRefusal`). A newer
 * gateway may send a state not listed here.
 */
export type IdentityViewState = "active" | "dormant" | "broken";

/**
 * Why the identity's last wake was refused; cleared by the next successful
 * wake. `code` is `-32051` (`IDENTITY_UNRESUMABLE_CODE`) for a final refusal,
 * when the identity reads `broken`, or `-32603` for a refusal held until an
 * operator acts, when it reads `dormant`. `sessionId` is the session whose
 * resume was refused. `remedy` names the way out; a newer value is kept as
 * sent.
 */
export interface WakeRefusal {
  readonly code: number;
  readonly sessionId: string;
  readonly remedy:
    | "delete_identity"
    | "repair_session"
    | "resolve_role_migration"
    | "operator_review"
    | (string & {});
}

/**
 * The session the last wake replaced with a fresh spawn: Core held no durable
 * session for the identity's last session (`reason` `"absent"`; a newer
 * reason is kept as sent). Present until the next wake.
 */
export interface ReplacedSession {
  readonly sessionId: string;
  readonly reason: "absent" | (string & {});
}

/**
 * Core's convergence status for an identity a host intent declaration
 * manages. The fields are Core's, unchanged: `condition` and `decision` are
 * Core's snake_case values (for example `"converged"` or `"backoff"`) and are
 * kept as sent.
 */
export interface IdentityConvergence {
  readonly agentIdentity: string;
  readonly desiredIntentRevision: number | null;
  readonly activeIntentRevision: number | null;
  readonly decision: string | null;
  readonly condition: string;
  readonly observedAtMs: number;
  readonly detail: string | null;
}

/**
 * Full identity status. Core's mob machine holds the identity's session and
 * generation; the status reads them per call. `agentRuntimeId` is `null` when
 * the identity is not live; `sessionId` and `generation` are `null` when Core
 * holds no binding for it.
 */
export interface IdentityStatus {
  readonly identity: string;
  readonly lifecycleState: IdentityViewState | (string & {});
  readonly agentRuntimeId: string | null;
  readonly sessionId: string | null;
  readonly profile: string;
  /** `"autonomous_host"` or `"turn_driven"`; `null` when not reported. */
  readonly runtimeMode: string | null;
  readonly addressability: string;
  readonly displayName: string | null;
  readonly labels: Readonly<Record<string, string>>;
  readonly generation: number | null;
  /**
   * `{ reason, artifact_refusal? }` while a restored member's customizer
   * tools are not published yet; `null` otherwise.
   */
  readonly customizerToolsPending: Readonly<Record<string, unknown>> | null;
  /**
   * `true` when a host intent declaration in Core manages the identity;
   * Meta's lifecycle verbs then refuse it with `IdentityHostAdoptedError`.
   */
  readonly intentManaged: boolean;
  /** Core's convergence status, when `intentManaged` is set and Core could read it. */
  readonly convergence: IdentityConvergence | null;
  /** The last refused wake, until a wake succeeds. */
  readonly wakeRefusal: WakeRefusal | null;
  /** The session the last wake replaced with a fresh spawn, until the next wake. */
  readonly replacedSession: ReplacedSession | null;
  /**
   * The exact delivery scope to persist before a scoped dispatch
   * (`dispatch(..., { expectedScope })`); `null` when none can be captured,
   * with the reason in `deliveryScopeUnavailable`.
   */
  readonly deliveryScope?: DeliveryScope | null;
  /** `{ kind, reason }` when `deliveryScope` is `null`. */
  readonly deliveryScopeUnavailable?: DeliveryScopeUnavailable | null;
  /**
   * Core's mob member id of the identity's last binding: the roster member
   * id, Meta's comms-safe encoding of the identity (the identity itself when
   * it is already a valid comms name, `mk--...` otherwise, as for an identity
   * with a `:`), from the same binding as `sessionId`; `null` when Core holds
   * no binding for the identity.
   */
  readonly memberId?: string | null;
}

export interface DeliveryScopeUnavailable {
  readonly kind: string;
  readonly reason: string;
}

export function parseIdentityStatus(raw: unknown): IdentityStatus {
  const d = asRecord(raw);
  let deliveryScope: DeliveryScope | null = null;
  const unavailableRaw = d.delivery_scope_unavailable;
  let deliveryScopeUnavailable: DeliveryScopeUnavailable | null =
    unavailableRaw !== null && typeof unavailableRaw === "object"
      ? {
          kind: String(asRecord(unavailableRaw).kind ?? ""),
          reason: String(asRecord(unavailableRaw).reason ?? ""),
        }
      : null;
  if (d.delivery_scope !== null && typeof d.delivery_scope === "object") {
    try {
      deliveryScope = parseDeliveryScope(d.delivery_scope);
    } catch (error) {
      if (!(error instanceof ContractMismatchError)) throw error;
      // A scope this SDK cannot read is never guessed at; the rest of the
      // status stays usable.
      deliveryScopeUnavailable = {
        kind: "unsupported_delivery_scope_version",
        reason: error.message,
      };
    }
  }
  return {
    deliveryScope,
    deliveryScopeUnavailable,
    identity: String(d.identity ?? ""),
    lifecycleState: String(d.state ?? ""),
    agentRuntimeId: typeof d.agent_runtime_id === "string" ? d.agent_runtime_id : null,
    sessionId: typeof d.session_id === "string" ? d.session_id : null,
    memberId: typeof d.member_id === "string" ? d.member_id : null,
    profile: String(d.profile ?? ""),
    runtimeMode: optionalString(d.runtime_mode),
    addressability: String(d.addressability ?? "addressable"),
    displayName: typeof d.display_name === "string" ? d.display_name : null,
    labels: asStringRecord(d.labels),
    generation: typeof d.generation === "number" ? d.generation : null,
    customizerToolsPending:
      d.customizer_tools_pending !== null && typeof d.customizer_tools_pending === "object"
        ? { ...asRecord(d.customizer_tools_pending) }
        : null,
    ...parseIdentityViewParts(d),
  };
}

/**
 * The identity-view parts `IdentityStatus` and `IdentityInspection` share:
 * `intent_managed` (always sent) and the optional `convergence`,
 * `wake_refusal` and `replaced_session` objects.
 */
function parseIdentityViewParts(d: Record<string, unknown>): {
  intentManaged: boolean;
  convergence: IdentityConvergence | null;
  wakeRefusal: WakeRefusal | null;
  replacedSession: ReplacedSession | null;
} {
  const part = <T>(value: unknown, parse: (raw: unknown) => T): T | null =>
    value !== null && typeof value === "object" ? parse(value) : null;
  return {
    intentManaged: d.intent_managed === true,
    convergence: part(d.convergence, parseIdentityConvergence),
    wakeRefusal: part(d.wake_refusal, parseWakeRefusal),
    replacedSession: part(d.replaced_session, parseReplacedSession),
  };
}

export function identityConvergenceToDict(
  convergence: IdentityConvergence,
): Record<string, unknown> {
  return {
    agent_identity: convergence.agentIdentity,
    desired_intent_revision: convergence.desiredIntentRevision,
    active_intent_revision: convergence.activeIntentRevision,
    decision: convergence.decision,
    condition: convergence.condition,
    observed_at_ms: convergence.observedAtMs,
    detail: convergence.detail,
  };
}

export function parseReplacedSession(raw: unknown): ReplacedSession {
  const d = asRecord(raw);
  return {
    sessionId: String(d.session_id ?? ""),
    reason: String(d.reason ?? ""),
  };
}

function optionalNumber(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

export function parseWakeRefusal(raw: unknown): WakeRefusal {
  const d = asRecord(raw);
  return {
    code: typeof d.code === "number" ? d.code : 0,
    sessionId: String(d.session_id ?? ""),
    remedy: String(d.remedy ?? ""),
  };
}

export function parseIdentityConvergence(raw: unknown): IdentityConvergence {
  const d = asRecord(raw);
  return {
    agentIdentity: String(d.agent_identity ?? ""),
    desiredIntentRevision: optionalNumber(d.desired_intent_revision),
    activeIntentRevision: optionalNumber(d.active_intent_revision),
    decision: optionalString(d.decision),
    condition: String(d.condition ?? ""),
    observedAtMs: typeof d.observed_at_ms === "number" ? d.observed_at_ms : 0,
    detail: optionalString(d.detail),
  };
}

// -- Provider interfaces (REQ-48) -----------------------------------------

/**
 * Deadline/cancellation context for a gateway-hosted provider callback.
 *
 * A provider that commits side effects must observe `signal` before
 * committing. A rejected or aborted callback is pre-commit.
 */
export interface ProviderCallbackContext {
  readonly signal: AbortSignal;
  /** Absolute Unix timestamp in milliseconds for the host callback deadline. */
  readonly deadlineMs: number;
}

export interface RosterProvider {
  /**
   * Return the desired `DurableAgentSpec` list. `context` carries the booted
   * mob definition and the identities the runtime has already registered.
   */
  roster(context: RosterContext): Promise<DurableAgentSpec[]>;
}

export interface AgentCustomizer {
  customizeBuild(
    context: AgentBuildContext,
    spec: DurableAgentSpec,
    draft: AgentBuildDraft,
  ): Promise<void>;
  afterCreate?(
    identity: string,
    sessionId: string,
    context: SessionCreatedContext,
  ): Promise<void>;
}

export interface TopologyProvider {
  computeEdges(
    targetIdentities: string[],
    context: unknown,
  ): Promise<ManagedPeerEdge[]>;
}

// =========================================================================
// WorkGraph
// =========================================================================
//
// Wire shapes mirror meerkat-workgraph 0.7.23's `WorkItem` / `WorkEdge` /
// `WorkAttentionBinding` / `WorkGraphSnapshot` / `WorkGraphEvent` (serde,
// snake_case) verbatim, per docs/design/workgraph-wire-contract.md.
// `machineState` and `completionPolicy` on `WorkGraphItem`, and `target` /
// `status` / `projectionPolicy` on `WorkGraphAttentionBinding`, are
// internally-tagged Rust enums/structs the SDK does not re-model — they pass
// through as opaque JSON so a future upstream variant never breaks parsing.

// -- WorkGraphOwnerKey / WorkGraphOwner ------------------------------------

export interface WorkGraphOwnerKey {
  readonly kind: string;
  readonly id: string;
}

export function parseWorkGraphOwnerKey(raw: unknown): WorkGraphOwnerKey {
  const d = asRecord(raw);
  return { kind: String(d.kind ?? ""), id: String(d.id ?? "") };
}

/** Input form of {@link WorkGraphOwnerKey} for claim/goal-target requests. */
export interface WorkGraphOwnerKeyInput {
  readonly kind: string;
  readonly id: string;
}

export function workGraphOwnerKeyInputToDict(
  owner: WorkGraphOwnerKeyInput,
): Record<string, unknown> {
  return { kind: owner.kind, id: owner.id };
}

export interface WorkGraphOwner {
  readonly key: WorkGraphOwnerKey;
  readonly displayName: string | null;
}

export function parseWorkGraphOwner(raw: unknown): WorkGraphOwner {
  const d = asRecord(raw);
  return {
    key: parseWorkGraphOwnerKey(d.key),
    displayName: typeof d.display_name === "string" ? d.display_name : null,
  };
}

/** Input form for `workgraphClaim`'s `owner` parameter. */
export interface WorkGraphOwnerInput {
  readonly kind: string;
  readonly id: string;
  readonly displayName?: string;
}

export function workGraphOwnerInputToDict(
  owner: WorkGraphOwnerInput,
): Record<string, unknown> {
  const result: Record<string, unknown> = { kind: owner.kind, id: owner.id };
  if (owner.displayName !== undefined) result.display_name = owner.displayName;
  return result;
}

// -- WorkGraphClaim ---------------------------------------------------------

export interface WorkGraphClaim {
  readonly owner: WorkGraphOwner;
  readonly claimedAt: string;
  readonly leaseExpiresAt: string | null;
}

export function parseWorkGraphClaim(raw: unknown): WorkGraphClaim {
  const d = asRecord(raw);
  return {
    owner: parseWorkGraphOwner(d.owner),
    claimedAt: String(d.claimed_at ?? ""),
    leaseExpiresAt:
      typeof d.lease_expires_at === "string" ? d.lease_expires_at : null,
  };
}

// -- WorkGraphExternalRef / WorkGraphEvidenceRef ---------------------------

export interface WorkGraphExternalRef {
  readonly kind: string;
  readonly id: string;
  readonly url: string | null;
}

export function parseWorkGraphExternalRef(raw: unknown): WorkGraphExternalRef {
  const d = asRecord(raw);
  return {
    kind: String(d.kind ?? ""),
    id: String(d.id ?? ""),
    url: typeof d.url === "string" ? d.url : null,
  };
}

export interface WorkGraphEvidenceRef {
  readonly kind: string;
  readonly id: string;
  readonly label: string | null;
  readonly summary: string | null;
  readonly confirmationKind: string | null;
  readonly confirmingOwnerKey: WorkGraphOwnerKey | null;
}

export function parseWorkGraphEvidenceRef(raw: unknown): WorkGraphEvidenceRef {
  const d = asRecord(raw);
  return {
    kind: String(d.kind ?? ""),
    id: String(d.id ?? ""),
    label: typeof d.label === "string" ? d.label : null,
    summary: typeof d.summary === "string" ? d.summary : null,
    confirmationKind:
      typeof d.confirmation_kind === "string" ? d.confirmation_kind : null,
    confirmingOwnerKey:
      d.confirming_owner_key != null
        ? parseWorkGraphOwnerKey(d.confirming_owner_key)
        : null,
  };
}

/** Input form for `workgraphAddEvidence` / `workgraphGoalConfirm`'s `evidence` parameter. */
export interface WorkGraphEvidenceInput {
  readonly kind: string;
  readonly id: string;
  readonly label?: string;
  readonly summary?: string;
}

export function workGraphEvidenceInputToDict(
  evidence: WorkGraphEvidenceInput,
): Record<string, unknown> {
  const result: Record<string, unknown> = {
    kind: evidence.kind,
    id: evidence.id,
  };
  if (evidence.label !== undefined) result.label = evidence.label;
  if (evidence.summary !== undefined) result.summary = evidence.summary;
  return result;
}

// -- WorkGraphItem -----------------------------------------------------------

export interface WorkGraphItem {
  readonly id: string;
  readonly realmId: string;
  readonly namespace: string;
  readonly title: string;
  readonly description: string | null;
  readonly status: string;
  /** Internally-tagged `WorkCompletionPolicy` enum — opaque passthrough. */
  readonly completionPolicy: unknown;
  readonly priority: string;
  readonly labels: readonly string[];
  readonly owner: WorkGraphOwner | null;
  readonly claim: WorkGraphClaim | null;
  /** Machine-owned lifecycle/revision authority — opaque passthrough. */
  readonly machineState: unknown;
  readonly revision: number;
  readonly dueAt: string | null;
  readonly notBefore: string | null;
  readonly snoozedUntil: string | null;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly terminalAt: string | null;
  readonly externalRefs: readonly WorkGraphExternalRef[];
  readonly evidenceRefs: readonly WorkGraphEvidenceRef[];
}

export function parseWorkGraphItem(raw: unknown): WorkGraphItem {
  const d = asRecord(raw);
  return {
    id: String(d.id ?? ""),
    realmId: String(d.realm_id ?? ""),
    namespace: String(d.namespace ?? ""),
    title: String(d.title ?? ""),
    description: typeof d.description === "string" ? d.description : null,
    status: String(d.status ?? ""),
    completionPolicy: d.completion_policy,
    priority: String(d.priority ?? ""),
    labels: asStringArray(d.labels),
    owner: d.owner != null ? parseWorkGraphOwner(d.owner) : null,
    claim: d.claim != null ? parseWorkGraphClaim(d.claim) : null,
    machineState: d.machine_state,
    revision: Number(d.revision ?? 0),
    dueAt: typeof d.due_at === "string" ? d.due_at : null,
    notBefore: typeof d.not_before === "string" ? d.not_before : null,
    snoozedUntil: typeof d.snoozed_until === "string" ? d.snoozed_until : null,
    createdAt: String(d.created_at ?? ""),
    updatedAt: String(d.updated_at ?? ""),
    terminalAt: typeof d.terminal_at === "string" ? d.terminal_at : null,
    externalRefs: asRecordArray(d.external_refs).map(parseWorkGraphExternalRef),
    evidenceRefs: asRecordArray(d.evidence_refs).map(parseWorkGraphEvidenceRef),
  };
}

// -- WorkGraphEdge -----------------------------------------------------------

export interface WorkGraphEdge {
  readonly realmId: string;
  readonly namespace: string;
  readonly kind: string;
  readonly fromId: string;
  readonly toId: string;
  readonly createdAt: string;
}

export function parseWorkGraphEdge(raw: unknown): WorkGraphEdge {
  const d = asRecord(raw);
  return {
    realmId: String(d.realm_id ?? ""),
    namespace: String(d.namespace ?? ""),
    kind: String(d.kind ?? ""),
    fromId: String(d.from_id ?? ""),
    toId: String(d.to_id ?? ""),
    createdAt: String(d.created_at ?? ""),
  };
}

// -- WorkGraphAttentionBinding ------------------------------------------------

export interface WorkGraphWorkRef {
  readonly itemId: string;
  readonly realmId: string;
  readonly namespace: string;
}

export function parseWorkGraphWorkRef(raw: unknown): WorkGraphWorkRef {
  const d = asRecord(raw);
  return {
    itemId: String(d.item_id ?? ""),
    realmId: String(d.realm_id ?? ""),
    namespace: String(d.namespace ?? ""),
  };
}

export interface WorkGraphAttentionBinding {
  readonly bindingId: string;
  readonly workRef: WorkGraphWorkRef;
  /** Internally-tagged `WorkAttentionTarget` enum (session | lowered_owner) — opaque passthrough. */
  readonly target: Readonly<Record<string, unknown>>;
  readonly mode: string;
  /** Internally-tagged `WorkAttentionStatus` enum (active | paused{until} | superseded | stopped) — opaque passthrough. */
  readonly status: Readonly<Record<string, unknown>>;
  /** Machine-owned lifecycle/revision authority — opaque passthrough. */
  readonly machineState: unknown;
  readonly delegatedAuthority: string;
  readonly projectionPolicy: Readonly<Record<string, unknown>>;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export function parseWorkGraphAttentionBinding(
  raw: unknown,
): WorkGraphAttentionBinding {
  const d = asRecord(raw);
  return {
    bindingId: String(d.binding_id ?? ""),
    workRef: parseWorkGraphWorkRef(d.work_ref),
    target: asRecord(d.target),
    mode: String(d.mode ?? ""),
    status: asRecord(d.status),
    machineState: d.machine_state,
    delegatedAuthority: String(d.delegated_authority ?? ""),
    projectionPolicy: asRecord(d.projection_policy),
    createdAt: String(d.created_at ?? ""),
    updatedAt: String(d.updated_at ?? ""),
  };
}

// -- WorkGraphSnapshotResult / WorkGraphItemsResult --------------------------

export interface WorkGraphSnapshotResult {
  readonly realmId: string;
  readonly namespace: string | null;
  readonly allNamespaces: boolean;
  readonly capturedAt: string;
  readonly eventHighWaterMark: number | null;
  readonly items: readonly WorkGraphItem[];
  readonly edges: readonly WorkGraphEdge[];
  readonly attention: readonly WorkGraphAttentionBinding[];
  readonly readyItemIds: readonly string[];
}

export function parseWorkGraphSnapshotResult(
  raw: unknown,
): WorkGraphSnapshotResult {
  const d = asRecord(raw);
  return {
    realmId: String(d.realm_id ?? ""),
    namespace: typeof d.namespace === "string" ? d.namespace : null,
    allNamespaces: Boolean(d.all_namespaces),
    capturedAt: String(d.captured_at ?? ""),
    eventHighWaterMark:
      typeof d.event_high_water_mark === "number"
        ? d.event_high_water_mark
        : null,
    items: asRecordArray(d.items).map(parseWorkGraphItem),
    edges: asRecordArray(d.edges).map(parseWorkGraphEdge),
    attention: asRecordArray(d.attention).map(parseWorkGraphAttentionBinding),
    readyItemIds: asStringArray(d.ready_item_ids),
  };
}

export interface WorkGraphItemsResult {
  readonly items: readonly WorkGraphItem[];
}

export function parseWorkGraphItemsResult(raw: unknown): WorkGraphItemsResult {
  const d = asRecord(raw);
  return { items: asRecordArray(d.items).map(parseWorkGraphItem) };
}

// -- WorkGraphGoalResult / WorkGraphAttentionReassignResult ------------------

export interface WorkGraphGoalResult {
  readonly item: WorkGraphItem;
  readonly attention: WorkGraphAttentionBinding;
}

export function parseWorkGraphGoalResult(raw: unknown): WorkGraphGoalResult {
  const d = asRecord(raw);
  return {
    item: parseWorkGraphItem(d.item),
    attention: parseWorkGraphAttentionBinding(d.attention),
  };
}

export interface WorkGraphAttentionReassignResult {
  readonly previous: WorkGraphAttentionBinding;
  readonly attention: WorkGraphAttentionBinding;
}

export function parseWorkGraphAttentionReassignResult(
  raw: unknown,
): WorkGraphAttentionReassignResult {
  const d = asRecord(raw);
  return {
    previous: parseWorkGraphAttentionBinding(d.previous),
    attention: parseWorkGraphAttentionBinding(d.attention),
  };
}

// -- WorkGraphEventEntry ------------------------------------------------------

export interface WorkGraphEventEntry {
  readonly seq: number | null;
  readonly realmId: string;
  readonly namespace: string;
  readonly itemId: string | null;
  readonly kind: string;
  readonly at: string;
  readonly payload: unknown;
}

export function parseWorkGraphEventEntry(raw: unknown): WorkGraphEventEntry {
  const d = asRecord(raw);
  return {
    seq: typeof d.seq === "number" ? d.seq : null,
    realmId: String(d.realm_id ?? ""),
    namespace: String(d.namespace ?? ""),
    itemId: typeof d.item_id === "string" ? d.item_id : null,
    kind: String(d.kind ?? ""),
    at: String(d.at ?? ""),
    payload: d.payload,
  };
}

// -- WorkGraph request-side option helpers -----------------------------------

/** Shared filter for `workgraphSnapshot` / `workgraphList`. */
export interface WorkGraphFilterOptions {
  readonly namespace?: string;
  readonly allNamespaces?: boolean;
  readonly statuses?: readonly string[];
  readonly labels?: readonly string[];
  readonly includeTerminal?: boolean;
  readonly limit?: number;
}

export function workGraphFilterOptionsToDict(
  options: WorkGraphFilterOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.namespace !== undefined) result.namespace = options.namespace;
  if (options.allNamespaces !== undefined) {
    result.all_namespaces = options.allNamespaces;
  }
  if (options.statuses !== undefined && options.statuses.length > 0) {
    result.statuses = [...options.statuses];
  }
  if (options.labels !== undefined && options.labels.length > 0) {
    result.labels = [...options.labels];
  }
  if (options.includeTerminal !== undefined) {
    result.include_terminal = options.includeTerminal;
  }
  if (options.limit !== undefined) result.limit = options.limit;
  return result;
}

export interface WorkGraphReadyOptions {
  readonly namespace?: string;
  readonly labels?: readonly string[];
  readonly limit?: number;
}

export function workGraphReadyOptionsToDict(
  options: WorkGraphReadyOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.namespace !== undefined) result.namespace = options.namespace;
  if (options.labels !== undefined && options.labels.length > 0) {
    result.labels = [...options.labels];
  }
  if (options.limit !== undefined) result.limit = options.limit;
  return result;
}

export interface WorkGraphEventsOptions {
  readonly namespace?: string;
  readonly allNamespaces?: boolean;
  readonly afterSeq?: number;
  readonly limit?: number;
}

export function workGraphEventsOptionsToDict(
  options: WorkGraphEventsOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.namespace !== undefined) result.namespace = options.namespace;
  if (options.allNamespaces !== undefined) {
    result.all_namespaces = options.allNamespaces;
  }
  if (options.afterSeq !== undefined) result.after_seq = options.afterSeq;
  if (options.limit !== undefined) result.limit = options.limit;
  return result;
}

export interface WorkGraphAttentionListOptions {
  readonly namespace?: string;
  readonly status?: string;
}

export function workGraphAttentionListOptionsToDict(
  options: WorkGraphAttentionListOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.namespace !== undefined) result.namespace = options.namespace;
  if (options.status !== undefined) result.status = options.status;
  return result;
}

/** Options for `workgraphCreate` beyond the required `title`. */
export interface WorkGraphCreateOptions {
  readonly description?: string;
  readonly priority?: string;
  /** Internally-tagged `WorkCompletionPolicy` enum, e.g. `{ kind: "self_attest" }`. */
  readonly completionPolicy?: unknown;
  readonly labels?: readonly string[];
  readonly dueAt?: string;
  readonly notBefore?: string;
  readonly snoozedUntil?: string;
  readonly externalRefs?: readonly Record<string, unknown>[];
  readonly evidenceRefs?: readonly Record<string, unknown>[];
  readonly status?: "open" | "blocked";
  readonly namespace?: string;
}

export function workGraphCreateOptionsToDict(
  options: WorkGraphCreateOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.description !== undefined) result.description = options.description;
  if (options.priority !== undefined) result.priority = options.priority;
  if (options.completionPolicy !== undefined) {
    result.completion_policy = options.completionPolicy;
  }
  if (options.labels !== undefined && options.labels.length > 0) {
    result.labels = [...options.labels];
  }
  if (options.dueAt !== undefined) result.due_at = options.dueAt;
  if (options.notBefore !== undefined) result.not_before = options.notBefore;
  if (options.snoozedUntil !== undefined) {
    result.snoozed_until = options.snoozedUntil;
  }
  if (options.externalRefs !== undefined && options.externalRefs.length > 0) {
    result.external_refs = [...options.externalRefs];
  }
  if (options.evidenceRefs !== undefined && options.evidenceRefs.length > 0) {
    result.evidence_refs = [...options.evidenceRefs];
  }
  if (options.status !== undefined) result.status = options.status;
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

/** Options for `workgraphUpdate` beyond `id`/`expectedRevision`. */
export interface WorkGraphUpdateOptions {
  readonly title?: string;
  readonly description?: string;
  readonly priority?: string;
  /** Explicit `[]` clears labels; omit to leave labels untouched. */
  readonly labels?: readonly string[];
  readonly dueAt?: string;
  readonly notBefore?: string;
  readonly snoozedUntil?: string;
  readonly namespace?: string;
}

export function workGraphUpdateOptionsToDict(
  options: WorkGraphUpdateOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.title !== undefined) result.title = options.title;
  if (options.description !== undefined) result.description = options.description;
  if (options.priority !== undefined) result.priority = options.priority;
  if (options.labels !== undefined) result.labels = [...options.labels];
  if (options.dueAt !== undefined) result.due_at = options.dueAt;
  if (options.notBefore !== undefined) result.not_before = options.notBefore;
  if (options.snoozedUntil !== undefined) {
    result.snoozed_until = options.snoozedUntil;
  }
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

export interface WorkGraphClaimOptions {
  readonly leaseSeconds?: number;
  readonly namespace?: string;
}

export function workGraphClaimOptionsToDict(
  options: WorkGraphClaimOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.leaseSeconds !== undefined) {
    result.lease_seconds = options.leaseSeconds;
  }
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

export interface WorkGraphCloseOptions {
  readonly status?: "completed" | "cancelled" | "failed";
  readonly namespace?: string;
}

export function workGraphCloseOptionsToDict(
  options: WorkGraphCloseOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.status !== undefined) result.status = options.status;
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

/**
 * Attention/goal target — mirrors upstream `GoalAttentionTarget` plus the
 * `identity` convenience form (lowered server-side via
 * `lower_agent_identity_attention_target` using the runtime's mob id). Used
 * by both `workgraphGoalCreate` and `workgraphAttentionReassign`.
 */
export type WorkGraphGoalTarget =
  | { readonly kind: "session"; readonly sessionId: string }
  | { readonly kind: "identity"; readonly identity: string }
  | { readonly kind: "owner"; readonly ownerKey: WorkGraphOwnerKeyInput };

export function workGraphGoalTargetToDict(
  target: WorkGraphGoalTarget,
): Record<string, unknown> {
  switch (target.kind) {
    case "session":
      return { kind: "session", session_id: target.sessionId };
    case "identity":
      return { kind: "identity", identity: target.identity };
    case "owner":
      return {
        kind: "owner",
        owner_key: workGraphOwnerKeyInputToDict(target.ownerKey),
      };
  }
}

export interface WorkGraphGoalCreateOptions {
  readonly description?: string;
  readonly mode?: string;
  /** Internally-tagged `WorkCompletionPolicy` enum, e.g. `{ kind: "self_attest" }`. */
  readonly completionPolicy?: unknown;
  readonly delegatedAuthority?: string;
  readonly namespace?: string;
}

export function workGraphGoalCreateOptionsToDict(
  options: WorkGraphGoalCreateOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.description !== undefined) result.description = options.description;
  if (options.mode !== undefined) result.mode = options.mode;
  if (options.completionPolicy !== undefined) {
    result.completion_policy = options.completionPolicy;
  }
  if (options.delegatedAuthority !== undefined) {
    result.delegated_authority = options.delegatedAuthority;
  }
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

export interface WorkGraphGoalConfirmOptions {
  readonly evidence?: WorkGraphEvidenceInput;
  readonly namespace?: string;
}

export function workGraphGoalConfirmOptionsToDict(
  options: WorkGraphGoalConfirmOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.evidence !== undefined) {
    result.evidence = workGraphEvidenceInputToDict(options.evidence);
  }
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

export interface WorkGraphGoalRequestCloseOptions {
  readonly status?: "completed" | "cancelled" | "failed";
  readonly namespace?: string;
}

export function workGraphGoalRequestCloseOptionsToDict(
  options: WorkGraphGoalRequestCloseOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.status !== undefined) result.status = options.status;
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

export interface WorkGraphAttentionPauseOptions {
  readonly until?: string;
  readonly namespace?: string;
}

export function workGraphAttentionPauseOptionsToDict(
  options: WorkGraphAttentionPauseOptions = {},
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  if (options.until !== undefined) result.until = options.until;
  if (options.namespace !== undefined) result.namespace = options.namespace;
  return result;
}

// -- Identity bootstrap status -----------------------------------------------

/**
 * Typed eager-restore progress for one identity, published on the bootstrap
 * status while the restore pass runs. `stage` is `registered`, `resuming`,
 * `minted`, `resumed` or `refused` (tolerate future values). A `refused`
 * stage carries the refusal's JSON-RPC `code` (the code the same refusal
 * answers a call with: `-32051`, `-32052`, `-32053` or `-32603`) and its
 * `kind` when the refusal has one.
 */
export interface IdentityRestoreProgress {
  readonly stage: string;
  readonly kind: string | null;
  readonly code: number | null;
}

/** Bootstrap progress for one durable identity. */
export interface IdentityBootstrapEntry {
  readonly identity: string;
  /** `dormant`, `warming`, `active` or `broken` (tolerate future values). */
  readonly state: string;
  readonly error: string | null;
  /** Typed eager-restore progress; `null` outside an eager restore pass. */
  readonly restore: IdentityRestoreProgress | null;
}

/**
 * A row of the `storage-migrate-native` first-boot manifest that the gateway
 * has not carried out yet. `kind` is `"resume"` (the identity resumes the
 * session the migration recorded) or `"fresh"` (it starts on a fresh
 * session). `waitingFor` is `"roster"` (the identity list does not name the
 * identity yet) or `"carry_out"` (it does, and the row is not carried out
 * yet). Newer values are kept as sent.
 */
export interface FirstBootPendingRow {
  readonly identity: string;
  readonly kind: "resume" | "fresh" | (string & {});
  readonly waitingFor: "roster" | "carry_out" | (string & {});
}

/**
 * A roster identity a candidate launch left to the authoritative launch:
 * waking it would change the stored roster. `reason` is `"create_member"`
 * (the store holds no member for it), `"move_role"` (its stored member moves
 * to another role), `"fresh_session"` (its stored session is gone),
 * `"first_boot_fresh"` (a pending fresh first-boot row) or `"resume_member"`
 * (a member would be spawned onto its stored session: Core holds no live
 * member for it, or Meta 0.9.3 spawned its member). Newer values are kept as
 * sent.
 */
export interface DeferredIdentity {
  readonly identity: string;
  readonly reason:
    | "create_member"
    | "move_role"
    | "fresh_session"
    | "first_boot_fresh"
    | "resume_member"
    | (string & {});
}

/**
 * Identity bootstrap snapshot (`meta/status_identity_bootstrap`).
 * `firstBootPending` lists the first-boot manifest rows still to be carried
 * out after `storage-migrate-native`; it is empty when none is pending.
 * `deferred` lists the identities a candidate launch left to the
 * authoritative launch (`counts.deferred` counts them); they are not in
 * `identities` and count neither as broken nor against `ready`. It is empty
 * on every other launch, and optional so a status a caller builds still
 * type-checks.
 */
export interface IdentityBootstrapStatus {
  readonly mode: Readonly<Record<string, unknown>>;
  readonly complete: boolean;
  readonly ready: boolean;
  readonly error: string | null;
  readonly counts: Readonly<Record<string, number>>;
  readonly identities: Readonly<Record<string, IdentityBootstrapEntry>>;
  /** Set by `meta/wait_identity_bootstrap` only. */
  readonly timedOut: boolean | null;
  readonly target: string | null;
  readonly startupReady: boolean | null;
  readonly firstBootPending: readonly FirstBootPendingRow[];
  readonly deferred?: readonly DeferredIdentity[];
}

/**
 * Every deferred identity the gateway sends is kept, so the deferred count
 * stays exact; a field that is not a string reads as `""` (identity) or
 * `"unknown"`.
 */
export function parseDeferredIdentity(raw: unknown): DeferredIdentity {
  const d = asRecord(raw);
  return {
    identity: typeof d.identity === "string" ? d.identity : "",
    reason: typeof d.reason === "string" ? d.reason : "unknown",
  };
}

/**
 * Every row the gateway sends is kept, so the pending count stays exact; a
 * field that is not a string reads as `""` (identity) or `"unknown"`.
 */
export function parseFirstBootPendingRow(raw: unknown): FirstBootPendingRow {
  const d = asRecord(raw);
  return {
    identity: typeof d.identity === "string" ? d.identity : "",
    kind: typeof d.kind === "string" ? d.kind : "unknown",
    waitingFor: typeof d.waiting_for === "string" ? d.waiting_for : "unknown",
  };
}

export function parseIdentityRestoreProgress(
  raw: unknown,
): IdentityRestoreProgress | null {
  if (typeof raw !== "object" || raw === null) return null;
  const d = raw as Record<string, unknown>;
  if (typeof d.stage !== "string") return null;
  return {
    stage: d.stage,
    kind: typeof d.kind === "string" ? d.kind : null,
    code: typeof d.code === "number" ? d.code : null,
  };
}

export function parseIdentityBootstrapStatus(raw: unknown): IdentityBootstrapStatus {
  const d = asRecord(raw);
  const identities: Record<string, IdentityBootstrapEntry> = {};
  for (const [identity, value] of Object.entries(asRecord(d.identities))) {
    const entry = asRecord(value);
    identities[identity] = {
      identity,
      state: typeof entry.state === "string" ? entry.state : "unknown",
      error: typeof entry.error === "string" ? entry.error : null,
      restore: parseIdentityRestoreProgress(entry.restore),
    };
  }
  const counts: Record<string, number> = {};
  for (const [key, value] of Object.entries(asRecord(d.counts))) {
    counts[key] = Number(value ?? 0);
  }
  return {
    mode: asRecord(d.mode),
    complete: d.complete === true,
    ready: d.ready === true,
    error: typeof d.error === "string" ? d.error : null,
    counts,
    identities,
    timedOut: typeof d.timed_out === "boolean" ? d.timed_out : null,
    target: typeof d.target === "string" ? d.target : null,
    startupReady: typeof d.startup_ready === "boolean" ? d.startup_ready : null,
    firstBootPending: Array.isArray(d.first_boot_pending)
      ? d.first_boot_pending.map(parseFirstBootPendingRow)
      : [],
    deferred: Array.isArray(d.deferred) ? d.deferred.map(parseDeferredIdentity) : [],
  };
}

// -- Mob announcements ----------------------------------------------------
//
// Result models for `meta/mob_announcements/*`. Every wire vocabulary
// (states, phases, statuses, outcomes, actions, reasons, kinds) is an open
// union, so a newer gateway's value reaches the caller instead of failing the
// parse. Nested declarations the caller wrote (a source's identity principal,
// audience, start, delivery, health policy, ...) are kept as the gateway sent
// them, in wire (snake_case) form.

/** The `feature_capabilities` entry of a gateway that serves sources. */
export const MOB_ANNOUNCEMENT_SOURCES_V1 = "mob_announcements.sources.v1" as const;

function wireInteger(value: unknown): number | null {
  return typeof value === "number" && Number.isInteger(value) ? value : null;
}

function wireRecordOrNull(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function wireNumberRecord(value: unknown): Record<string, number> {
  const result: Record<string, number> = {};
  for (const [key, entry] of Object.entries(asRecord(value))) {
    result[key] = Number(entry ?? 0);
  }
  return result;
}

/** The member a subscription delivers to: an identity within a mob. */
export interface AnnouncementMember {
  readonly mobId: string;
  readonly identity: string;
}

export function parseAnnouncementMember(raw: unknown): AnnouncementMember {
  const d = asRecord(raw);
  return { mobId: String(d.mob_id ?? ""), identity: String(d.identity ?? "") };
}

/** One announcement by its stable identity: its mob and source key. */
export interface AnnouncementRef {
  readonly mobId: string;
  readonly sourceKey: string;
}

/**
 * Who made an announcement: the host, a member (`identity`), a source
 * (`sourceId`) or a newer kind. `raw` keeps the object as sent.
 */
export interface Announcer {
  readonly kind: "host" | "member" | "source" | (string & {});
  readonly identity: string | null;
  readonly sourceId: string | null;
  readonly raw: Readonly<Record<string, unknown>>;
}

/** A committed, immutable mob announcement. */
export interface Announcement {
  readonly mobId: string;
  readonly sequence: number;
  readonly announcedBy: Announcer;
  readonly sourceKey: string;
  readonly topics: readonly string[];
  readonly body: string;
  readonly correlationId: string | null;
  readonly causes: readonly AnnouncementRef[];
  readonly digest: string;
  readonly announcedAtMs: number;
}

export function parseAnnouncement(raw: unknown): Announcement {
  const d = asRecord(raw);
  const by = asRecord(d.announced_by);
  return {
    mobId: String(d.mob_id ?? ""),
    sequence: Number(d.sequence ?? 0),
    announcedBy: {
      kind: String(by.kind ?? ""),
      identity: optionalString(by.identity),
      sourceId: optionalString(by.source_id),
      raw: by,
    },
    sourceKey: String(d.source_key ?? ""),
    topics: asStringArray(d.topics),
    body: String(d.body ?? ""),
    correlationId: optionalString(d.correlation_id),
    causes: asRecordArray(d.causes).map((cause) => ({
      mobId: String(cause.mob_id ?? ""),
      sourceKey: String(cause.source_key ?? ""),
    })),
    digest: String(d.digest ?? ""),
    announcedAtMs: Number(d.announced_at_ms ?? 0),
  };
}

/**
 * The answer to an announce: the committed announcement, whether it replayed
 * an identical earlier one, and how many subscriptions captured it (a count,
 * never their names).
 */
export interface AnnounceResult {
  readonly announcement: Announcement;
  readonly replayed: boolean;
  readonly captured: number;
}

export function parseAnnounceResult(raw: unknown): AnnounceResult {
  const d = asRecord(raw);
  return {
    announcement: parseAnnouncement(d.announcement),
    replayed: d.replayed === true,
    captured: Number(d.captured ?? 0),
  };
}

/**
 * A durable subscription. `frontier` maps each mob to the sequence after
 * which the selector in force captures; `ended` says why an ended
 * subscription ended. `delivery` is how its member takes what it captures,
 * as set on the subscription (`{await_handling}`), and `awaitHandling` reads
 * it; both are `null` when it was not set, which leaves the gateway's
 * default.
 */
export interface AnnouncementSubscription {
  readonly id: string;
  readonly member: AnnouncementMember;
  readonly topics: readonly string[];
  readonly mobs: readonly string[];
  readonly state: "active" | "paused" | "cancelled" | "ended" | (string & {});
  readonly revision: number;
  readonly selectorRevision: number;
  readonly frontier: Readonly<Record<string, number>>;
  readonly endedMobs: readonly string[];
  readonly ended: Readonly<Record<string, unknown>> | null;
  readonly memberMobCursor: number | null;
  readonly createdAtMs: number;
  readonly updatedAtMs: number;
  readonly delivery: Readonly<Record<string, unknown>> | null;
  readonly awaitHandling: boolean | null;
}

export function parseAnnouncementSubscription(raw: unknown): AnnouncementSubscription {
  const d = asRecord(raw);
  const selector = asRecord(d.selector);
  const delivery = wireRecordOrNull(d.delivery);
  const awaitHandling = delivery === null ? undefined : delivery.await_handling;
  return {
    id: String(d.id ?? ""),
    member: parseAnnouncementMember(d.member),
    topics: asStringArray(selector.topics),
    mobs: asStringArray(selector.mobs),
    state: String(d.state ?? ""),
    revision: Number(d.revision ?? 0),
    selectorRevision: Number(d.selector_revision ?? 0),
    frontier: wireNumberRecord(d.frontier),
    endedMobs: asStringArray(d.ended_mobs),
    ended: wireRecordOrNull(d.ended),
    memberMobCursor: wireInteger(d.member_mob_cursor),
    createdAtMs: Number(d.created_at_ms ?? 0),
    updatedAtMs: Number(d.updated_at_ms ?? 0),
    delivery,
    awaitHandling: typeof awaitHandling === "boolean" ? awaitHandling : null,
  };
}

/**
 * A subscription pause, resume or cancel: the subscription as it is now, and
 * the batches that still hold its announcements and have not settled (a
 * change never rewrites a sealed batch).
 */
/**
 * The members a role template takes: every member of `mobId` its roster
 * records with `role` (the profile it was spawned from).
 */
export interface AnnouncementRoleSelector {
  readonly mobId: string;
  readonly role: string;
}

export function parseAnnouncementRoleSelector(raw: unknown): AnnouncementRoleSelector {
  const d = asRecord(raw);
  return { mobId: String(d.mob_id ?? ""), role: String(d.role ?? "") };
}

/**
 * A role template: a subscription for every member of a mob with a role. It
 * never delivers by itself: each member it takes captures through a child
 * subscription with the id `"<template id>/<16 hex digits>"`. `delivery` is
 * every child's delivery, as set on the template (`{await_handling}`), and
 * `awaitHandling` reads it; both are `null` when it was not set.
 */
export interface AnnouncementRoleTemplate {
  readonly id: string;
  readonly members: AnnouncementRoleSelector;
  readonly topics: readonly string[];
  readonly mobs: readonly string[];
  readonly state: "active" | "paused" | "cancelled" | (string & {});
  readonly revision: number;
  readonly createdAtMs: number;
  readonly updatedAtMs: number;
  readonly delivery: Readonly<Record<string, unknown>> | null;
  readonly awaitHandling: boolean | null;
}

export function parseAnnouncementRoleTemplate(raw: unknown): AnnouncementRoleTemplate {
  const d = asRecord(raw);
  const delivery = wireRecordOrNull(d.delivery);
  const awaitHandling = delivery === null ? undefined : delivery.await_handling;
  return {
    id: String(d.id ?? ""),
    members: parseAnnouncementRoleSelector(d.members),
    topics: asStringArray(d.topics),
    mobs: asStringArray(d.mobs),
    state: String(d.state ?? ""),
    revision: Number(d.revision ?? 0),
    createdAtMs: Number(d.created_at_ms ?? 0),
    updatedAtMs: Number(d.updated_at_ms ?? 0),
    delivery,
    awaitHandling: typeof awaitHandling === "boolean" ? awaitHandling : null,
  };
}

/**
 * A subscription pause, resume or cancel: the subscription as it is now, and
 * the batches that still hold its announcements and have not settled. The
 * same verbs on a role template's id change the template and its children in
 * one step: then `template` and `children` are set and `subscription` is
 * `null`.
 */
export interface AnnouncementSubscriptionChange {
  readonly subscription: AnnouncementSubscription | null;
  readonly unresolvedBatches: readonly string[];
  readonly template: AnnouncementRoleTemplate | null;
  readonly children: readonly AnnouncementSubscription[];
}

export function parseAnnouncementSubscriptionChange(raw: unknown): AnnouncementSubscriptionChange {
  const d = asRecord(raw);
  const template = wireRecordOrNull(d.template);
  if (template !== null) {
    return {
      subscription: null,
      unresolvedBatches: asStringArray(d.unresolved_batches),
      template: parseAnnouncementRoleTemplate(template),
      children: Array.isArray(d.children) ? d.children.map(parseAnnouncementSubscription) : [],
    };
  }
  return {
    subscription: parseAnnouncementSubscription(d.subscription),
    unresolvedBatches: asStringArray(d.unresolved_batches),
    template: null,
    children: [],
  };
}

/**
 * Why a member's deliveries wait now, and on what evidence. `causeKind` is
 * the cause's kind (`"peer_composing"`, `"member_not_live"`, ... or newer);
 * `cause` keeps the object as sent.
 */
export interface AnnouncementSuspendMarker {
  readonly cause: Readonly<Record<string, unknown>>;
  readonly causeKind: string;
  readonly evidence: string;
  readonly sinceMs: number;
}

function parseOptionalSuspendMarker(raw: unknown): AnnouncementSuspendMarker | null {
  const d = wireRecordOrNull(raw);
  if (d === null) return null;
  const cause = asRecord(d.cause);
  return {
    cause,
    causeKind: String(cause.kind ?? ""),
    evidence: String(d.evidence ?? ""),
    sinceMs: Number(d.since_ms ?? 0),
  };
}

/**
 * `subscriptions/get`: the subscription (`null` when unknown), why its
 * member's deliveries wait if they do, and its backlog when the gateway
 * reports one.
 */
export interface AnnouncementSubscriptionView {
  readonly subscription: AnnouncementSubscription | null;
  readonly marker: AnnouncementSuspendMarker | null;
  readonly backlog: Readonly<Record<string, unknown>> | null;
  /** A role template's id reads the template here, beside no subscription. */
  readonly template: AnnouncementRoleTemplate | null;
}

export function parseAnnouncementSubscriptionView(raw: unknown): AnnouncementSubscriptionView {
  const d = asRecord(raw);
  const subscription = wireRecordOrNull(d.subscription);
  const template = wireRecordOrNull(d.template);
  return {
    subscription: subscription === null ? null : parseAnnouncementSubscription(subscription),
    marker: parseOptionalSuspendMarker(d.marker),
    backlog: wireRecordOrNull(d.backlog),
    template: template === null ? null : parseAnnouncementRoleTemplate(template),
  };
}

/** One captured announcement in a batch, under the subscription revision that captured it. */
export interface AnnouncementBatchItem {
  readonly subscriptionId: string;
  readonly subscriptionRevision: number;
  readonly mobId: string;
  readonly sequence: number;
  readonly sourceKey: string;
}

/** Announcements owed to a batch's member that it reports as not delivered. */
export interface AnnouncementGap {
  readonly mobId: string;
  readonly count: number;
  readonly reason:
    | "expired"
    | "not_retained"
    | "delivery_not_confirmed"
    | "admission_not_confirmed"
    | (string & {});
}

/**
 * What became of a delivered batch's turn, as Meta read it, and what it did
 * about it. `cause` is why the turn did not complete (the receipt's
 * completion type, or the abandon reason when there is none). `redelivered`
 * counts the items owed again with a notice, `held` the items held for an
 * operator, and `dismissed` the held items an operator settled.
 */
export interface AnnouncementBatchHandling {
  readonly state: "pending" | "handled" | "handled_unverified" | "unhandled" | "unknown" | (string & {});
  readonly cause: string | null;
  readonly redelivered: number;
  readonly held: number;
  readonly dismissed: number;
  readonly readAtMs: number;
}

export function parseAnnouncementBatchHandling(raw: unknown): AnnouncementBatchHandling {
  const d = asRecord(raw);
  return {
    state: String(d.state ?? ""),
    cause: optionalString(d.cause),
    redelivered: Number(d.redelivered ?? 0),
    held: Number(d.held ?? 0),
    dismissed: Number(d.dismissed ?? 0),
    readAtMs: Number(d.read_at_ms ?? 0),
  };
}

function parseOptionalBatchHandling(raw: unknown): AnnouncementBatchHandling | null {
  return wireRecordOrNull(raw) === null ? null : parseAnnouncementBatchHandling(raw);
}

/**
 * An immutable delivery batch for one member. `state` is the delivery
 * state's tag and `stateDetail` the full state object as sent. `route` is
 * `"local"`, `"child"`, `"peer"` or newer. `handling` is `null` from a
 * gateway that does not report it.
 */
export interface AnnouncementBatch {
  readonly id: string;
  readonly member: AnnouncementMember;
  readonly items: readonly AnnouncementBatchItem[];
  readonly gaps: readonly AnnouncementGap[];
  readonly route: "local" | "child" | "peer" | (string & {});
  readonly body: string;
  readonly contentDigest: string;
  readonly predecessor: string | null;
  readonly successor: string | null;
  readonly state:
    | "sealed"
    | "submitting"
    | "committed"
    | "delivered"
    | "refused"
    | "stranded"
    | "cancelled"
    | "blocked"
    | "expired"
    | "expired_unverified"
    | "committed_unconfirmed"
    | (string & {});
  readonly stateDetail: Readonly<Record<string, unknown>>;
  /** What became of its turn: `null` until the batch is admitted, and from a gateway that does not report it. */
  readonly handling: AnnouncementBatchHandling | null;
  readonly sealedAtMs: number;
  readonly updatedAtMs: number;
}

export function parseAnnouncementBatch(raw: unknown): AnnouncementBatch {
  const d = asRecord(raw);
  const state = asRecord(d.state);
  return {
    id: String(d.id ?? ""),
    member: parseAnnouncementMember(d.member),
    items: asRecordArray(d.items).map((item) => ({
      subscriptionId: String(item.subscription_id ?? ""),
      subscriptionRevision: Number(item.subscription_revision ?? 0),
      mobId: String(item.mob_id ?? ""),
      sequence: Number(item.sequence ?? 0),
      sourceKey: String(item.source_key ?? ""),
    })),
    gaps: asRecordArray(d.gaps).map((gap) => ({
      mobId: String(gap.mob_id ?? ""),
      count: Number(gap.count ?? 0),
      reason: String(gap.reason ?? ""),
    })),
    route: String(d.route ?? ""),
    body: String(d.body ?? ""),
    contentDigest: String(d.content_digest ?? ""),
    predecessor: optionalString(d.predecessor),
    successor: optionalString(d.successor),
    state: String(state.state ?? ""),
    stateDetail: state,
    handling: parseOptionalBatchHandling(d.handling),
    sealedAtMs: Number(d.sealed_at_ms ?? 0),
    updatedAtMs: Number(d.updated_at_ms ?? 0),
  };
}

/**
 * `deliveries/observe` and `deliveries/reconcile`: the batch, the native
 * owner's live status (`native`, as sent; `nativeStatus` is its `status`),
 * the admitted input's own state once admitted, and why the member's
 * deliveries wait, if they do.
 */
export interface AnnouncementDeliveryObservation {
  readonly batch: AnnouncementBatch;
  readonly native: Readonly<Record<string, unknown>>;
  readonly nativeStatus: string;
  readonly input: Readonly<Record<string, unknown>> | null;
  readonly marker: AnnouncementSuspendMarker | null;
  /** What became of the batch's turn: `null` until it is admitted. */
  readonly handling: AnnouncementBatchHandling | null;
}

export function parseAnnouncementDeliveryObservation(raw: unknown): AnnouncementDeliveryObservation {
  const d = asRecord(raw);
  const native = asRecord(d.native);
  return {
    batch: parseAnnouncementBatch(d.batch),
    native,
    nativeStatus: String(native.status ?? ""),
    input: wireRecordOrNull(d.input),
    marker: parseOptionalSuspendMarker(d.marker),
    handling: parseOptionalBatchHandling(d.handling),
  };
}

/**
 * One of a source's routes: `kind` is `"direct"`, `"triage"` or newer; `raw`
 * keeps the route as sent.
 */
export interface AnnouncementSourceRoute {
  readonly kind: "direct" | "triage" | (string & {});
  readonly raw: Readonly<Record<string, unknown>>;
}

/** A source's health. `totals` counts what its collections did. */
export interface AnnouncementSourceHealth {
  readonly status:
    | "baselining"
    | "backfilling"
    | "ok"
    | "failing"
    | "stale"
    | "paused"
    | "ended"
    | (string & {});
  readonly lastSuccessAtMs: number | null;
  readonly lastNewItemAtMs: number | null;
  readonly lastObserved: Readonly<Record<string, unknown>> | null;
  readonly lastObservedAtMs: number | null;
  readonly emptySnapshotSinceMs: number | null;
  readonly consecutiveFailures: number;
  readonly totals: Readonly<Record<string, number>>;
  readonly lastFailure: Readonly<Record<string, unknown>> | null;
  readonly lastConflict: Readonly<Record<string, unknown>> | null;
  /** The source's open escalations. */
  readonly escalations: readonly AnnouncementEscalation[];
  /** Whether quiet detection is configured, as sent; `null` when not reported. */
  readonly quietMonitoring: Readonly<Record<string, unknown>> | null;
  /** Whether empty coverage detection is configured, as sent; `null` when not reported. */
  readonly emptyCoverageMonitoring: Readonly<Record<string, unknown>> | null;
  /**
   * Whether this gateway schedules the source's runner; `null` without a
   * runner, or from a gateway that does not report it. A runner on a gateway
   * that schedules none never fires there.
   */
  readonly runnerScheduledHere: boolean | null;
}

/**
 * A durable announcement source, one generation of it. `revision` rises on
 * every change, across generations, and every collector step names it with
 * the cursor version. The cursor's token and a failure's detail appear only
 * in `sources/get`; a list shows the principal as its kind and digest only.
 */
export interface AnnouncementSource {
  readonly mobId: string;
  readonly sourceId: string;
  readonly generation: number;
  readonly revision: number;
  readonly identity: {
    readonly provider: string;
    readonly principal: Readonly<Record<string, unknown>>;
    readonly scope: Readonly<Record<string, unknown>>;
  };
  readonly connection: Readonly<Record<string, unknown>> | null;
  readonly observation: Readonly<Record<string, unknown>>;
  readonly topics: readonly string[];
  readonly audience: readonly Readonly<Record<string, unknown>>[];
  readonly routes: readonly AnnouncementSourceRoute[];
  readonly conversationMembers: readonly AnnouncementMember[];
  readonly start: Readonly<Record<string, unknown>>;
  readonly delivery: Readonly<Record<string, unknown>>;
  readonly healthPolicy: Readonly<Record<string, unknown>>;
  readonly runner: { readonly hostRunnable: string; readonly everySeconds: number } | null;
  readonly state: "active" | "paused" | "ended" | (string & {});
  readonly collection: "baselining" | "backfilling" | "live" | (string & {});
  readonly cursor: {
    readonly version: number;
    readonly token: string | null;
    readonly advancedAtMs: number | null;
  };
  readonly streams: Readonly<Record<string, number>>;
  readonly backfilled: number;
  readonly health: AnnouncementSourceHealth;
  readonly createdAtMs: number;
  readonly updatedAtMs: number;
}

export function parseAnnouncementSource(raw: unknown): AnnouncementSource {
  const d = asRecord(raw);
  const identity = asRecord(d.identity);
  const cursor = asRecord(d.cursor);
  const runner = wireRecordOrNull(d.runner);
  const health = asRecord(d.health);
  return {
    mobId: String(d.mob_id ?? ""),
    sourceId: String(d.source_id ?? ""),
    generation: Number(d.generation ?? 0),
    revision: Number(d.revision ?? 0),
    identity: {
      provider: String(identity.provider ?? ""),
      principal: asRecord(identity.principal),
      scope: asRecord(identity.scope),
    },
    connection: wireRecordOrNull(d.connection),
    observation: asRecord(d.observation),
    topics: asStringArray(d.topics),
    audience: asRecordArray(d.audience),
    routes: asRecordArray(d.routes).map((route) => ({ kind: String(route.kind ?? ""), raw: route })),
    conversationMembers: asRecordArray(d.conversation_members).map(parseAnnouncementMember),
    start: asRecord(d.start),
    delivery: asRecord(d.delivery),
    healthPolicy: asRecord(d.health_policy),
    runner:
      runner === null
        ? null
        : { hostRunnable: String(runner.host_runnable ?? ""), everySeconds: Number(runner.every_seconds ?? 0) },
    state: String(d.state ?? ""),
    collection: String(d.collection ?? ""),
    cursor: {
      version: Number(cursor.version ?? 0),
      token: optionalString(cursor.token),
      advancedAtMs: wireInteger(cursor.advanced_at_ms),
    },
    streams: wireNumberRecord(d.streams),
    backfilled: Number(d.backfilled ?? 0),
    health: {
      status: String(health.status ?? ""),
      lastSuccessAtMs: wireInteger(health.last_success_at_ms),
      lastNewItemAtMs: wireInteger(health.last_new_item_at_ms),
      lastObserved: wireRecordOrNull(health.last_observed),
      lastObservedAtMs: wireInteger(health.last_observed_at_ms),
      emptySnapshotSinceMs: wireInteger(health.empty_snapshot_since_ms),
      consecutiveFailures: Number(health.consecutive_failures ?? 0),
      totals: wireNumberRecord(health.totals),
      lastFailure: wireRecordOrNull(health.last_failure),
      lastConflict: wireRecordOrNull(health.last_conflict),
      escalations: asRecordArray(health.escalations).map(parseAnnouncementEscalation),
      quietMonitoring: wireRecordOrNull(health.quiet_monitoring),
      emptyCoverageMonitoring: wireRecordOrNull(health.empty_coverage_monitoring),
      runnerScheduledHere: typeof health.runner_scheduled_here === "boolean" ? health.runner_scheduled_here : null,
    },
    createdAtMs: Number(d.created_at_ms ?? 0),
    updatedAtMs: Number(d.updated_at_ms ?? 0),
  };
}

/**
 * `sources/put`: the source as declared. `affectedSubscriptions` and
 * `deliveryReadiness` are `null` from a gateway that does not report them.
 */
export interface AnnouncementSourcePutResult {
  readonly source: AnnouncementSource;
  readonly affectedSubscriptions: number | null;
  readonly deliveryReadiness: Readonly<Record<string, unknown>> | null;
}

export function parseAnnouncementSourcePutResult(raw: unknown): AnnouncementSourcePutResult {
  const d = asRecord(raw);
  return {
    source: parseAnnouncementSource(d.source),
    affectedSubscriptions: wireInteger(d.affected_subscriptions),
    deliveryReadiness: wireRecordOrNull(d.delivery_readiness),
  };
}

/** A stream of a source as `sources/get` lists it. */
export interface AnnouncementSourceStream {
  readonly stream: string;
  readonly phase: "baselining" | "backfilling" | "live" | (string & {});
  readonly mode: unknown;
  readonly maxItems: number | null;
  readonly backfilled: number;
  readonly sinceMs: number | null;
}

/**
 * `sources/get`: the source (`null` when the mob has none by that id), and
 * its streams when asked for them.
 */
export interface AnnouncementSourceView {
  readonly source: AnnouncementSource | null;
  readonly streams: readonly AnnouncementSourceStream[] | null;
}

export function parseAnnouncementSourceView(raw: unknown): AnnouncementSourceView {
  const d = asRecord(raw);
  const source = wireRecordOrNull(d.source);
  return {
    source: source === null ? null : parseAnnouncementSource(source),
    streams: Array.isArray(d.streams)
      ? asRecordArray(d.streams).map((stream) => ({
          stream: String(stream.stream ?? ""),
          phase: String(stream.phase ?? ""),
          mode: stream.mode ?? null,
          maxItems: wireInteger(stream.max_items),
          backfilled: Number(stream.backfilled ?? 0),
          sinceMs: wireInteger(stream.since_ms),
        }))
      : null,
  };
}

/**
 * What became of one item of a collector step. `sequence` is the
 * announcement's (for a conflict, the original's); `captured` counts the
 * subscriptions that captured it.
 */
export interface SourceItemOutcome {
  readonly itemId: string;
  readonly version: string;
  readonly sourceKey: string;
  readonly outcome: "announced" | "replayed" | "conflict" | (string & {});
  readonly sequence: number | null;
  readonly captured: number | null;
  readonly existingDigest: string | null;
  readonly offeredDigest: string | null;
}

/**
 * `sources/advance`: the source as the step committed it, each item's
 * outcome, and the phases of the streams it onboarded. `replayed` says the
 * step was answered from its receipt.
 */
export interface SourceAdvanceResult {
  readonly source: AnnouncementSource;
  readonly replayed: boolean;
  readonly items: readonly SourceItemOutcome[];
  readonly streams: readonly {
    readonly stream: string;
    readonly phase: "baselining" | "backfilling" | "live" | (string & {});
  }[];
}

export function parseSourceAdvanceResult(raw: unknown): SourceAdvanceResult {
  const d = asRecord(raw);
  return {
    source: parseAnnouncementSource(d.source),
    replayed: d.replayed === true,
    items: asRecordArray(d.items).map((item) => ({
      itemId: String(item.item_id ?? ""),
      version: String(item.version ?? ""),
      sourceKey: String(item.source_key ?? ""),
      outcome: String(item.outcome ?? ""),
      sequence: wireInteger(item.sequence),
      captured: wireInteger(item.captured),
      existingDigest: optionalString(item.existing_digest),
      offeredDigest: optionalString(item.offered_digest),
    })),
    streams: asRecordArray(d.streams).map((stream) => ({
      stream: String(stream.stream ?? ""),
      phase: String(stream.phase ?? ""),
    })),
  };
}

/**
 * One counted audit entry: every refusal or withholding of one member by one
 * generation of one source, for one reason, and every hold, redelivery or
 * dismissal of an item whose turn ended unhandled. `sourceId` and
 * `generation` are `null` for a plain announcement.
 */
export interface AnnouncementAuditEntry {
  readonly id: number;
  readonly atMs: number;
  readonly action:
    | "subscription_refused"
    | "capture_withheld"
    | "items_held"
    | "items_released"
    | "expired_while_held"
    | "delivered_across_revocation"
    | "redelivered"
    | "dismissed"
    | (string & {});
  readonly mobId: string;
  readonly sourceId: string | null;
  readonly generation: number | null;
  readonly member: AnnouncementMember;
  readonly reason:
    | "audience_not_covered"
    | "no_direct_route"
    | "route_overlap"
    | "route_conflict"
    | "unhandled"
    | (string & {});
  readonly count: number;
  readonly firstAtMs: number;
  readonly lastAtMs: number;
}

export function parseAnnouncementAuditEntry(raw: unknown): AnnouncementAuditEntry {
  const d = asRecord(raw);
  return {
    id: Number(d.id ?? 0),
    atMs: Number(d.at_ms ?? 0),
    action: String(d.action ?? ""),
    mobId: String(d.mob_id ?? ""),
    sourceId: optionalString(d.source_id),
    generation: optionalNumber(d.generation),
    member: parseAnnouncementMember(d.member),
    reason: String(d.reason ?? ""),
    count: Number(d.count ?? 0),
    firstAtMs: Number(d.first_at_ms ?? 0),
    lastAtMs: Number(d.last_at_ms ?? 0),
  };
}

/**
 * A durable mob announcement escalation. `subject` is kept as sent
 * (`{kind: "source", mob_id, source_id}`, `{kind: "member", member}`,
 * `{kind: "declaration", record_kind, id}` or newer); `subjectKind` is its
 * kind. `resolvedAtMs` is `null` while it is open.
 */
export interface AnnouncementEscalation {
  readonly escalationId: string;
  readonly cause:
    | "failure_streak"
    | "stale"
    | "quiet"
    | "empty_coverage"
    | "source_gap"
    | "conflicts"
    | "suspended"
    | "member_gap"
    | "audience_hold"
    | "unhandled"
    | "handled_unverified"
    | "declaration_pending"
    | (string & {});
  readonly subject: Readonly<Record<string, unknown>>;
  readonly subjectKind: "source" | "member" | "declaration" | (string & {});
  readonly sinceMs: number;
  readonly raisedAtMs: number;
  readonly renotifyCount: number;
  readonly resolvedAtMs: number | null;
  readonly evidence: string | null;
}

export function parseAnnouncementEscalation(raw: unknown): AnnouncementEscalation {
  const d = asRecord(raw);
  const subject = asRecord(d.subject);
  return {
    escalationId: String(d.escalation_id ?? ""),
    cause: String(d.cause ?? ""),
    subject,
    subjectKind: String(subject.kind ?? ""),
    sinceMs: Number(d.since_ms ?? 0),
    raisedAtMs: Number(d.raised_at_ms ?? 0),
    renotifyCount: Number(d.renotify_count ?? 0),
    resolvedAtMs: wireInteger(d.resolved_at_ms),
    evidence: optionalString(d.evidence),
  };
}

/** One raise, notification or resolution of an escalation, at its event cursor. */
export interface AnnouncementEscalationEvent {
  readonly cursor: number;
  readonly escalationId: string;
  readonly event: "raised" | "renotified" | "resolved" | (string & {});
  readonly cause: string;
  readonly subject: Readonly<Record<string, unknown>>;
  readonly subjectKind: string;
  readonly sinceMs: number;
  readonly atMs: number;
  readonly renotifyCount: number;
}

/**
 * `escalations/list`: the events after a cursor (`events`), or with
 * `open: true` the open escalations (`escalations`); the other is `null`.
 * `next` is the cursor to read on from.
 */
export interface AnnouncementEscalationsPage {
  readonly next: number;
  readonly events: readonly AnnouncementEscalationEvent[] | null;
  readonly escalations: readonly AnnouncementEscalation[] | null;
}

export function parseAnnouncementEscalationsPage(raw: unknown): AnnouncementEscalationsPage {
  const d = asRecord(raw);
  return {
    next: Number(d.next ?? 0),
    events: Array.isArray(d.events)
      ? asRecordArray(d.events).map((event) => {
          const subject = asRecord(event.subject);
          return {
            cursor: Number(event.cursor ?? 0),
            escalationId: String(event.escalation_id ?? ""),
            event: String(event.event ?? ""),
            cause: String(event.cause ?? ""),
            subject,
            subjectKind: String(subject.kind ?? ""),
            sinceMs: Number(event.since_ms ?? 0),
            atMs: Number(event.at_ms ?? 0),
            renotifyCount: Number(event.renotify_count ?? 0),
          };
        })
      : null,
    escalations: Array.isArray(d.escalations) ? asRecordArray(d.escalations).map(parseAnnouncementEscalation) : null,
  };
}

/** What a member is owed: deliverable items, held and withheld counts. */
export interface AnnouncementMemberBacklog {
  readonly eligibleCount: number;
  readonly eligibleBytes: number;
  readonly oldestCapturedAtMs: number | null;
  readonly lastSealedAtMs: number | null;
  readonly outstanding: boolean;
  readonly heldAudience: number;
  readonly heldUnhandled: number;
  readonly heldRefused: number;
  readonly withheldAudience: number;
}

/**
 * What became of a member's admitted batches' turns: batches by handling
 * (`handledUnverified` is never folded into `handled`), the items it holds
 * for an operator now (`held`), and when its oldest unhandled turn was read.
 */
export interface AnnouncementMemberHandling {
  readonly pending: number;
  readonly handled: number;
  readonly handledUnverified: number;
  readonly unhandled: number;
  readonly held: number;
  readonly oldestUnhandledAtMs: number | null;
}

export function parseAnnouncementMemberHandling(raw: unknown): AnnouncementMemberHandling {
  const d = asRecord(raw);
  return {
    pending: Number(d.pending ?? 0),
    handled: Number(d.handled ?? 0),
    handledUnverified: Number(d.handled_unverified ?? 0),
    unhandled: Number(d.unhandled ?? 0),
    held: Number(d.held ?? 0),
    oldestUnhandledAtMs: wireInteger(d.oldest_unhandled_at_ms),
  };
}

/**
 * `members/get` and each `members/list` entry: a member's backlog, why its
 * deliveries wait (`marker`, persisted), its open escalations, and what
 * became of its turns (`handling`, `null` from a gateway that does not report
 * it).
 */
export interface AnnouncementMemberView {
  readonly member: AnnouncementMember;
  readonly backlog: AnnouncementMemberBacklog;
  readonly marker: AnnouncementSuspendMarker | null;
  readonly escalations: readonly AnnouncementEscalation[];
  readonly handling: AnnouncementMemberHandling | null;
}

export function parseAnnouncementMemberView(raw: unknown): AnnouncementMemberView {
  const d = asRecord(raw);
  const backlog = asRecord(d.backlog);
  return {
    member: parseAnnouncementMember(d.member),
    backlog: {
      eligibleCount: Number(backlog.eligible_count ?? 0),
      eligibleBytes: Number(backlog.eligible_bytes ?? 0),
      oldestCapturedAtMs: wireInteger(backlog.oldest_captured_at_ms),
      lastSealedAtMs: wireInteger(backlog.last_sealed_at_ms),
      outstanding: backlog.outstanding === true,
      heldAudience: Number(backlog.held_audience ?? 0),
      heldUnhandled: Number(backlog.held_unhandled ?? 0),
      heldRefused: Number(backlog.held_refused ?? 0),
      withheldAudience: Number(backlog.withheld_audience ?? 0),
    },
    marker: parseOptionalSuspendMarker(d.marker),
    escalations: asRecordArray(d.escalations).map(parseAnnouncementEscalation),
    handling: wireRecordOrNull(d.handling) === null ? null : parseAnnouncementMemberHandling(d.handling),
  };
}

/**
 * One record the applied declarations manage. `refusal` is `{kind, message}`,
 * the refusal a pending record's put met, and `refusalKind` its kind. An
 * applied source carries `collection`; an applied subscription or role child
 * carries `delivery` (`{kind: "ready"}` or `{kind: "refused", cause}`), and
 * `deliveryKind` its kind.
 */
export interface AnnouncementDeclarationRecord {
  readonly kind: "source" | "subscription" | "role_child" | (string & {});
  readonly id: string;
  readonly status: "applied" | "pending" | "left" | "suppressed" | (string & {});
  readonly refusal: Readonly<Record<string, unknown>> | null;
  readonly refusalKind: string | null;
  readonly collection: "baselining" | "backfilling" | "live" | "failing" | "stale" | (string & {}) | null;
  readonly delivery: Readonly<Record<string, unknown>> | null;
  readonly deliveryKind: string | null;
}

export function parseAnnouncementDeclarationRecord(raw: unknown): AnnouncementDeclarationRecord {
  const d = asRecord(raw);
  const refusal = wireRecordOrNull(d.refusal);
  const delivery = wireRecordOrNull(d.delivery);
  return {
    kind: String(d.kind ?? ""),
    id: String(d.id ?? ""),
    status: String(d.status ?? ""),
    refusal,
    refusalKind: refusal === null ? null : optionalString(refusal.kind),
    collection: optionalString(d.collection),
    delivery,
    deliveryKind: delivery === null ? null : optionalString(delivery.kind),
  };
}

/**
 * `declarations/get` and `declarations/apply`: the applied `revision` (0
 * before any), its `digest`, and one record per declared source and
 * subscription and per child of a declared role template.
 */
export interface AnnouncementDeclarationsView {
  readonly revision: number;
  readonly digest: string;
  readonly records: readonly AnnouncementDeclarationRecord[];
}

export function parseAnnouncementDeclarationsView(raw: unknown): AnnouncementDeclarationsView {
  const d = asRecord(raw);
  return {
    revision: Number(d.revision ?? 0),
    digest: String(d.digest ?? ""),
    records: Array.isArray(d.records) ? d.records.map(parseAnnouncementDeclarationRecord) : [],
  };
}

/**
 * `meta/status.mob_announcements`: sources by state (`active`, `paused`,
 * `failing`, `stale`), members (`suspended`, `unhandled_held`), admitted
 * batches by handling (`pending`, `handled`, `handled_unverified`,
 * `unhandled`; empty from a gateway that does not report them), and the open
 * escalations with the last event cursor.
 */
export interface MobAnnouncementsStatus {
  readonly sources: Readonly<Record<string, number>>;
  readonly members: Readonly<Record<string, number>>;
  readonly handling: Readonly<Record<string, number>>;
  readonly openEscalations: number;
  readonly lastEventCursor: number | null;
}

function parseOptionalMobAnnouncementsStatus(raw: unknown): MobAnnouncementsStatus | null {
  const d = wireRecordOrNull(raw);
  if (d === null) return null;
  const escalations = asRecord(d.escalations);
  return {
    sources: wireNumberRecord(d.sources),
    members: wireNumberRecord(d.members),
    handling: wireNumberRecord(d.handling),
    openEscalations: Number(escalations.open ?? 0),
    lastEventCursor: wireInteger(escalations.last_event_cursor),
  };
}

/**
 * `meta/capabilities.mob_announcements`: what this gateway's mob
 * announcements offer. `sources` is `"v1"` when sources are served.
 * `localDelivery` is `{kind: "ready"}`, `{kind: "refused", cause}`,
 * `{kind: "unavailable", detail}` or newer, as sent; `localDeliveryKind` is
 * its kind.
 */
export interface MobAnnouncementsCapability {
  readonly sources: string | null;
  readonly localDelivery: Readonly<Record<string, unknown>> | null;
  readonly localDeliveryKind: "ready" | "refused" | "unavailable" | (string & {}) | null;
  readonly governedAdmission: boolean;
}

function parseOptionalMobAnnouncementsCapability(raw: unknown): MobAnnouncementsCapability | null {
  const d = wireRecordOrNull(raw);
  if (d === null) return null;
  const localDelivery = wireRecordOrNull(d.local_delivery);
  return {
    sources: optionalString(d.sources),
    localDelivery,
    localDeliveryKind: localDelivery === null ? null : optionalString(localDelivery.kind),
    governedAdmission: d.governed_admission === true,
  };
}

/**
 * Whether the gateway serves mob announcement sources: its
 * `featureCapabilities` name `"mob_announcements.sources.v1"`. Takes a
 * {@link CapabilitiesResult} or its `featureCapabilities`. A gateway that
 * lists `meta/mob_announcements/announce` among its methods without it runs
 * mob announcements without sources; an older one answers the source methods
 * with method-not-found.
 */
export function supportsAnnouncementSources(
  capabilities: CapabilitiesResult | readonly string[],
): boolean {
  const features = Array.isArray(capabilities)
    ? (capabilities as readonly string[])
    : (capabilities as CapabilitiesResult).featureCapabilities;
  return features.includes(MOB_ANNOUNCEMENT_SOURCES_V1);
}

/**
 * The `meta/capabilities` feature capability that says `meta/run_flow`
 * accepts `role_targets`.
 */
export const FLOW_ROLE_TARGETS = "flow_role_targets" as const;

/**
 * Whether `meta/run_flow` accepts `role_targets` on this gateway: its
 * `featureCapabilities` name `"flow_role_targets"`. Takes a
 * {@link CapabilitiesResult} or its `featureCapabilities`. A gateway without
 * it ignores the parameter and runs the flow role-wide, so a caller checks
 * this before sending role targets (`MobHandle.runFlow` does).
 */
export function supportsFlowRoleTargets(
  capabilities: CapabilitiesResult | readonly string[],
): boolean {
  const features = Array.isArray(capabilities)
    ? (capabilities as readonly string[])
    : (capabilities as CapabilitiesResult).featureCapabilities;
  return features.includes(FLOW_ROLE_TARGETS);
}

/**
 * The `meta/capabilities` feature capability that says `meta/run_flow`
 * accepts `idempotency_key`.
 */
export const FLOW_RUN_IDEMPOTENCY_KEY = "flow_run_idempotency_key" as const;

/**
 * Whether `meta/run_flow` accepts `idempotency_key` on this gateway: its
 * `featureCapabilities` name `"flow_run_idempotency_key"`. Takes a
 * {@link CapabilitiesResult} or its `featureCapabilities`. A gateway without
 * it ignores the key, so a retry would admit a second run: a caller checks
 * this before sending a key (`MobHandle.runFlow` does).
 */
export function supportsFlowRunIdempotencyKey(
  capabilities: CapabilitiesResult | readonly string[],
): boolean {
  const features = Array.isArray(capabilities)
    ? (capabilities as readonly string[])
    : (capabilities as CapabilitiesResult).featureCapabilities;
  return features.includes(FLOW_RUN_IDEMPOTENCY_KEY);
}
