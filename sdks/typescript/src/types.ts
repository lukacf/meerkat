/**
 * Public domain types for the Meerkat TypeScript SDK.
 *
 * These replace the `Wire*` prefixed generated types.
 */

import type { Usage } from "./events.js";

export type { Usage } from "./events.js";

/** Warning emitted when structured output doesn't match a provider's schema. */
export interface SchemaWarning {
  readonly provider: string;
  readonly path: string;
  readonly message: string;
}

/** Runtime health snapshot for skill source resolution. */
export interface SourceHealthSnapshot {
  readonly state: string;
  readonly invalidRatio: number;
  readonly invalidCount: number;
  readonly totalCount: number;
  readonly failureStreak: number;
  readonly handshakeFailed: boolean;
}

/** Diagnostic details for a single quarantined skill entry. */
export interface SkillQuarantineDiagnostic {
  readonly sourceUuid: string;
  readonly skillId: string;
  readonly location: string;
  readonly errorCode: string;
  readonly errorClass: string;
  readonly message: string;
  readonly firstSeenUnixSecs: number;
  readonly lastSeenUnixSecs: number;
}

/** Runtime diagnostics emitted by the Rust skill subsystem. */
export interface SkillRuntimeDiagnostics {
  readonly sourceHealth: SourceHealthSnapshot;
  readonly quarantined: readonly SkillQuarantineDiagnostic[];
}

/** Structured skill identifier (source UUID + skill name). */
export interface SkillKey {
  readonly sourceUuid: string;
  readonly skillName: string;
}

/** A structured skill reference. */
export type SkillRef = SkillKey;

/** Inline image content accepted by input-bearing APIs. */
export interface InlineImageBlock {
  readonly type: "image";
  readonly media_type: string;
  readonly source?: "inline";
  readonly data: string;
}

/** Blob-backed image content accepted by input-bearing APIs and emitted by history surfaces. */
export interface BlobImageBlock {
  readonly type: "image";
  readonly media_type: string;
  readonly source: "blob";
  readonly blob_id: string;
}

/** Inline video content accepted by input-bearing APIs. */
export interface InlineVideoBlock {
  readonly type: "video";
  readonly media_type: string;
  readonly duration_ms: number;
  readonly source?: "inline";
  readonly data: string;
}

/** A content block in a multimodal prompt. */
export type ContentBlock =
  | { type: "text"; text: string }
  | InlineImageBlock
  | BlobImageBlock
  | InlineVideoBlock;

/** Canonical content input returned by history surfaces and accepted by input-bearing APIs. */
export type ContentInput = string | readonly ContentBlock[];

/** Raw blob bytes fetched by blob id. */
export interface BlobPayload {
  readonly blobId: string;
  readonly mediaType: string;
  readonly dataBase64: string;
}

/** Ephemeral per-turn tool visibility overlay. */
export interface TurnToolOverlay {
  readonly allowedTools?: readonly string[];
  readonly blockedTools?: readonly string[];
}

/** Result of an agent session creation or turn. */
export interface RunResult {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly text: string;
  readonly turns: number;
  readonly toolCalls: number;
  readonly usage: Usage;
  readonly structuredOutput?: unknown;
  readonly schemaWarnings?: readonly SchemaWarning[];
  readonly skillDiagnostics?: SkillRuntimeDiagnostics;
}

/** Session metadata shape used by `listSessions()` and `readSession()`. */
export interface SessionInfo {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly createdAt: number;
  readonly updatedAt: number;
  readonly messageCount: number;
  readonly totalTokens?: number;
  readonly isActive: boolean;
  readonly model?: string;
  readonly provider?: string;
  readonly lastAssistantText?: string;
  readonly labels: Readonly<Record<string, string>>;
}

export interface SessionToolCall {
  readonly id: string;
  readonly name: string;
  readonly args: unknown;
}

export interface SessionToolResult {
  readonly toolUseId: string;
  readonly content: ContentInput;
  readonly isError: boolean;
}

export interface SessionAssistantBlock {
  readonly blockType: string;
  readonly text?: string;
  readonly id?: string;
  readonly name?: string;
  readonly args?: unknown;
  readonly imageId?: string;
  readonly blobId?: string;
  readonly mediaType?: string;
  readonly width?: number;
  readonly height?: number;
  readonly revisedPrompt?: Record<string, unknown>;
  readonly meta?: Record<string, unknown>;
}

export interface SessionMessage {
  readonly role: string;
  readonly createdAt: string;
  readonly content?: ContentInput;
  readonly toolCalls: readonly SessionToolCall[];
  readonly stopReason?: string;
  readonly blocks: readonly SessionAssistantBlock[];
  readonly results: readonly SessionToolResult[];
}

export interface SessionHistory {
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly messageCount: number;
  readonly offset: number;
  readonly limit?: number;
  readonly hasMore: boolean;
  readonly messages: readonly SessionMessage[];
}

/** Session listing filters for `session/list`. */
export interface SessionListOptions {
  readonly labels?: Readonly<Record<string, string>>;
  readonly limit?: number;
  readonly offset?: number;
}

/** Shared source/idempotency options used by runtime context/event APIs. */
export interface SessionIngressOptions {
  readonly source?: string;
  readonly idempotencyKey?: string;
}

/** Per-turn options for normal turns and deferred first turns. */
export interface TurnOptions {
  readonly skillRefs?: SkillRef[];
  readonly flowToolOverlay?: TurnToolOverlay;
  readonly additionalInstructions?: string[];
  readonly keepAlive?: boolean;
  readonly model?: string;
  readonly provider?: string;
  readonly maxTokens?: number;
  readonly systemPrompt?: string;
  readonly outputSchema?: Record<string, unknown>;
  readonly structuredOutputRetries?: number;
  readonly providerParams?: Record<string, unknown>;
}

export interface EventEnvelope<T = unknown> {
  readonly timestamp_ms: number;
  readonly source_id: string;
  readonly seq: number;
  readonly event_id: string;
  readonly payload: T;
}

export interface AttributedEvent {
  readonly source: string;
  readonly sourceFenceToken?: number;
  readonly role: string;
  readonly envelope: EventEnvelope;
}

/** Public mob definition input for host-side `mob/create`. */
export interface MobToolConfig {
  readonly builtins?: boolean;
  readonly shell?: boolean;
  readonly comms?: boolean;
  readonly memory?: boolean;
  readonly mob?: boolean;
  readonly mob_tasks?: boolean;
  readonly schedule?: boolean;
  readonly mcp?: readonly string[];
}

export interface MobProfile {
  readonly model: string;
  readonly skills?: readonly string[];
  readonly tools?: MobToolConfig;
  readonly peer_description?: string;
  readonly external_addressable?: boolean;
  readonly backend?: "session" | "external";
  readonly runtime_mode?: "autonomous_host" | "turn_driven";
  readonly max_inline_peer_notifications?: number;
  readonly output_schema?: Record<string, unknown>;
  readonly provider_params?: Record<string, unknown>;
}

export type MobProfileBinding = MobProfile | { readonly realm_profile: string };

export interface MobDefinition {
  readonly id: string;
  readonly orchestrator?: { readonly profile: string };
  readonly profiles: Readonly<Record<string, MobProfileBinding>>;
  readonly mcp_servers?: Readonly<Record<string, Record<string, unknown>>>;
  readonly wiring?: Record<string, unknown>;
  readonly flows?: Record<string, unknown>;
  readonly skills?: Record<string, unknown>;
  readonly backend?: Record<string, unknown>;
  readonly topology?: Record<string, unknown>;
  readonly supervisor?: Record<string, unknown>;
  readonly limits?: Record<string, unknown>;
  readonly spawn_policy?: Record<string, unknown>;
  readonly event_router?: Record<string, unknown>;
}

export interface SpawnSpec {
  readonly profile: string;
  readonly agentIdentity: string;
  readonly initialMessage?: string | ContentBlock[];
  readonly runtimeMode?: "turn_driven" | "autonomous_host";
  readonly backend?: "session" | "external";
  readonly labels?: Record<string, string>;
  readonly context?: Record<string, unknown>;
  readonly generation?: number;
  readonly additionalInstructions?: string[];
}

export interface MobSpawnResult {
  readonly mobId: string;
  readonly agentIdentity: string;
  readonly memberRef: MobMemberRef;
}

export interface MobMember {
  readonly agentIdentity: string;
  readonly memberRef: MobMemberRef;
  readonly profile: string;
  readonly peerId?: string;
  readonly externalPeerSpecs?: Readonly<Record<string, Record<string, unknown>>>;
  readonly runtimeMode?: string;
  readonly state?: string;
  readonly wiredTo?: readonly string[];
  readonly labels?: Record<string, string>;
  readonly status?: string;
  readonly error?: string;
  readonly isFinal?: boolean;
}

export interface MobSummary {
  readonly mobId: string;
  readonly status: string;
}

export interface MobStatus {
  readonly mobId: string;
  readonly status: string;
}

export type MobLifecycleAction = "stop" | "resume" | "complete" | "destroy" | "reset";

/**
 * Server-resolved opaque handle for a mob member. Treat as an opaque token:
 * app code never constructs or inspects these — they come back from
 * `mob/ensure_member`, `mob/spawn_helper`, `mob/fork_helper`, and member
 * list surfaces, and are passed back on work-lane and member-targeted
 * calls.
 */
export type MobMemberRef = string;

export interface MobFlowStatus {
  readonly run?: Record<string, unknown> | null;
}

export interface MobSpawnManyResultEntry {
  readonly ok: boolean;
  readonly agentIdentity?: string;
  readonly memberRef?: MobMemberRef;
  readonly error?: string;
}

export interface MobEventsOptions {
  readonly afterCursor?: number;
  readonly limit?: number;
}

export interface MobEventsResult {
  readonly events: readonly Record<string, unknown>[];
}

export interface MobStoredProfile {
  readonly name: string;
  readonly profile: MobProfile;
  readonly revision: number;
  readonly createdAt: string;
  readonly updatedAt: string;
}

export interface MobProfileLookupResult {
  readonly notFound: boolean;
  readonly name: string;
  readonly profile?: MobProfile;
  readonly revision?: number;
  readonly createdAt?: string;
  readonly updatedAt?: string;
}

export interface MobProfileDeleteResult {
  readonly name: string;
  readonly deletedRevision: number;
}

export interface Capability {
  readonly id: string;
  readonly description: string;
  readonly status: string;
}

export interface ConfigEnvelope {
  readonly config: Record<string, unknown>;
  readonly generation: number;
  readonly realmId?: string;
  readonly instanceId?: string;
  readonly backend?: string;
  readonly resolvedPaths?: Readonly<Record<string, string>>;
}

export interface CommsSendReceipt extends Record<string, unknown> {
  readonly kind?: string;
  readonly requestId?: string;
  readonly interactionId?: string;
  readonly inputId?: string;
}

// ---------------------------------------------------------------------------
// comms/send typed command surface.
//
// Mirrors the Rust `CommsCommandRequest` enum (`meerkat-core/src/comms.rs`).
// Invalid discriminator values are rejected at the server's typed-serde
// boundary — these aliases document the closed-world shape for callers.
// ---------------------------------------------------------------------------

export type CommsHandlingMode = "queue" | "steer";
export type CommsInputSource = "tcp" | "uds" | "stdin" | "webhook" | "rpc";
export type CommsInputStreamMode = "none" | "reserve_interaction";
export type CommsResponseStatus = "accepted" | "completed" | "failed";

export interface CommsInputCommand {
  kind: "input";
  body: string;
  source?: CommsInputSource;
  stream?: CommsInputStreamMode;
  handling_mode?: CommsHandlingMode;
  allow_self_session?: boolean;
}

export interface CommsPeerMessageCommand {
  kind: "peer_message";
  to: string;
  body: string;
  handling_mode?: CommsHandlingMode;
}

export interface CommsPeerLifecycleCommand {
  kind: "peer_lifecycle";
  to: string;
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired";
  params?: unknown;
}

export interface CommsPeerRequestCommand {
  kind: "peer_request";
  to: string;
  intent: string;
  params?: Record<string, unknown>;
  handling_mode?: CommsHandlingMode;
  stream?: CommsInputStreamMode;
}

export interface CommsPeerResponseCommand {
  kind: "peer_response";
  to: string;
  in_reply_to: string;
  status: CommsResponseStatus;
  result?: unknown;
  handling_mode?: CommsHandlingMode;
}

/** Typed comms/send command — serde-tagged on `kind`. */
export type CommsCommand =
  | CommsInputCommand
  | CommsPeerMessageCommand
  | CommsPeerLifecycleCommand
  | CommsPeerRequestCommand
  | CommsPeerResponseCommand;

export type ModelTier = "recommended" | "supported";

export interface ModelProfile {
  readonly modelFamily: string;
  readonly supportsTemperature: boolean;
  readonly supportsThinking: boolean;
  readonly supportsReasoning: boolean;
  readonly inlineVideo: boolean;
  readonly paramsSchema: unknown;
}

export interface CatalogModel {
  readonly id: string;
  readonly displayName: string;
  readonly tier: ModelTier;
  readonly contextWindow?: number;
  readonly maxOutputTokens?: number;
  readonly serverId?: string;
  readonly profile?: ModelProfile;
}

export interface ProviderModelCatalog {
  readonly provider: string;
  readonly defaultModelId: string;
  readonly models: readonly CatalogModel[];
}

export interface ContractVersion {
  readonly major: number;
  readonly minor: number;
  readonly patch: number;
}

export interface ModelsCatalog {
  readonly contractVersion: ContractVersion;
  readonly providers: readonly ProviderModelCatalog[];
}

export interface Schedule {
  readonly scheduleId: string;
  readonly phase: string;
  readonly revision: number;
  readonly name?: string;
  readonly description?: string;
  readonly trigger: Record<string, unknown>;
  readonly target: Record<string, unknown>;
  readonly misfirePolicy?: Record<string, unknown> | string;
  readonly overlapPolicy?: string;
  readonly missingTargetPolicy?: string;
  readonly planningHorizonDays?: number;
  readonly planningHorizonOccurrences?: number;
  readonly nextOccurrenceOrdinal?: number;
  readonly planningCursorUtc?: string;
  readonly createdAtUtc?: string;
  readonly updatedAtUtc?: string;
  readonly deletedAtUtc?: string;
  readonly labels: Readonly<Record<string, string>>;
}

export interface ScheduleOccurrence {
  readonly occurrenceId: string;
  readonly scheduleId: string;
  readonly scheduleRevision: number;
  readonly occurrenceOrdinal: number;
  readonly phase: string;
  readonly dueAtUtc: string;
  readonly triggerSnapshot: Record<string, unknown>;
  readonly targetSnapshot: Record<string, unknown>;
  readonly misfirePolicy?: Record<string, unknown> | string;
  readonly overlapPolicy?: string;
  readonly missingTargetPolicy?: string;
  readonly claimedBy?: string;
  readonly leaseExpiresAtUtc?: string;
  readonly deliveryCorrelationId?: string;
  readonly lastReceipt?: Record<string, unknown>;
  readonly failureClass?: string;
  readonly failureDetail?: string;
  readonly attemptCount?: number;
  readonly createdAtUtc?: string;
  readonly claimedAtUtc?: string;
  readonly dispatchedAtUtc?: string;
  readonly completedAtUtc?: string;
  readonly supersededByRevision?: number;
}

export interface CreateScheduleRequest {
  readonly name?: string;
  readonly description?: string;
  readonly trigger: Record<string, unknown>;
  readonly target: Record<string, unknown>;
  readonly misfirePolicy?: Record<string, unknown> | string;
  readonly overlapPolicy?: string;
  readonly missingTargetPolicy?: string;
  readonly labels?: Readonly<Record<string, string>>;
  readonly planningHorizonDays?: number;
  readonly planningHorizonOccurrences?: number;
}

export interface UpdateSchedulePatch {
  readonly expectedRevision?: number;
  readonly name?: string;
  readonly description?: string;
  readonly trigger?: Record<string, unknown>;
  readonly target?: Record<string, unknown>;
  readonly misfirePolicy?: Record<string, unknown> | string;
  readonly overlapPolicy?: string;
  readonly missingTargetPolicy?: string;
  readonly planningHorizonDays?: number;
  readonly planningHorizonOccurrences?: number;
  readonly labels?: Readonly<Record<string, string>>;
}

export interface UpdateScheduleRequest {
  readonly scheduleId: string;
  readonly update: UpdateSchedulePatch;
}

export interface ScheduleOccurrencesResult {
  readonly occurrences: readonly ScheduleOccurrence[];
}

export interface ScheduleToolsResult {
  readonly tools: readonly Record<string, unknown>[];
}

export interface ScheduleListOptions {
  readonly labels?: Readonly<Record<string, string>>;
  readonly limit?: number;
  readonly offset?: number;
}

export interface ScheduleOccurrencesOptions {
  readonly includeTerminal?: boolean;
}

export interface ScheduleToolCallRequest {
  readonly name: string;
  readonly arguments?: unknown;
}

/** Options for creating a new session. */
export interface SessionOptions {
  model?: string;
  provider?: string;
  systemPrompt?: string;
  maxTokens?: number;
  outputSchema?: Record<string, unknown>;
  structuredOutputRetries?: number;
  hooksOverride?: Record<string, unknown>;
  enableBuiltins?: boolean;
  enableShell?: boolean;
  enableMemory?: boolean;
  enableMob?: boolean;
  keepAlive?: boolean;
  commsName?: string;
  peerMeta?: Record<string, unknown>;
  budgetLimits?: Record<string, unknown>;
  providerParams?: Record<string, unknown>;
  preloadSkills?: SkillRef[];
  skillRefs?: SkillRef[];
  labels?: Readonly<Record<string, string>>;
  additionalInstructions?: readonly string[];
  appContext?: unknown;
  shellEnv?: Readonly<Record<string, string>>;
  externalTools?: readonly Record<string, unknown>[];
}


/** Explicit standalone session-event envelope. */
export interface AgentEventEnvelope {
  readonly eventId?: string;
  readonly sourceId?: string;
  readonly seq?: number;
  readonly timestampMs?: number;
  readonly payload?: import("./events.js").AgentEvent;
}

/** Mob-wide attributed event emitted by member/mob observation streams. */
export interface AttributedMobEvent {
  readonly source: string;
  readonly profile: string;
  readonly envelope: AgentEventEnvelope;
}

/** Options for creating a mob through the RPC-backed SDK surface. */
export interface MobCreateOptions {
  definition: MobDefinition;
}
