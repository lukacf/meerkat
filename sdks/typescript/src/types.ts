/**
 * Public domain types for the Meerkat TypeScript SDK.
 *
 * These replace the `Wire*` prefixed generated types.
 */

import type {
  BridgeBootstrapToken as WireBridgeBootstrapToken,
  BridgeCommand as WireBridgeCommand,
  BridgeCommandAuthorizeSupervisor as WireBridgeCommandAuthorizeSupervisor,
  BridgeCommandBindMember as WireBridgeCommandBindMember,
  BridgeCommandDeliverMemberInput as WireBridgeCommandDeliverMemberInput,
  BridgeCommandDestroyMember as WireBridgeCommandDestroyMember,
  BridgeCommandHardCancelMember as WireBridgeCommandHardCancelMember,
  BridgeCommandInterruptMember as WireBridgeCommandInterruptMember,
  BridgeCommandObserveMember as WireBridgeCommandObserveMember,
  BridgeCommandRetireMember as WireBridgeCommandRetireMember,
  BridgeCommandRevokeSupervisor as WireBridgeCommandRevokeSupervisor,
  BridgeCommandUnwireMember as WireBridgeCommandUnwireMember,
  BridgeCommandWireMember as WireBridgeCommandWireMember,
  BridgePeerSpec as WireBridgePeerSpec,
  BridgeProtocolVersion as WireBridgeProtocolVersion,
  CommsCommandRequest as WireCommsCommandRequest,
  CommsPeersResult as WireCommsPeersResult,
  CommsSendResult as WireCommsSendResult,
  WireMobEvent,
  WireMobRun,
  MobBackendConfigInput,
  MobEventRouterConfigInput,
  MobFlowSpecInput,
  MobLimitsSpecInput,
  MobOrchestratorInput,
  MobSkillSourceInput,
  MobSpawnPolicyInput,
  MobSupervisorSpecInput,
  MobToolConfigInput,
  MobTopologySpecInput,
  MobTurnStartParams,
  ScheduleToolCallParams as WireScheduleToolCallParams,
  ScheduleToolCallResult as WireScheduleToolCallResult,
  ScheduleToolDescriptor as WireScheduleToolDescriptor,
  ScheduleToolsResult as WireScheduleToolsResult,
  MobWiringRulesInput,
  WireBudgetSplitPolicy,
  WireAuthBindingRef,
  WireContentInput,
  WireMemberLaunchMode,
  WireMobBackendKind,
  WireMobProfile,
  WireMobRuntimeMode,
  WireRuntimeBinding,
  WireToolAccessPolicy,
  WireToolFilter,
  WireTranscriptReplacement,
} from "./generated/types.js";
import type { ToolResultError, TurnTerminalCauseKind, Usage } from "./events.js";

export type { ToolResultError, TurnTerminalCauseKind, Usage } from "./events.js";

declare const peerIdBrand: unique symbol;
declare const peerCorrelationIdBrand: unique symbol;

/** Canonical comms routing identity for a peer. */
export type PeerId = string & { readonly [peerIdBrand]: "PeerId" };

/** Canonical request/response correlation identity for peer interactions. */
export type PeerCorrelationId = string & {
  readonly [peerCorrelationIdBrand]: "PeerCorrelationId";
};

/** Presentation-only metadata for terminal peer responses. */
export interface PeerResponseTerminalOptions {
  readonly displayName?: string;
}

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
  readonly key?: SkillKey;
  readonly identityHint: string;
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
  readonly terminalCauseKind?: TurnTerminalCauseKind;
  readonly structuredOutput?: unknown;
  readonly extractionError?: ExtractionError;
  readonly schemaWarnings?: readonly SchemaWarning[];
  readonly skillDiagnostics?: SkillRuntimeDiagnostics;
}

export interface ExtractionError {
  readonly lastOutput: string;
  readonly attempts: number;
  readonly reason: string;
}

export type HelpExecutionMode = "explain_only" | "plan_execution";

export interface HelpOptions {
  readonly prompt?: string;
  readonly executionMode?: HelpExecutionMode;
  readonly model?: string;
  readonly provider?: string;
  readonly maxTokens?: number;
}

export interface HelpRequest extends HelpOptions {
  readonly question: string;
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
  readonly resolvedCapabilities?: ResolvedModelCapabilities;
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
  readonly error?: ToolResultError | null;
}

export type ToolCallbackContent = string | readonly ContentBlock[];

export interface ToolCallbackResult {
  readonly content: ToolCallbackContent;
  readonly isError?: boolean;
  readonly is_error?: boolean;
}

export type ToolCallbackReturn = ToolCallbackContent | ToolCallbackResult;

/**
 * Ordered block inside a block-assistant transcript message.
 *
 * `blockType` carries the lane discriminator (`text`, `transcript`,
 * `reasoning`, `tool_use`, `server_tool_content`, `image`, ...). For
 * `transcript` blocks `source` records the originating lane (today
 * `"spoken"`); both `text` and `transcript` blocks expose their rendered
 * string in `text`.
 */
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
  /**
   * Lane provenance for `transcript` blocks (e.g. `"spoken"`). Undefined
   * for non-transcript block types.
   */
  readonly source?: string;
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

/** Behavior for transcript edit requests when the source session has active work. */
export type TranscriptEditRunningBehavior = "reject";

/** Options shared by transcript fork/edit APIs. */
export interface TranscriptEditOptions {
  readonly runningBehavior?: TranscriptEditRunningBehavior;
}

export type TranscriptMessageReplacement = Extract<
  WireTranscriptReplacement,
  { type: "message" }
>;
export type TranscriptUserContentBlockReplacement = Extract<
  WireTranscriptReplacement,
  { type: "user_content_block" }
>;
export type TranscriptAssistantBlockReplacement = Extract<
  WireTranscriptReplacement,
  { type: "assistant_block" }
>;
export type TranscriptToolResultContentBlockReplacement = Extract<
  WireTranscriptReplacement,
  { type: "tool_result_content_block" }
>;

/** Generated transcript replacement contract used by `session/fork_replace`. */
export type TranscriptReplacement = WireTranscriptReplacement;

/** Result of creating a forked transcript branch. */
export interface SessionForkResult {
  readonly sourceSessionId: string;
  readonly sessionId: string;
  readonly sessionRef?: string;
  readonly messageCount: number;
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

export type EventSourceIdentity =
  | { readonly type: "session"; readonly sessionId: string }
  | { readonly type: "runtime"; readonly runtimeId: string }
  | { readonly type: "interaction"; readonly interactionId: string }
  | { readonly type: "callback" }
  | { readonly type: "external"; readonly sourceId: string };

export interface EventEnvelope<T = unknown> {
  readonly timestamp_ms: number;
  readonly source: EventSourceIdentity;
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
export type MobToolConfig = MobToolConfigInput;

export interface MobProfile {
  readonly model: string;
  readonly skills?: readonly string[];
  readonly tools?: MobToolConfig;
  readonly peer_description?: string;
  readonly external_addressable?: boolean;
  readonly backend?: WireMobBackendKind;
  readonly runtime_mode?: WireMobRuntimeMode;
  readonly max_inline_peer_notifications?: number;
  readonly output_schema?: unknown;
  readonly provider_params?: unknown;
}

export type MobProfileBinding = MobProfile | { readonly realm_profile: string };

export interface MobDefinition {
  readonly id: string;
  readonly orchestrator?: MobOrchestratorInput;
  readonly profiles: Readonly<Record<string, MobProfileBinding>>;
  readonly wiring?: MobWiringRulesInput;
  readonly flows?: Readonly<Record<string, MobFlowSpecInput>>;
  readonly skills?: Readonly<Record<string, MobSkillSourceInput>>;
  readonly backend?: MobBackendConfigInput;
  readonly topology?: MobTopologySpecInput;
  readonly supervisor?: MobSupervisorSpecInput;
  readonly limits?: MobLimitsSpecInput;
  readonly spawn_policy?: MobSpawnPolicyInput;
  readonly event_router?: MobEventRouterConfigInput;
}

export interface SpawnManySpec {
  readonly profile: string;
  readonly agentIdentity: string;
  readonly initialMessage?: WireContentInput;
  readonly runtimeMode?: WireMobRuntimeMode;
  readonly backend?: WireMobBackendKind;
  readonly labels?: Record<string, string>;
  readonly context?: unknown;
  readonly additionalInstructions?: string[];
  readonly authBinding?: WireAuthBindingRef;
}

export interface SpawnSpec extends SpawnManySpec {
  readonly binding?: WireRuntimeBinding;
  readonly shellEnv?: Readonly<Record<string, string>>;
  readonly autoWireParent?: boolean;
  readonly launchMode?: WireMemberLaunchMode;
  readonly toolAccessPolicy?: WireToolAccessPolicy;
  readonly budgetSplitPolicy?: WireBudgetSplitPolicy;
  readonly inheritedToolFilter?: WireToolFilter;
  readonly overrideProfile?: WireMobProfile;
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

export type MobLifecycleStatus =
  | "Creating"
  | "Running"
  | "Stopped"
  | "Completed"
  | "Destroyed";

export interface MobSummary {
  readonly mobId: string;
  readonly status: MobLifecycleStatus;
}

export interface MobStatus {
  readonly mobId: string;
  readonly status: MobLifecycleStatus;
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
  readonly run?: WireMobRun | null;
}

type MobTurnStartWireOptions = Omit<
  MobTurnStartParams,
  "mob_id" | "agent_identity" | "prompt"
>;

export interface MobTurnStartOptions {
  readonly skillRefs?: SkillRef[];
  readonly flowToolOverlay?: TurnToolOverlay;
  readonly additionalInstructions?: MobTurnStartWireOptions["additional_instructions"];
  readonly keepAlive?: MobTurnStartWireOptions["keep_alive"];
  readonly model?: MobTurnStartWireOptions["model"];
  readonly provider?: MobTurnStartWireOptions["provider"];
  readonly maxTokens?: MobTurnStartWireOptions["max_tokens"];
  readonly systemPrompt?: MobTurnStartWireOptions["system_prompt"];
  readonly outputSchema?: MobTurnStartWireOptions["output_schema"];
  readonly structuredOutputRetries?: MobTurnStartWireOptions["structured_output_retries"];
  readonly providerParams?: MobTurnStartWireOptions["provider_params"];
  readonly clearProviderParams?: MobTurnStartWireOptions["clear_provider_params"];
  readonly authBinding?: MobTurnStartWireOptions["auth_binding"];
  readonly clearAuthBinding?: MobTurnStartWireOptions["clear_auth_binding"];
}

export interface MobEventsOptions {
  readonly afterCursor?: number;
  readonly limit?: number;
}

export interface MobEventsResult {
  readonly events: readonly WireMobEvent[];
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

export type CommsSendReceipt = WireCommsSendResult;
export type CommsPeersResult = WireCommsPeersResult;
export type CommsCommand = WireCommsCommandRequest;
export type CommsHandlingMode = Extract<
  WireCommsCommandRequest,
  { handling_mode?: unknown }
>["handling_mode"];
export type CommsInputSource = Extract<WireCommsCommandRequest, { kind: "input" }>["source"];
export type CommsInputStreamMode = Extract<
  WireCommsCommandRequest,
  { kind: "input" | "peer_request" }
>["stream"];
export type CommsResponseStatus = Extract<
  WireCommsCommandRequest,
  { kind: "peer_response" }
>["status"];
export type BridgeProtocolVersion = WireBridgeProtocolVersion;
export type BridgeBootstrapToken = WireBridgeBootstrapToken;
export type BridgePeerSpec = WireBridgePeerSpec;
export type BridgeCommand = WireBridgeCommand;
export type BridgeCommandBindMember = WireBridgeCommandBindMember;
export type BridgeCommandAuthorizeSupervisor = WireBridgeCommandAuthorizeSupervisor;
export type BridgeCommandRevokeSupervisor = WireBridgeCommandRevokeSupervisor;
export type BridgeCommandDeliverMemberInput = WireBridgeCommandDeliverMemberInput;
export type BridgeCommandObserveMember = WireBridgeCommandObserveMember;
export type BridgeCommandInterruptMember = WireBridgeCommandInterruptMember;
export type BridgeCommandHardCancelMember = WireBridgeCommandHardCancelMember;
export type BridgeCommandRetireMember = WireBridgeCommandRetireMember;
export type BridgeCommandDestroyMember = WireBridgeCommandDestroyMember;
export type BridgeCommandWireMember = WireBridgeCommandWireMember;
export type BridgeCommandUnwireMember = WireBridgeCommandUnwireMember;
export type CommsChecksumTokenPeerRequestCommand = Extract<
  WireCommsCommandRequest,
  { kind: "peer_request"; intent: "checksum_token" }
>;
export type CommsSupervisorBridgePeerRequestCommand = Extract<
  WireCommsCommandRequest,
  { kind: "peer_request"; intent: "supervisor.bridge" }
>;
export type CommsPeerRequestCommand =
  | CommsChecksumTokenPeerRequestCommand
  | CommsSupervisorBridgePeerRequestCommand;

export type ModelTier = "recommended" | "supported";

export interface ResolvedModelCapabilities {
  readonly vision: boolean;
  readonly imageInput: boolean;
  readonly imageToolResults: boolean;
  readonly inlineVideo: boolean;
  readonly realtime: boolean;
  readonly webSearch: boolean;
  readonly imageGeneration: boolean;
}

export interface ModelProfile {
  readonly modelFamily: string;
  readonly vision: boolean;
  readonly imageInput: boolean;
  readonly imageToolResults: boolean;
  readonly supportsTemperature: boolean;
  readonly supportsThinking: boolean;
  readonly supportsReasoning: boolean;
  readonly inlineVideo: boolean;
  readonly realtime: boolean;
  readonly webSearch: boolean;
  readonly imageGeneration: boolean;
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

export type ScheduleToolDescriptor = WireScheduleToolDescriptor;
export type ScheduleToolsResult = WireScheduleToolsResult;

export interface ScheduleListOptions {
  readonly labels?: Readonly<Record<string, string>>;
  readonly limit?: number;
  readonly offset?: number;
}

export interface ScheduleOccurrencesOptions {
  readonly includeTerminal?: boolean;
}

export type ScheduleToolCallRequest = WireScheduleToolCallParams;
export type ScheduleToolCallResult = WireScheduleToolCallResult;

export type WorkGraphStatus =
  | "open"
  | "in_progress"
  | "blocked"
  | "completed"
  | "cancelled"
  | "failed";

export type WorkGraphPriority = "low" | "medium" | "high";

export type WorkGraphEdgeKind =
  | "blocks"
  | "parent"
  | "related"
  | "supersedes"
  | "derived_from";

export type WorkGraphEventKind =
  | "created"
  | "updated"
  | "claimed"
  | "released"
  | "blocked"
  | "closed"
  | "linked"
  | "evidence_added";

export type WorkGraphOwnerKind = "principal" | "agent" | "session" | "mob" | "label";

export interface WorkGraphOwnerKey {
  readonly kind: WorkGraphOwnerKind;
  readonly id: string;
}

export interface WorkGraphOwner {
  readonly key: WorkGraphOwnerKey;
  readonly displayName?: string;
}

export interface WorkGraphClaim {
  readonly owner: WorkGraphOwner;
  readonly claimedAt: string;
  readonly leaseExpiresAt?: string;
}

export interface ExternalWorkRef {
  readonly kind: string;
  readonly id: string;
  readonly url?: string;
}

export interface WorkEvidenceRef {
  readonly kind: string;
  readonly id: string;
  readonly label?: string;
  readonly summary?: string;
}

export interface WorkItem {
  readonly id: string;
  readonly realmId: string;
  readonly namespace: string;
  readonly title: string;
  readonly description?: string;
  readonly status: WorkGraphStatus;
  readonly priority: WorkGraphPriority;
  readonly labels: readonly string[];
  readonly owner?: WorkGraphOwner;
  readonly claim?: WorkGraphClaim;
  readonly revision: number;
  readonly dueAt?: string;
  readonly notBefore?: string;
  readonly snoozedUntil?: string;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly terminalAt?: string;
  readonly externalRefs: readonly ExternalWorkRef[];
  readonly evidenceRefs: readonly WorkEvidenceRef[];
}

export interface WorkGraphEdge {
  readonly realmId: string;
  readonly namespace: string;
  readonly kind: WorkGraphEdgeKind;
  readonly fromId: string;
  readonly toId: string;
  readonly createdAt: string;
}

export interface WorkGraphEvent {
  readonly seq?: number;
  readonly realmId: string;
  readonly namespace: string;
  readonly itemId?: string;
  readonly kind: WorkGraphEventKind;
  readonly at: string;
  readonly payload?: unknown;
}

export interface WorkItemListResult {
  readonly items: readonly WorkItem[];
}

export interface WorkGraphEventsResult {
  readonly events: readonly WorkGraphEvent[];
}

export interface WorkGraphSnapshot {
  readonly realmId: string;
  readonly namespace?: string;
  readonly allNamespaces: boolean;
  readonly capturedAt: string;
  readonly eventHighWaterMark?: number;
  readonly items: readonly WorkItem[];
  readonly edges: readonly WorkGraphEdge[];
  readonly readyItemIds: readonly string[];
}

export interface WorkGraphItemLookupOptions {
  readonly realmId?: string;
  readonly namespace?: string;
}

export interface WorkGraphItemFilter extends WorkGraphItemLookupOptions {
  readonly allNamespaces?: boolean;
  readonly statuses?: readonly WorkGraphStatus[];
  readonly labels?: readonly string[];
  readonly includeTerminal?: boolean;
  readonly limit?: number;
}

export interface WorkGraphReadyFilter extends WorkGraphItemLookupOptions {
  readonly labels?: readonly string[];
  readonly limit?: number;
}

export interface WorkGraphSnapshotFilter extends WorkGraphItemFilter {}

export interface WorkGraphEventFilter extends WorkGraphItemLookupOptions {
  readonly allNamespaces?: boolean;
  readonly afterSeq?: number;
  readonly limit?: number;
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
  enableSchedule?: boolean;
  enableWorkGraph?: boolean;
  enableMob?: boolean;
  enableWebSearch?: boolean;
  toolFilter?: WireToolFilter;
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
  readonly source?: EventSourceIdentity;
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
