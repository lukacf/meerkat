import { MeerkatClient, Mob } from "../src/index.js";
import type {
  AgentErrorReport,
  BridgeCommandDeclareMemberOutboundTaint,
  BridgeCommandDeliverMemberInput,
  BridgeCommandHardCancelMember,
  BridgeCommandCancelTrackedMemberInput,
  BridgeCommandObserveSupervisorRotation,
  BridgeHostCapabilityRequirements,
  BridgeCommandOpenMemberLiveChannel,
  BridgeCommandPollMemberEvents,
  BridgeCommandUnwireMember,
  BridgeCommandWireMember,
  BridgeDeliveryPayload,
  BridgeDeliveryRejectionCauseStaleMemberIncarnation,
  BridgeDeliveryRejectionCauseStaleMemberResidency,
  BridgeEventCursor,
  BridgeMemberIncarnation,
  BridgeMobPeerOverlayHandoff,
  BridgeOutboundTaintTarget,
  BridgePeerWiringPayload,
  BridgeTurnDirective,
  BridgeTurnOutcomeAck,
  BridgeTurnOutcomeRecord,
  BridgeTrackedInputCancelOutcome,
  CommsCommand,
  ContentBlock,
  ContentInput,
  MobControlScope,
  MobCreateOptions,
  MobDefinition,
  MobGrantRecord,
  MobGrantScopesParams,
  MobGrantScopesResult,
  MobGrantsResult,
  MobHardCancelParams,
  MobHardCancelResult,
  MobRevokeScopesParams,
  MobRevokeScopesResult,
  MobRotateSupervisorResult,
  MobTurnStartOptions,
  MobWireMembersBatchEdgeInput,
  MobWireMembersBatchResult,
  LiveOpenTransport,
  OperationId,
  PeerCorrelationId,
  PeerId,
  RunId,
  RunFailedEvent,
  SessionContentBlock,
  SessionContentInput,
  SessionOptions,
  SpawnManySpec,
  SpawnSpec,
  SupervisorRotationReportWire,
  ToolCallRequestedEvent,
  TranscriptForkOptions,
  TranscriptRewriteOptions,
  WireControlScope,
  WireFlowFailureDetail,
  WireFlowTurnOutcome,
  WireGrantRecord,
} from "../src/index.js";
import type {
  MobSpawnParams as PublicMobSpawnParams,
  MobSpawnSpecParams as PublicMobSpawnSpecParams,
} from "../src/index.js";
import type {
  BridgeLiveControlOutcome,
  BridgeLiveControlOutcomeCommitInput,
  BridgeLiveControlOutcomeInterrupt,
  BridgeLiveControlOutcomeRefresh,
  BridgeLiveControlOutcomeTruncate,
  BridgeLiveControlVerb,
  BridgeLiveControlVerbCommitInput,
  BridgeLiveControlVerbInterrupt,
  BridgeLiveControlVerbRefresh,
  BridgeLiveControlVerbTruncate,
  LiveCloseResult,
  LiveInputChunkWire,
  LiveOpenResult,
  LiveSendInputParams,
  LiveStatusResult,
  MultiHostErrorCode,
  MobBindHostParams,
  MobBindHostResult,
  MobHostStatus,
  MobHostsResult,
  MobMemberHistoryParams,
  MobMemberHistoryResult,
  MobMemberLiveChannelParams,
  MobMemberLiveControlParams,
  MobMemberLiveOpenOptions,
  MobMemberLiveOpenParams,
  MobMemberLiveStatusParams,
  MobRevokeHostParams,
  MobRevokeHostResult,
  MobRouteInstallsResult,
  LiveSendInputErrorData,
  RealtimeImageChunk,
  RealtimeInputChunk,
  RealtimeInputKind,
  RealtimeTranscriptEvent,
  RealtimeTranscriptEventItemObserved,
  RealtimeTurningMode,
  WireAssistantBlock,
  WireHistoryRow,
  WireHostUnavailableDetail,
  WireHostBindPhase,
  WireHostBindingDescriptor,
  WireHostBindingDescriptorKind,
  WireHostCapabilityFlags,
  WireHostRef,
  WireLiveAdapterErrorCode,
  WireLiveAdapterObservation,
  WireLiveAdapterObservationAssistantAudioChunk,
  WireLiveAdapterObservationCommandRejected,
  WireLiveAdapterObservationUserContentCommitted,
  WireLiveConfigRejectionReason,
  WireMemberHistoryPageBody,
  WireMemberLifecycleCapabilities,
  WireMemberProgressSnapshot,
  WireNonPortableResourceKind,
  WireProjectionProvenance,
  WireReachability,
  WireRouteInstallObligation,
  WireScopeDeniedDetail,
  WireSessionMessage,
  WireStaleCursorDetail,
  WireStaleFenceDetail,
  WireTrustedPeerIdentity,
  WireTrustedPeerIdentityEd25519PublicKey,
  WireLiveConfigRejectionReasonImageInputInvalidBase64,
  WireLiveConfigRejectionReasonImageInputIdempotencyConflict,
  WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid,
  WireLiveConfigRejectionReasonImageInputRequiresCommit,
  WireLiveConfigRejectionReasonInputBackpressured,
  WireLiveConfigRejectionReasonInputTooLarge,
  WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen,
} from "../src/index.js";

const publicMemberProgress: WireMemberProgressSnapshot = {
  run_state: "run_open",
  in_flight_work: 1,
  last_progress_at_ms: 1,
  last_progress_event: "execution_advanced",
  health: "healthy",
};
void publicMemberProgress;
import type {
  BridgeRejectionCauseMaterializeBuildRejectedPayload,
  BridgeRejectionCauseScopeDeniedPayload,
  DeferredCatalogDelta,
  MemberBuildRejection,
  MobCreateParams,
  MobDefinitionInput,
  MobEnsureMemberParams,
  MobEnsureMemberResult,
  MobMemberListEntryWire,
  MobMemberSpecWire,
  MobMembersResult,
  MobMemberStatusResult,
  MobReconcileParams,
  MobSpawnManyParams,
  MobSpawnManyFailedResult,
  MobSpawnManyFailureCause,
  MobSpawnManyResult,
  MobSpawnManyResultEntry,
  MobSpawnManyResultPayload,
  MobSpawnManyResultStatus,
  MobSpawnManySpawnedResult,
  MobSpawnParams,
  MobSpawnReceiptWire,
  MobSpawnResult,
  MobSpawnSpecParams,
  MobSubmitWorkParams,
  MobTurnStartParams,
  MobWireMembersBatchParams,
  MobWireMembersBatchResult as WireMobWireMembersBatchResult,
  RuntimeHostCapabilities,
  RuntimeHostFeatureFlags,
  ContractVersion as GeneratedContractVersion,
  SystemNoticeBlockComms,
  SystemNoticeBlockToolConfig,
  SystemNoticePeer,
  ToolConfigChangedPayload,
  ToolConfigChangeStatus,
  WireAuthBindingRef,
  WirePeerConnectivityKnown,
  WirePeerConnectivitySnapshot,
  WireUnreachablePeer,
  WireMemberLaunchMode,
  WireMemberRef,
  WireMobBackendKind,
  WireMobProfile,
  WireMobMemberStatus,
  WireMobRuntimeMode,
  WireMobToolConfig,
  WireToolAccessPolicy,
  WireToolFilter,
} from "../src/generated/types.js";

const runtimeHostFeatureFlags: RuntimeHostFeatureFlags = {
  runtime_backed_sessions: true,
  mobs: true,
  mcp_live: true,
  comms: true,
  blobs: true,
  session_events: true,
  session_streams: true,
  schedules: true,
  skills: true,
  event_replay: true,
  artifacts: true,
  approvals: true,
  external_members: true,
  secure_remote_rpc: true,
  multi_host_mobs: true,
};
const generatedContractVersion: GeneratedContractVersion = {
  major: 0,
  minor: 7,
  patch: 29,
};
const runtimeHostCapabilities: RuntimeHostCapabilities = {
  contract_version: generatedContractVersion,
  features: runtimeHostFeatureFlags,
};
void runtimeHostCapabilities;

// These multi-host and bridge contracts are part of the package-root type
// surface. Consumers must not import implementation details from
// `src/generated/types` to construct grant/cancel commands or inspect typed
// terminal outcomes.
const publicGrantScopes: MobGrantScopesParams = {
  mob_id: "mob-1",
  principal: "operator",
  scopes: ["cancel"],
};
const publicWireGrant: WireGrantRecord = {
  principal: "operator",
  scopes: ["cancel"],
};
const publicGrantResult: MobGrantScopesResult = { record: publicWireGrant };
const publicGrantsResult: MobGrantsResult = { grants: [publicWireGrant] };
const publicRevokeScopes: MobRevokeScopesParams = {
  mob_id: "mob-1",
  principal: "operator",
  scopes: ["cancel"],
};
const publicRevokeResult: MobRevokeScopesResult = { removed: true };
const publicHardCancel: MobHardCancelParams = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  reason: "operator request",
};
const publicHardCancelResult: MobHardCancelResult = { cancelled: true };
const publicScope: WireControlScope = "cancel";
const publicHostRequirements: BridgeHostCapabilityRequirements = {
  autonomous_members: true,
  durable_sessions: true,
  protocol_v4: true,
  tracked_input_cancel: true,
};
const publicFailureDetail: WireFlowFailureDetail = {
  original_utf8_bytes: 7,
  text: "failure",
  truncated: false,
};
const publicFlowOutcome: WireFlowTurnOutcome = {
  run_failed: { detail: publicFailureDetail },
};
const publicTurnOutcome: BridgeTurnOutcomeRecord = {
  fence_token: 3,
  generation: 2,
  input_id: "input-1",
  outcome: publicFlowOutcome,
  terminal_seq: 9,
};
const publicCancelOutcome: BridgeTrackedInputCancelOutcome = {
  outcome: "terminal",
  record: publicTurnOutcome,
};
declare const publicCancelTrackedCommand: BridgeCommandCancelTrackedMemberInput;
declare const publicObserveRotationCommand: BridgeCommandObserveSupervisorRotation;
const publicCancelTrackedTag: "cancel_tracked_member_input" =
  publicCancelTrackedCommand.command;
const publicObserveRotationTag: "observe_supervisor_rotation" =
  publicObserveRotationCommand.command;
void [
  publicGrantScopes,
  publicGrantResult,
  publicGrantsResult,
  publicRevokeScopes,
  publicRevokeResult,
  publicHardCancel,
  publicHardCancelResult,
  publicScope,
  publicHostRequirements,
  publicCancelOutcome,
  publicCancelTrackedTag,
  publicObserveRotationTag,
];

// Nested public mob contracts must retain their named generated types. A
// schema-local `$defs` regression previously widened these fields to opaque
// Record<string, unknown> values while still letting codegen freshness pass.
declare const generatedMobHistory: MobMemberHistoryResult;
declare const generatedMobHosts: MobHostsResult;
declare const generatedMobBind: MobBindHostResult;
declare const generatedRouteInstalls: MobRouteInstallsResult;
declare const generatedGrant: MobGrantScopesResult;
declare const generatedGrants: MobGrantsResult;

const generatedHistoryPage: WireMemberHistoryPageBody = generatedMobHistory.page;
const generatedHistoryRows: WireHistoryRow[] = generatedHistoryPage.messages;
declare const generatedConnectivity: WirePeerConnectivityKnown;
const generatedConnectivitySnapshot: WirePeerConnectivitySnapshot =
  generatedConnectivity.snapshot;
const generatedUnreachablePeers: WireUnreachablePeer[] =
  generatedConnectivitySnapshot.unreachable_peers ?? [];
const generatedHosts: MobHostStatus[] = generatedMobHosts.hosts;
const generatedHostCapabilities: WireHostCapabilityFlags = generatedMobBind.capabilities;
const generatedRouteObligations: WireRouteInstallObligation[] =
  generatedRouteInstalls.outstanding;
const generatedGrantRecord: WireGrantRecord = generatedGrant.record;
const generatedGrantRecords: readonly WireGrantRecord[] = generatedGrants.grants;

void generatedHistoryPage;
void generatedHistoryRows;
void generatedConnectivitySnapshot;
void generatedUnreachablePeers;
void generatedHosts;
void generatedHostCapabilities;
void generatedRouteObligations;
void generatedGrantRecord;
void generatedGrantRecords;

// The public supervisor-bridge contracts are emitted from schema-local refs.
// Keep every newly exposed nested field named and typed instead of allowing
// codegen to widen it to unknown/Record<string, unknown>.
declare const generatedBridgeDelivery: BridgeDeliveryPayload;
declare const generatedBridgeOutboundTaint: BridgeCommandDeclareMemberOutboundTaint;
declare const generatedBridgeDeliverCommand: BridgeCommandDeliverMemberInput;
declare const generatedBridgeHardCancel: BridgeCommandHardCancelMember;
declare const generatedBridgePoll: BridgeCommandPollMemberEvents;
declare const generatedBridgeLiveOpen: BridgeCommandOpenMemberLiveChannel;
declare const generatedBridgeWire: BridgeCommandWireMember;
declare const generatedBridgeUnwire: BridgeCommandUnwireMember;
declare const generatedBridgePeerWiring: BridgePeerWiringPayload;
declare const generatedStaleMemberRejection: BridgeDeliveryRejectionCauseStaleMemberIncarnation;
declare const generatedMaterializeRejection: BridgeRejectionCauseMaterializeBuildRejectedPayload;
declare const generatedScopeRejection: BridgeRejectionCauseScopeDeniedPayload;

const generatedDeliveryExpectedMember: BridgeMemberIncarnation | null | undefined =
  generatedBridgeDelivery.expected_member;
const generatedOutboundTaintTarget: BridgeOutboundTaintTarget | null | undefined =
  generatedBridgeOutboundTaint.target;
const generatedDeliveryTurn: BridgeTurnDirective | null | undefined =
  generatedBridgeDelivery.turn;
const generatedCommandExpectedMember: BridgeMemberIncarnation | null | undefined =
  generatedBridgeDeliverCommand.expected_member;
const generatedCommandObjective: string | null | undefined =
  generatedBridgeDeliverCommand.objective_id;
const generatedCommandTurn: BridgeTurnDirective | null | undefined =
  generatedBridgeDeliverCommand.turn;
const generatedInjectedContext: readonly ContentInput[] | undefined =
  generatedBridgeDeliverCommand.injected_context;
const generatedExpectedRunId: RunId = generatedBridgeHardCancel.expected_run_id;
const generatedOperationId: OperationId = generatedBridgeHardCancel.operation_id;
const generatedCursor: BridgeEventCursor = generatedBridgePoll.cursor;
const generatedOutcomeAcks: readonly BridgeTurnOutcomeAck[] | undefined =
  generatedBridgePoll.outcome_acks;
const generatedLiveTransport: LiveOpenTransport | null | undefined =
  generatedBridgeLiveOpen.transport;
const generatedCommandOverlay: BridgeMobPeerOverlayHandoff | null | undefined =
  generatedBridgeWire.mob_peer_overlay;
const generatedUnwireOverlay: BridgeMobPeerOverlayHandoff | null | undefined =
  generatedBridgeUnwire.mob_peer_overlay;
const generatedPayloadOverlay: BridgeMobPeerOverlayHandoff | null | undefined =
  generatedBridgePeerWiring.mob_peer_overlay;
const generatedCurrentMember: BridgeMemberIncarnation =
  generatedStaleMemberRejection.current;
declare const generatedResidencyMember: BridgeMemberIncarnation;
const generatedAbsentResidency: BridgeDeliveryRejectionCauseStaleMemberResidency = {
  kind: "stale_member_residency",
  expected: generatedResidencyMember,
};
const generatedAbsentResidencyCurrent: BridgeMemberIncarnation | null | undefined =
  generatedAbsentResidency.current;
const generatedBuildRejection: MemberBuildRejection =
  generatedMaterializeRejection.cause;
const generatedPresentedScopes: WireControlScope[] =
  generatedScopeRejection.presented;
const generatedRequiredScope: WireControlScope = generatedScopeRejection.required;

void generatedDeliveryExpectedMember;
void generatedOutboundTaintTarget;
void generatedDeliveryTurn;
void generatedCommandExpectedMember;
void generatedCommandTurn;
void generatedInjectedContext;
void generatedExpectedRunId;
void generatedOperationId;
void generatedCursor;
void generatedOutcomeAcks;
void generatedLiveTransport;
void generatedCommandOverlay;
void generatedUnwireOverlay;
void generatedPayloadOverlay;
void generatedCurrentMember;
void generatedAbsentResidency;
void generatedAbsentResidencyCurrent;
void generatedBuildRejection;
void generatedPresentedScopes;
void generatedRequiredScope;

declare const generatedCommsNotice: SystemNoticeBlockComms;
declare const generatedToolConfigNotice: SystemNoticeBlockToolConfig;
const generatedNoticePeer: SystemNoticePeer | null | undefined =
  generatedCommsNotice.peer;
const generatedToolConfigPayload: ToolConfigChangedPayload =
  generatedToolConfigNotice.payload;
const generatedToolConfigStatus: ToolConfigChangeStatus =
  generatedToolConfigPayload.status_info;
const generatedDeferredCatalogDelta: DeferredCatalogDelta | null | undefined =
  generatedToolConfigPayload.deferred_catalog_delta;
void generatedNoticePeer;
void generatedToolConfigPayload;
void generatedToolConfigStatus;
void generatedDeferredCatalogDelta;

// The package root is the supported SDK surface. Every new public
// host/history/route/member-live contract (including discriminated control
// variants and nested wire vocabulary) must remain importable from there.
type PublicMultiHostContractSurface = [
  BridgeLiveControlOutcome,
  BridgeLiveControlOutcomeCommitInput,
  BridgeLiveControlOutcomeInterrupt,
  BridgeLiveControlOutcomeRefresh,
  BridgeLiveControlOutcomeTruncate,
  BridgeLiveControlVerb,
  BridgeLiveControlVerbCommitInput,
  BridgeLiveControlVerbInterrupt,
  BridgeLiveControlVerbRefresh,
  BridgeLiveControlVerbTruncate,
  MobBindHostParams,
  MobBindHostResult,
  MobHostStatus,
  MobHostsResult,
  MobMemberHistoryParams,
  MobMemberHistoryResult,
  MobMemberLiveChannelParams,
  MobMemberLiveControlParams,
  MobMemberLiveOpenParams,
  MobMemberLiveStatusParams,
  MobRevokeHostParams,
  MobRevokeHostResult,
  MobRouteInstallsResult,
  WireHistoryRow,
  WireHostBindPhase,
  WireHostBindingDescriptor,
  WireHostBindingDescriptorKind,
  WireHostCapabilityFlags,
  WireHostRef,
  WireMemberHistoryPageBody,
  WireMemberLifecycleCapabilities,
  WireNonPortableResourceKind,
  WireProjectionProvenance,
  WireReachability,
  WireRouteInstallObligation,
  WireTrustedPeerIdentity,
  WireTrustedPeerIdentityEd25519PublicKey,
];
declare const publicMultiHostContractSurface: PublicMultiHostContractSurface;
void publicMultiHostContractSurface;

const publicHistoryRow: WireHistoryRow = {
  role: "user",
  created_at: "2026-07-10T00:00:00Z",
  content: "hello",
};
const publicHistoryMessage: WireSessionMessage = publicHistoryRow;
void publicHistoryMessage;
const publicAssistantHistory: WireHistoryRow = {
  role: "block_assistant",
  created_at: "2026-07-10T00:00:01Z",
  blocks: [{ block_type: "text", data: { text: "answer" } }],
  stop_reason: "end_turn",
};
const publicToolResultsHistory: WireHistoryRow = {
  role: "tool_results",
  created_at: "2026-07-10T00:00:02Z",
  results: [{ tool_use_id: "tool-1", content: "done" }],
};
const publicSystemNoticeHistory: WireHistoryRow = {
  role: "system_notice",
  created_at: "2026-07-10T00:00:03Z",
  kind: "generic",
  blocks: [{ type: "auth", state: "refresh_required" }],
};
void publicAssistantHistory;
void publicToolResultsHistory;
void publicSystemNoticeHistory;

const publicSessionContentBlock: SessionContentBlock = {
  type: "structured",
  data: { answer: 42 },
};
const publicSessionContent: SessionContentInput = [
  publicSessionContentBlock,
  { type: "unknown" },
  {
    type: "image",
    media_type: "image/png",
    source: "inline",
    data: "aW1hZ2U=",
  },
];
void publicSessionContent;

const scopeDeniedDetail: WireScopeDeniedDetail = {
  required: "live",
  presented: ["list"],
};
const hostUnavailableDetail: WireHostUnavailableDetail = {
  host: "host:remote-1",
  timeout_ms: 5000,
};
const staleCursorDetail: WireStaleCursorDetail = {
  watermark: 42,
  requested: 7,
};
const staleFenceDetail: WireStaleFenceDetail = {
  runtime_id: "runtime-1",
  expected: 9,
  actual: 8,
};
const multiHostErrorCode: MultiHostErrorCode = "SCOPE_DENIED";
// @ts-expect-error multi-host error codes are a generated closed vocabulary.
const invalidMultiHostErrorCode: MultiHostErrorCode = "REMOTE_FAILURE";
void scopeDeniedDetail;
void hostUnavailableDetail;
void staleCursorDetail;
void staleFenceDetail;
void multiHostErrorCode;
void invalidMultiHostErrorCode;

declare const publicMob: Mob;
declare const publicClient: MeerkatClient;

const clientGrantResult: Promise<WireGrantRecord> = publicClient.grantMobScopes(
  "mob-1",
  "operator-1",
  ["read_history"],
);
const clientGrantList: Promise<WireGrantRecord[]> =
  publicClient.listMobGrants("mob-1");
const mobGrantResult: Promise<WireGrantRecord> = publicMob.grantScopes(
  "operator-1",
  ["read_history"],
);
const mobGrantList: Promise<WireGrantRecord[]> = publicMob.grants();
void clientGrantResult;
void clientGrantList;
void mobGrantResult;
void mobGrantList;

const memberLiveOpenOptions: MobMemberLiveOpenOptions = {
  turningMode: "explicit_commit",
  transport: "websocket",
};
const memberLiveTurningMode: RealtimeTurningMode = "provider_managed";
const memberLiveOpenParams: MobMemberLiveOpenParams = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  turning_mode: memberLiveTurningMode,
  transport: "webrtc",
};

type ClientMemberLiveOpenOptions = NonNullable<
  Parameters<MeerkatClient["openMobMemberLive"]>[2]
>;
const clientMemberLiveOpenOptions: ClientMemberLiveOpenOptions = memberLiveOpenOptions;

const invalidMemberLiveTurningMode: MobMemberLiveOpenOptions = {
  // @ts-expect-error turning mode is the generated closed vocabulary.
  turningMode: "automatic",
};
// @ts-expect-error transport is the generated websocket/webrtc literal.
const invalidMemberLiveTransport: MobMemberLiveOpenOptions = { transport: "sse" };

const clientMemberLiveOpenResult: Promise<LiveOpenResult> =
  publicClient.openMobMemberLive("mob-1", "worker-1", memberLiveOpenOptions);
const mobMemberLiveOpenResult: Promise<LiveOpenResult> =
  publicMob.memberLiveOpen("worker-1", memberLiveOpenOptions);
const mobMemberLiveCloseResult: Promise<LiveCloseResult> =
  publicMob.memberLiveClose("worker-1", "channel-1");
const mobMemberLiveStatusResult: Promise<LiveStatusResult> =
  publicMob.memberLiveStatus("worker-1", "channel-1");
const memberLiveControlVerb: BridgeLiveControlVerb = {
  verb: "truncate",
  item_id: "item-1",
  content_index: 0,
  audio_played_ms: 50,
};
const mobMemberLiveControlResult: Promise<BridgeLiveControlOutcome> =
  publicMob.memberLiveControl("worker-1", "channel-1", memberLiveControlVerb);

// @ts-expect-error control verbs are a generated discriminated union.
publicMob.memberLiveControl("worker-1", "channel-1", { verb: "pause" });

void memberLiveOpenParams;
void clientMemberLiveOpenOptions;
void invalidMemberLiveTurningMode;
void invalidMemberLiveTransport;
void clientMemberLiveOpenResult;
void mobMemberLiveOpenResult;
void mobMemberLiveCloseResult;
void mobMemberLiveStatusResult;
void mobMemberLiveControlResult;

const spawnSpec: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-1",
  placement: "host-b",
};

void spawnSpec;

const mobControlScope: MobControlScope = "admin_grants";
const mobGrantRecord: MobGrantRecord = {
  principal: "operator-1",
  scopes: ["read_history", mobControlScope],
  expires_at_ms: 1_750_000_000_000,
};
// @ts-expect-error grant records preserve the generated snake_case boundary.
mobGrantRecord.expiresAtMs;
// @ts-expect-error grant scopes are a closed wire vocabulary.
const invalidMobControlScope: MobControlScope = "manage";

void mobGrantRecord;
void invalidMobControlScope;

const sessionOptionsWithAuthBinding: SessionOptions = {
  authBinding: { realm: "dev", binding: "default_openai" },
};

void sessionOptionsWithAuthBinding;

const mobWireMembersBatchEdges: MobWireMembersBatchEdgeInput[] = [
  ["lead-1", "worker-1"],
  { a: "lead-1", b: "reviewer-1" },
];

const mobWireMembersBatchPromise: Promise<MobWireMembersBatchResult> =
  new MeerkatClient().mobWireMembersBatch("mob-1", mobWireMembersBatchEdges);

void mobWireMembersBatchEdges;
void mobWireMembersBatchPromise;

const hookDeniedErrorReport: AgentErrorReport = {
  class: "hook",
  message: "denied",
  reason: {
    reason_type: "hook_denied",
    hook_id: "policy-gate",
    point: "pre_tool_execution",
    reason_code: "policy",
  },
};

const publicRunFailedEvent: RunFailedEvent = {
  type: "run_failed",
  sessionId: "session-1",
  errorClass: "hook",
  error: "denied",
  errorReport: hookDeniedErrorReport,
};

void publicRunFailedEvent;

const spawnSpecWithGeneration: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-2",
  // @ts-expect-error generation is runtime-owned and not a public spawn knob.
  generation: 1,
};

void spawnSpecWithGeneration;

const publicMobDefinition: MobDefinition = {
  id: "mob-1",
  profiles: {
    worker: { model: "claude-sonnet-4-6", tools: { shell: true, mcp: ["browser"] } },
    reviewer: { realm_profile: "reviewer-default" },
  },
  flows: {
    kickoff: {
      steps: {
        start: {
          role: "worker",
          message: "start",
        },
      },
    },
  },
  backend: { default: "session" },
  wiring: { auto_wire_orchestrator: true },
};

const publicMobCreateOptions: MobCreateOptions = { definition: publicMobDefinition };

void publicMobDefinition;
void publicMobCreateOptions;

const publicSupervisorRotationReport: SupervisorRotationReportWire = {
  previous_epoch: 1,
  current_epoch: 2,
  public_peer_id: "ed25519:supervisor-next",
};

const publicSupervisorRotationResult: MobRotateSupervisorResult = {
  mob_id: "mob-1",
  ok: true,
  report: publicSupervisorRotationReport,
};

void publicSupervisorRotationResult;

const publicMobDefinitionWithBadFlow: MobDefinition = {
  id: "mob-bad-flow",
  profiles: { worker: { model: "claude-sonnet-4-6" } },
  flows: {
    kickoff: {
      steps: {
        // @ts-expect-error flow steps must use the typed mob flow contract.
        start: { role: "worker" },
      },
    },
  },
};

void publicMobDefinitionWithBadFlow;

const publicMobDefinitionWithBadMcpEnv: MobDefinition = {
  id: "mob-bad-mcp",
  profiles: {
    worker: {
      model: "claude-sonnet-4-6",
      tools: {
        // @ts-expect-error MCP tool allowlist entries are registry names.
        mcp: [1],
      },
    },
  },
};

void publicMobDefinitionWithBadMcpEnv;

const publicSpawnSpecWithAdvancedFields: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-advanced",
  initialMessage: [{ type: "text", text: "hello" }],
  runtimeMode: "autonomous_host",
  backend: "session",
  labels: { role: "worker" },
  context: { ticket: "LUC-134" },
  additionalInstructions: ["stay focused"],
  binding: { kind: "session" },
  shellEnv: { TEST_MODE: "1" },
  autoWireParent: true,
  launchMode: { mode: "fresh" },
  toolAccessPolicy: { type: "allow_list", value: ["grep"] },
  inheritedToolFilter: { Allow: ["grep"] },
  overrideProfile: {
    model: "claude-sonnet-4-6",
    tools: { shell: true },
  },
  authBinding: { realm: "dev", binding: "default_anthropic" },
};

void publicSpawnSpecWithAdvancedFields;

const publicTranscriptForkOptions: TranscriptForkOptions = {
  runningBehavior: "reject",
  toolAccessPolicy: { type: "allow_list", value: ["grep"] },
};
void publicTranscriptForkOptions;

const publicTranscriptRewriteOptions: TranscriptRewriteOptions = {
  runningBehavior: "reject",
  actor: "operator",
  // @ts-expect-error Tool policy belongs to fork creation, not in-place rewrite.
  toolAccessPolicy: { type: "allow_list", value: ["grep"] },
};
void publicTranscriptRewriteOptions;

const publicSpawnManySpec: SpawnManySpec = {
  profile: "worker",
  agentIdentity: "worker-many",
  placement: "host-b-peer",
  authBinding: { realm: "dev", binding: "default_anthropic" },
};

void publicSpawnManySpec;

const publicSpawnManySpecWithSingleSpawnOnlyField: SpawnManySpec = {
  profile: "worker",
  agentIdentity: "worker-many-bad",
  // @ts-expect-error launchMode is only supported by the single-member mob/spawn contract.
  launchMode: { mode: "fresh" },
};

void publicSpawnManySpecWithSingleSpawnOnlyField;

const publicMobTurnStartOptions: MobTurnStartOptions = {
  skillRefs: [{ sourceUuid: "00000000-0000-4000-8000-000000000001", skillName: "read" }],
  turnToolOverlay: { allowedTools: ["read"], blockedTools: [] },
  additionalInstructions: ["stay concise"],
  injectedContext: ["remembered operator preferences"],
  keepAlive: true,
  model: "gpt-test",
  provider: "openai",
  maxTokens: 128,
  systemPrompt: "system",
  outputSchema: { type: "object" },
  structuredOutputRetries: 2,
  providerParams: { action: "set", value: { temperature: 0.2 } },
  authBinding: { action: "set", value: { realm: "dev", binding: "default_openai" } },
};

const publicMobTurnStartOptionsClear: MobTurnStartOptions = {
  providerParams: { action: "clear" },
  authBinding: { action: "clear" },
};
void publicMobTurnStartOptionsClear;

const publicMobTurnStartOptionsWithRetiredSplitClear: MobTurnStartOptions = {
  model: "gpt-test",
  // @ts-expect-error the retired split clear_* option form fails closed.
  clearProviderParams: true,
};
void publicMobTurnStartOptionsWithRetiredSplitClear;

const publicMobTurnStartOptionsWithUnknown: MobTurnStartOptions = {
  model: "gpt-test",
  // @ts-expect-error turn_start overrides are explicit and must fail closed.
  unexpectedOverride: true,
};

type MobTurnStartSupportedWireOptionKeys =
  | "additional_instructions"
  | "auth_binding"
  | "turn_tool_overlay"
  | "injected_context"
  | "keep_alive"
  | "max_tokens"
  | "model"
  | "output_schema"
  | "provider"
  | "provider_params"
  | "skill_refs"
  | "structured_output_retries"
  | "system_prompt";
type MobTurnStartUncoveredWireOptionKeys = Exclude<
  keyof Omit<MobTurnStartParams, "mob_id" | "agent_identity" | "prompt">,
  MobTurnStartSupportedWireOptionKeys
>;
type AssertNever<T extends never> = T;
type MobTurnStartNoUncoveredWireOptionKeys =
  AssertNever<MobTurnStartUncoveredWireOptionKeys>;
const generatedMobTurnStartOptionCoverage: MobTurnStartNoUncoveredWireOptionKeys =
  null as never;

const publicMobTurnStartClient = new MeerkatClient();
void publicMobTurnStartClient.mobTurnStart(
  "mob-1",
  "worker-1",
  [{ type: "text", text: "continue" }],
  publicMobTurnStartOptions,
);
void publicMobTurnStartOptions;

const toolCallRequestedEvent: ToolCallRequestedEvent = {
  type: "tool_call_requested",
  id: "tool-1",
  name: "search",
  args: { query: "rust" },
};

void toolCallRequestedEvent;

const toolCallRequestedWithStringArgs: ToolCallRequestedEvent = {
  type: "tool_call_requested",
  id: "tool-2",
  name: "search",
  // @ts-expect-error tool-call args are projected as object-only semantic data.
  args: "{\"query\":\"rust\"}",
};

void toolCallRequestedWithStringArgs;
void publicMobTurnStartOptionsWithUnknown;
void generatedMobTurnStartOptionCoverage;

const publicPeerResponsePeerId =
  "00000000-0000-4000-8000-000000000161" as PeerId;
const publicPeerResponseRequestId =
  "00000000-0000-4000-8000-000000000162" as PeerCorrelationId;
void publicMobTurnStartClient.sendPeerResponseTerminal(
  "session-1",
  publicPeerResponsePeerId,
  publicPeerResponseRequestId,
  "completed",
  { ok: true },
  { displayName: "analyst" },
);
void publicMobTurnStartClient.sendPeerResponseTerminal(
  "session-1",
  // @ts-expect-error peer id and peer correlation id are branded separately.
  publicPeerResponseRequestId,
  publicPeerResponsePeerId,
  "completed",
  { ok: true },
);
void publicMobTurnStartClient.sendPeerResponseTerminal(
  "session-1",
  // @ts-expect-error raw strings cannot satisfy the public peer-response identity contract.
  "analyst",
  "req-1",
  "completed",
  { ok: true },
);

const generatedMobSpawn: MobSpawnParams = {
  mob_id: "mob-1",
  profile: "worker",
  agent_identity: "worker-1",
};

void generatedMobSpawn;

const publicIndexedMobSpawnParams: PublicMobSpawnParams = generatedMobSpawn;
const publicIndexedMobSpawnSpecParams: PublicMobSpawnSpecParams = {
  profile: "worker",
  agent_identity: "worker-indexed",
};

void publicIndexedMobSpawnParams;
void publicIndexedMobSpawnSpecParams;

const generatedMobSpawnWithAdvancedJsonSlot: MobSpawnParams = {
  mob_id: "mob-1",
  profile: "worker",
  agent_identity: "worker-2",
  launch_mode: { mode: "fresh" },
  tool_access_policy: { type: "allow_list", value: ["grep"] },
  inherited_tool_filter: { Allow: ["grep"] },
  override_profile: {
    model: "claude-sonnet-4-6",
    tools: { shell: true },
  },
};

void generatedMobSpawnWithAdvancedJsonSlot;

const generatedMobLaunchMode: WireMemberLaunchMode = { mode: "fresh" };
const generatedMobToolAccess: WireToolAccessPolicy = {
  type: "allow_list",
  value: ["grep"],
};
const generatedMobToolFilter: WireToolFilter = { Allow: ["grep"] };
const generatedMobProfile: WireMobProfile = { model: "claude-sonnet-4-6" };
const generatedMobOverrideTools: WireMobToolConfig = { shell: true };
const generatedMobOverrideToolsWithRustBundles: WireMobToolConfig = {
  // @ts-expect-error rust_bundles is runtime-internal and not public spawn input.
  rust_bundles: ["internal-only"],
};

void generatedMobLaunchMode;
void generatedMobToolAccess;
void generatedMobToolFilter;
void generatedMobProfile;
void generatedMobOverrideTools;
void generatedMobOverrideToolsWithRustBundles;

const generatedMobDefinition: MobDefinitionInput = {
  id: "mob-1",
  profiles: {
    worker: { model: "claude-sonnet-4-6" },
  },
};

const generatedMobCreate: MobCreateParams = {
  definition: generatedMobDefinition,
};

void generatedMobCreate;

const generatedMobCreateWithBadDefinition: MobCreateParams = {
  // @ts-expect-error definition must use the typed mob definition input.
  definition: {},
};

void generatedMobCreateWithBadDefinition;

const generatedMobMemberSpec: MobMemberSpecWire = {
  profile: "worker",
  agent_identity: "worker-1",
};

const generatedMobEnsureMemberParams: MobEnsureMemberParams = {
  mob_id: "mob-1",
  spec: generatedMobMemberSpec,
};

void generatedMobEnsureMemberParams;

const generatedMobEnsureMemberWithBadSpec: MobEnsureMemberParams = {
  mob_id: "mob-1",
  // @ts-expect-error spec must use the typed member spec contract.
  spec: { profile: "worker" },
};

void generatedMobEnsureMemberWithBadSpec;

const generatedMobReconcileParams: MobReconcileParams = {
  mob_id: "mob-1",
  desired: [generatedMobMemberSpec],
};

void generatedMobReconcileParams;

const generatedMobReconcileWithBadDesired: MobReconcileParams = {
  mob_id: "mob-1",
  // @ts-expect-error desired entries must use the typed member spec contract.
  desired: [{ profile: "worker" }],
};

void generatedMobReconcileWithBadDesired;

const generatedMobSpawnManySpec: MobSpawnSpecParams = {
  profile: "worker",
  agent_identity: "worker-3",
  initial_message: "hello",
  backend: "session",
  placement: "host-b-peer",
  auth_binding: { realm: "dev", binding: "default_anthropic" },
};

const generatedMobSpawnManyBackend: WireMobBackendKind = "session";
const generatedMobSpawnManyAuthBindingRef: WireAuthBindingRef = {
  realm: "dev",
  binding: "default_anthropic",
};

void generatedMobSpawnManyBackend;
void generatedMobSpawnManyAuthBindingRef;

const generatedMobSpawnManySpecWithBadContent: MobSpawnSpecParams = {
  profile: "worker",
  agent_identity: "worker-bad",
  // @ts-expect-error content input must be text or content blocks.
  initial_message: 123,
};

void generatedMobSpawnManySpecWithBadContent;

const generatedMobSpawnManySpecWithBadBackend: MobSpawnSpecParams = {
  profile: "worker",
  agent_identity: "worker-bad-backend",
  // @ts-expect-error backend is a closed mob backend enum.
  backend: "background",
};

void generatedMobSpawnManySpecWithBadBackend;

const generatedMobSpawnManyParams: MobSpawnManyParams = {
  mob_id: "mob-1",
  specs: [generatedMobSpawnManySpec],
};

const generatedMobSpawnManyResultEntry: MobSpawnManyResultEntry = {
  status: "spawned",
  result: {
    agent_identity: "worker-3",
    member_ref: "opaque-member-ref",
  },
};

const generatedMobSpawnManyResult: MobSpawnManyResult = {
  results: [generatedMobSpawnManyResultEntry],
};

const generatedMobSpawnManyStatus: MobSpawnManyResultStatus = "failed";
const generatedMobSpawnManyFailureCause: MobSpawnManyFailureCause = "missing_member_capability";
const generatedMobSpawnManyFailure: MobSpawnManyFailedResult = {
  cause: generatedMobSpawnManyFailureCause,
  message: "profile missing",
};
const generatedMobSpawnManyPayload: MobSpawnManyResultPayload = generatedMobSpawnManyFailure;
const generatedMobSpawnManySpawned: MobSpawnManySpawnedResult = {
  agent_identity: "worker-4",
  member_ref: "opaque-member-ref-4",
};

void generatedMobSpawnManyParams;
void generatedMobSpawnManyResult;
void generatedMobSpawnManyStatus;
void generatedMobSpawnManyPayload;
void generatedMobSpawnManySpawned;

const generatedMobWireMembersBatchParams: MobWireMembersBatchParams = {
  mob_id: "mob-1",
  edges: [{ a: "lead-1", b: "worker-1" }],
};
const generatedMobWireMembersBatchReport: WireMobWireMembersBatchResult = {
  requested: 1,
  wired: [{ a: "lead-1", b: "worker-1" }],
  already_wired: [],
};

void generatedMobWireMembersBatchParams;
void generatedMobWireMembersBatchReport;

const generatedMobTurnStart: MobTurnStartParams = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  prompt: [{ type: "text", text: "continue" }],
  model: "gpt-test",
  provider_params: { action: "clear" },
};

void generatedMobTurnStart;

const generatedMobSpawnResult: MobSpawnResult = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  member_ref: "opaque-member-ref",
};

void generatedMobSpawnResult;

const generatedMobMemberStatus: MobMemberStatusResult = {
  status: "active",
  member_ref: "opaque-member-ref",
  tokens_used: 0,
  is_final: false,
};

void generatedMobMemberStatus;

const generatedMobMemberRef: WireMemberRef = "opaque-member-ref";

const generatedMobRuntimeMode: WireMobRuntimeMode = "turn_driven";
const generatedMobMemberListStatus: WireMobMemberStatus = "active";

void generatedMobRuntimeMode;
void generatedMobMemberListStatus;

const generatedMobSpawnReceipt: MobSpawnReceiptWire = {
  agent_identity: "worker-1",
  member_ref: generatedMobMemberRef,
};

void generatedMobSpawnReceipt;

const generatedMobMemberListEntry: MobMemberListEntryWire = {
  agent_identity: "worker-1",
  member_ref: generatedMobMemberRef,
  role: "worker",
  runtime_mode: "turn_driven",
  status: "active",
  is_final: false,
};

void generatedMobMemberListEntry;

const generatedMobEnsureMember: MobEnsureMemberResult = {
  outcome: { spawned: generatedMobSpawnReceipt },
};

void generatedMobEnsureMember;

const generatedMobMembers: MobMembersResult = {
  mob_id: "mob-1",
  members: [generatedMobMemberListEntry],
};

void generatedMobMembers;

const generatedMobSubmitFromMembers: MobSubmitWorkParams = {
  member_ref: generatedMobMembers.members[0].member_ref,
  content: "continue",
};

void generatedMobSubmitFromMembers;

const generatedMobSubmitFromEnsureMember: MobSubmitWorkParams = {
  member_ref:
    "spawned" in generatedMobEnsureMember.outcome
      ? generatedMobEnsureMember.outcome.spawned.member_ref
      : generatedMobEnsureMember.outcome.existed.member_ref,
  content: "continue",
};

void generatedMobSubmitFromEnsureMember;

// @ts-expect-error mob runtime mode is a closed wire enum.
const generatedMobBadRuntimeMode: WireMobRuntimeMode = "background";

void generatedMobBadRuntimeMode;

const generatedMobBadMemberStatus: MobMemberListEntryWire = {
  agent_identity: "worker-1",
  member_ref: generatedMobMemberRef,
  role: "worker",
  runtime_mode: "turn_driven",
  // @ts-expect-error member-list status is a closed wire enum.
  status: "paused",
  is_final: false,
};

void generatedMobBadMemberStatus;

const sdkCommsImageBlock: ContentBlock = {
  type: "image",
  media_type: "image/png",
  source: "inline",
  data: "AAAA",
};

const sdkCommsVideoUriBlock: ContentBlock = {
  type: "video",
  media_type: "video/mp4",
  duration_ms: 12000,
  source: "uri",
  uri: "gs://example-bucket/timeline.mp4",
};

const sdkCommsPeerMessageWithBlocks: CommsCommand = {
  kind: "peer_message",
  to: "agent-a",
  body: "describe",
  blocks: [{ type: "text", text: "describe" }, sdkCommsImageBlock, sdkCommsVideoUriBlock],
};

const sdkCommsPeerRequestWithBlocks: CommsCommand = {
  kind: "peer_request",
  to: "agent-a",
  intent: "checksum_token",
  params: { subject: "attached" },
  blocks: [{ type: "text", text: "describe" }, sdkCommsImageBlock],
};

const sdkCommsSupervisorBridgeWithPublicBlocks: CommsCommand = {
  kind: "peer_request",
  to: "agent-a",
  intent: "supervisor.bridge",
  params: {
    command: "deliver_member_input",
    content: [{ type: "text", text: "describe" }, sdkCommsImageBlock],
    epoch: 1,
    expected_member: {
      mob_id: "mob-1",
      agent_identity: "worker-1",
      host_id: "host-1",
      binding_generation: 2,
      member_session_id: "session-1",
      generation: 3,
      fence_token: 4,
    },
    handling_mode: "steer",
    injected_context: ["host context"],
    input_id: "input-1",
    protocol_version: 1,
    supervisor: {
      address: "inproc://supervisor",
      name: "supervisor",
      peer_id: "pictionary/supervisor/supervisor",
    },
    turn: { correlation: { run_id: "run-1", step_id: "step-1" } },
  },
  blocks: [{ type: "text", text: "describe" }, sdkCommsImageBlock],
};

const sdkBridgeSupervisor = {
  address: "inproc://supervisor",
  name: "supervisor",
  peer_id: "pictionary/supervisor/supervisor",
  pubkey: [1, 2, 3] as const,
} as const;

const sdkBridgeExpectedMember: BridgeMemberIncarnation = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  host_id: "host-1",
  binding_generation: 2,
  member_session_id: "session-1",
  generation: 3,
  fence_token: 4,
};

const sdkBridgeHardCancel: BridgeCommandHardCancelMember = {
  command: "hard_cancel_member",
  epoch: 1,
  expected_member: sdkBridgeExpectedMember,
  expected_run_id: "run-1",
  operation_id: "operation-1",
  protocol_version: 1,
  reason: "operator interrupt",
  supervisor: sdkBridgeSupervisor,
};

const sdkBridgePoll: BridgeCommandPollMemberEvents = {
  command: "poll_member_events",
  cursor: { cursor: "at", generation: 3, seq: 9 },
  epoch: 1,
  expected_member: sdkBridgeExpectedMember,
  outcome_acks: [{ generation: 3, fence_token: 4, input_id: "input-1" }],
  protocol_version: 1,
  supervisor: sdkBridgeSupervisor,
};

const sdkBridgeLiveOpen: BridgeCommandOpenMemberLiveChannel = {
  command: "open_member_live_channel",
  epoch: 1,
  expected_member: sdkBridgeExpectedMember,
  protocol_version: 1,
  supervisor: sdkBridgeSupervisor,
  transport: "websocket",
};

// Overlay handoff is optional on both wire and unwire for pre-V3 peers.
const sdkBridgeWireWithoutOverlay: BridgeCommandWireMember = {
  command: "wire_member",
  epoch: 1,
  peer_spec: sdkBridgeSupervisor,
  protocol_version: 1,
  supervisor: sdkBridgeSupervisor,
};
const sdkBridgeUnwireWithoutOverlay: BridgeCommandUnwireMember = {
  command: "unwire_member",
  epoch: 1,
  peer_spec: sdkBridgeSupervisor,
  protocol_version: 1,
  supervisor: sdkBridgeSupervisor,
};

const sdkCommsPeerRequestIntentMismatch: CommsCommand = {
  kind: "peer_request",
  to: "agent-a",
  intent: "supervisor.bridge",
  // @ts-expect-error supervisor.bridge requests must carry BridgeCommand params.
  params: { subject: "attached" },
};

void sdkCommsPeerMessageWithBlocks;
void sdkCommsPeerRequestWithBlocks;
void sdkCommsSupervisorBridgeWithPublicBlocks;
void sdkCommsPeerRequestIntentMismatch;
void sdkBridgeHardCancel;
void sdkBridgePoll;
void sdkBridgeLiveOpen;
void sdkBridgeWireWithoutOverlay;
void sdkBridgeUnwireWithoutOverlay;

// R5-10: `LiveSendInputParams.chunk` must be the typed `LiveInputChunkWire`
// discriminated union, not an opaque `Record<string, unknown>`. Each typed
// variant must compile under the typed `chunk` slot.
const liveAudioChunk: LiveInputChunkWire = {
  kind: "audio",
  data: "AQID",
  sample_rate_hz: 24_000,
  channels: 1,
};
const liveTextChunk: LiveInputChunkWire = {
  kind: "text",
  text: "hello",
};
const liveImageChunk: LiveInputChunkWire = {
  kind: "image",
  idempotency_key: "image-turn-1",
  mime: "image/png",
  data: "iVBORw0KGgo=",
};
const liveVideoFrameChunk: LiveInputChunkWire = {
  kind: "video_frame",
  codec: "vp8",
  data: "AQID",
  timestamp_ms: 1_234,
};
const realtimeImageChunk: RealtimeImageChunk = {
  idempotency_key: "image-turn-1",
  mime_type: "image/png",
  data: "iVBORw0KGgo=",
};
const realtimeInputKind: RealtimeInputKind = "image";
const realtimeInputChunk: RealtimeInputChunk = {
  kind: "image_chunk",
  idempotency_key: realtimeImageChunk.idempotency_key,
  mime_type: realtimeImageChunk.mime_type,
  data: realtimeImageChunk.data,
};
void realtimeInputKind;
void realtimeInputChunk;

const liveSendInputAudio: LiveSendInputParams = {
  channel_id: "live_1",
  chunk: liveAudioChunk,
};
const liveSendInputText: LiveSendInputParams = {
  channel_id: "live_1",
  chunk: liveTextChunk,
};
const liveSendInputImage: LiveSendInputParams = {
  channel_id: "live_1",
  chunk: liveImageChunk,
};
const liveSendInputVideoFrame: LiveSendInputParams = {
  channel_id: "live_1",
  chunk: liveVideoFrameChunk,
};
const liveInputTooLarge: WireLiveConfigRejectionReasonInputTooLarge = {
  kind: "input_too_large",
  max_bytes: 256,
  actual_bytes: 257,
};
const liveInputBackpressured: WireLiveConfigRejectionReasonInputBackpressured = {
  kind: "input_backpressured",
  max_pending_bytes: 1024,
};
const liveSendInputErrorData: LiveSendInputErrorData = {
  error_code: {
    code: "config_rejected",
    reason: liveInputBackpressured,
  },
};
void liveInputTooLarge;
void liveSendInputErrorData;

// R5-10: chunks missing the `kind` discriminator must be rejected at compile
// time. This proves `chunk` is no longer typed as `Record<string, unknown>`,
// which would have accepted any free-form object.
const liveSendInputUntyped: LiveSendInputParams = {
  channel_id: "live_1",
  // @ts-expect-error LiveInputChunkWire requires a discriminated `kind` tag.
  chunk: { foo: "bar" },
};

void liveSendInputAudio;
void liveSendInputText;
void liveSendInputImage;
void liveSendInputVideoFrame;
void liveSendInputUntyped;
void realtimeImageChunk;

// FIX-SDK-OBS: `WireLiveAdapterObservation` is a discriminated union over
// the `observation` tag. Browser/Node clients can type-narrow on each
// variant and read R5-4 identity fields (`item_id`, `response_id`,
// `content_index`) on `assistant_audio_chunk` and the typed `code`
// payload on R5-9 `command_rejected` without parsing raw JSON.

const liveObsAudio: WireLiveAdapterObservation = {
  observation: "assistant_audio_chunk",
  data: "AQID",
  sample_rate_hz: 24_000,
  channels: 1,
  item_id: "item_audio",
  response_id: "resp_audio",
  content_index: 0,
};
const liveObsCommandRejected: WireLiveAdapterObservation = {
  observation: "command_rejected",
  // R7-2 (P2): `reason` is now emitted as the typed
  // `WireLiveConfigRejectionReason` discriminated union (tagged on
  // `kind`) rather than `Record<string, unknown>`. SDK codegen follows
  // the schema-local `$defs` reference into a named union so consumers
  // route on `kind` without parsing raw JSON.
  code: {
    code: "config_rejected",
    reason: { kind: "image_input_not_implemented" },
  },
  message: "rejected",
};
const liveObsReady: WireLiveAdapterObservation = { observation: "ready" };
const liveObsTurnInterrupted: WireLiveAdapterObservation = {
  observation: "turn_interrupted",
};
const liveObsUserContentCommitted: WireLiveAdapterObservationUserContentCommitted = {
  observation: "user_content_committed",
  idempotency_key: "image-turn-1",
  item_id: "item_image",
  content_index: 0,
  media_type: "image/png",
};
const liveInvalidBase64Reason: WireLiveConfigRejectionReasonImageInputInvalidBase64 = {
  kind: "image_input_invalid_base64",
};
const liveRewriteRequiresReopenReason: WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen = {
  kind: "refresh_transcript_rewrite_requires_reopen",
};

void liveInvalidBase64Reason;
void liveRewriteRequiresReopenReason;

// Type narrowing on the discriminator: each branch sees the right
// payload without an `as` cast.
function readAudioIdentity(
  obs: WireLiveAdapterObservation,
): {
  item_id?: string | null;
  response_id?: string | null;
  content_index?: number | null;
} | null {
  if (obs.observation !== "assistant_audio_chunk") return null;
  // Compile-time proof: `obs` is narrowed to the audio variant; the
  // identity fields are visible without further type assertions.
  const audio: WireLiveAdapterObservationAssistantAudioChunk = obs;
  return {
    item_id: audio.item_id,
    response_id: audio.response_id,
    content_index: audio.content_index,
  };
}

function readRejectionCode(
  obs: WireLiveAdapterObservation,
): WireLiveAdapterErrorCode | null {
  if (obs.observation !== "command_rejected") return null;
  const rejected: WireLiveAdapterObservationCommandRejected = obs;
  return rejected.code;
}

function readCommittedImageIdentity(
  obs: WireLiveAdapterObservation,
): { idempotencyKey: string; itemId: string; contentIndex: number; mediaType: string } | null {
  if (obs.observation !== "user_content_committed") return null;
  const committed: WireLiveAdapterObservationUserContentCommitted = obs;
  return {
    idempotencyKey: committed.idempotency_key,
    itemId: committed.item_id,
    contentIndex: committed.content_index,
    mediaType: committed.media_type,
  };
}

// `chunk: { foo: "bar" }` was the @ts-expect-error pattern for chunks; the
// same constraint must hold for observations missing the discriminator.
const liveObsUntyped: WireLiveAdapterObservation = {
  // @ts-expect-error WireLiveAdapterObservation requires the typed `observation` tag.
  observation: "not_a_real_variant",
};

void liveObsAudio;
void liveObsCommandRejected;
void liveObsReady;
void liveObsTurnInterrupted;
void liveObsUserContentCommitted;
void liveObsUntyped;
void readAudioIdentity;
void readRejectionCode;
void readCommittedImageIdentity;

// The root transcript alias is the public-safe wire union. Its variants can
// be narrowed without exposing the internal byte-bearing user-content command.
const safeRealtimeTranscript: RealtimeTranscriptEvent = {
  type: "item_observed",
  item_id: "item-user-1",
  role: "user",
};
const narrowedSafeRealtimeTranscript: RealtimeTranscriptEventItemObserved =
  safeRealtimeTranscript;
const unsafeRealtimeTranscript: RealtimeTranscriptEvent = {
  // @ts-expect-error `user_content_final` is internal and absent from the wire union.
  type: "user_content_final",
  item_id: "item-private",
};
void narrowedSafeRealtimeTranscript;
void unsafeRealtimeTranscript;

// =============================================================================
// R7-2 (P2) regression: `WireLiveConfigRejectionReason` lands as a typed
// discriminated union (tagged on `kind`) at the SDK boundary, NOT
// `Record<string, unknown>`. Each variant is constructible with its typed
// payload, and the union slots into `WireLiveAdapterErrorCodeConfigRejected.reason`
// without `as` casts. This pins the codegen contract: schema-local `$defs`
// referenced by promoted typed enums must be followed across the ref so SDK
// consumers can route on `kind` without parsing English from a wildcard.
// =============================================================================

const reasonChannelIdentitySwap: WireLiveConfigRejectionReason = {
  kind: "channel_identity_swap",
  from_model: "gpt-5.4",
  from_provider: "openai",
  to_model: "gemini-3.1-flash-lite",
  to_provider: "gemini",
};
const reasonNonRealtime: WireLiveConfigRejectionReason = {
  kind: "non_realtime_resolution",
  detail: "no realtime adapter available",
};
const reasonImageInput: WireLiveConfigRejectionReason = {
  kind: "image_input_not_implemented",
};
const reasonImageUnsupportedMime: WireLiveConfigRejectionReason = {
  kind: "image_input_unsupported_mime",
  mime_type: "image/gif",
};
const reasonImageContentMismatch: WireLiveConfigRejectionReason = {
  kind: "image_input_content_mismatch",
  mime_type: "image/png",
};
const reasonImageTooLarge: WireLiveConfigRejectionReason = {
  kind: "image_input_too_large",
  max_bytes: 20,
  actual_bytes: 21,
};
const reasonInvalidImageIdempotencyKey: WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid = {
  kind: "image_input_idempotency_key_invalid",
  max_bytes: 128,
  actual_bytes: 129,
};
const reasonImageIdempotencyConflict: WireLiveConfigRejectionReasonImageInputIdempotencyConflict = {
  kind: "image_input_idempotency_conflict",
};
const reasonImageHistoryBudgetExceeded: WireLiveConfigRejectionReason = {
  kind: "image_input_history_budget_exceeded",
  max_decoded_bytes: 8_388_608,
};
const reasonImageRequiresCommit: WireLiveConfigRejectionReasonImageInputRequiresCommit = {
  kind: "image_input_requires_commit",
};
const reasonInputTooLarge: WireLiveConfigRejectionReason = {
  kind: "input_too_large",
  max_bytes: 64,
  actual_bytes: 65,
};
const reasonInputBackpressured: WireLiveConfigRejectionReason = {
  kind: "input_backpressured",
  max_pending_bytes: 64,
};
const reasonImageBackpressured: WireLiveConfigRejectionReason = {
  kind: "image_input_backpressured",
  max_pending_bytes: 40,
};
const reasonImageTransportUnsupported: WireLiveConfigRejectionReason = {
  kind: "image_input_transport_unsupported",
  transport: "webrtc_data_channel_use_live_send_input_rpc",
};
const reasonVideoFrameInput: WireLiveConfigRejectionReason = {
  kind: "video_frame_input_not_implemented",
};
const reasonUnsupportedChunk: WireLiveConfigRejectionReason = {
  kind: "unsupported_input_chunk_variant",
};
const reasonRefreshModelSwap: WireLiveConfigRejectionReason = {
  kind: "refresh_model_swap",
  from_model: "gpt-5.4",
  to_model: "gpt-5.4-mini",
};
const reasonRefreshProviderSwap: WireLiveConfigRejectionReason = {
  kind: "refresh_provider_swap",
  from_provider: "openai",
  to_provider: "gemini",
};
const reasonRefreshAudioMismatch: WireLiveConfigRejectionReason = {
  kind: "refresh_audio_config_mismatch",
  detail: "mismatched sample rate",
};
const reasonAudioFormatMismatch: WireLiveConfigRejectionReason = {
  kind: "audio_input_format_mismatch",
  expected_sample_rate_hz: 24_000,
  expected_channels: 1,
  actual_sample_rate_hz: 48_000,
  actual_channels: 2,
};
const reasonOther: WireLiveConfigRejectionReason = {
  kind: "other",
  detail: "freeform escape hatch",
};
const reasonUnknown: WireLiveConfigRejectionReason = {
  kind: "unknown",
  debug: "future_core_variant_x",
};

// Each variant slots into the typed `config_rejected` error code without
// `as` casts, proving the discriminated union is followed across the
// schema reference.
const errorCodeWithReason: WireLiveAdapterErrorCode = {
  code: "config_rejected",
  reason: reasonAudioFormatMismatch,
};

// Type narrowing on `kind` exposes the variant-specific payload fields.
function readRejectionDetail(reason: WireLiveConfigRejectionReason): string {
  switch (reason.kind) {
    case "channel_identity_swap":
      return `${reason.from_model} -> ${reason.to_model}`;
    case "non_realtime_resolution":
    case "refresh_audio_config_mismatch":
    case "other":
      return reason.detail;
    case "audio_input_format_mismatch":
      return `${reason.actual_sample_rate_hz}Hz`;
    case "refresh_model_swap":
      return `${reason.from_model} -> ${reason.to_model}`;
    case "refresh_provider_swap":
      return `${reason.from_provider} -> ${reason.to_provider}`;
    case "image_input_unsupported_mime":
    case "image_input_content_mismatch":
      return reason.mime_type;
    case "image_input_too_large":
    case "image_input_idempotency_key_invalid":
    case "input_too_large":
      return `${reason.actual_bytes}/${reason.max_bytes}`;
    case "image_input_backpressured":
    case "input_backpressured":
      return `${reason.max_pending_bytes}`;
    case "image_input_history_budget_exceeded":
      return `${reason.max_decoded_bytes}`;
    case "image_input_transport_unsupported":
      return reason.transport;
    case "image_input_not_implemented":
    case "image_input_invalid_base64":
    case "image_input_idempotency_conflict":
    case "image_input_requires_commit":
    case "video_frame_input_not_implemented":
    case "unsupported_input_chunk_variant":
    case "refresh_transcript_rewrite_requires_reopen":
      return reason.kind;
    case "unknown":
      return reason.debug;
  }

  const exhaustive: never = reason;
  return exhaustive;
}

void reasonChannelIdentitySwap;
void reasonNonRealtime;
void reasonImageInput;
void reasonImageUnsupportedMime;
void reasonImageContentMismatch;
void reasonImageTooLarge;
void reasonInvalidImageIdempotencyKey;
void reasonImageIdempotencyConflict;
void reasonImageHistoryBudgetExceeded;
void reasonImageRequiresCommit;
void reasonInputTooLarge;
void reasonInputBackpressured;
void reasonImageBackpressured;
void reasonImageTransportUnsupported;
void reasonVideoFrameInput;
void reasonUnsupportedChunk;
void reasonRefreshModelSwap;
void reasonRefreshProviderSwap;
void reasonRefreshAudioMismatch;
void reasonAudioFormatMismatch;
void reasonOther;
void reasonUnknown;
void errorCodeWithReason;
void readRejectionDetail;

// =============================================================================
// R7-1 (P2) regression: `WireAssistantBlock::Transcript`'s inline `data`
// shape lands as a typed structural object (with `text`, `source`, optional
// `meta`), NOT `Record<string, unknown>`. SDK consumers can read transcript
// `text` and route on the `source` lane without ad-hoc JSON parsing. This
// pins the codegen contract: discriminated-union variant payloads with
// inline anonymous-object schemas are emitted as inline structural types.
// =============================================================================

const transcriptBlock: WireAssistantBlock = {
  block_type: "transcript",
  data: {
    text: "Hello, world.",
    source: { kind: "spoken" },
  },
};

function readTranscriptText(block: WireAssistantBlock): string | null {
  if (block.block_type !== "transcript") return null;
  // Compile-time proof: `block.data.text` is `string`, `block.data.source`
  // is `"spoken"` — both visible without further type assertions.
  return block.data.text;
}

void transcriptBlock;
void readTranscriptText;

// =============================================================================
// LiveChannel: type-level smoke tests proving the helper class is exported
// from the SDK root, constructs from a MeerkatClient + session id, and
// exposes the expected method signatures at the type level.
// =============================================================================

import { LiveChannel } from "../src/index.js";
import type { LiveChannelOptions } from "../src/index.js";

// LiveChannel.session() is the named constructor — it returns a LiveChannel.
declare const mockClient: MeerkatClient;
const liveChannel: LiveChannel = LiveChannel.session(mockClient, "session_123");
void liveChannel;

// LiveChannelOptions is assignable with turning_mode.
const liveOpts: LiveChannelOptions = { turningMode: "explicit_commit" };
void liveOpts;

// The convenience wrapper exposes the bounded serialized seed as camelCase.
const liveSeedWindowOpts: LiveChannelOptions = { seedMaxChars: 24_000 };
// @ts-expect-error seedMaxChars is numeric; the server validates positivity.
const liveInvalidSeedWindowOpts: LiveChannelOptions = { seedMaxChars: "24000" };
void liveSeedWindowOpts;
void liveInvalidSeedWindowOpts;

// LiveChannel with options
const liveChannelWithOpts: LiveChannel = LiveChannel.session(
  mockClient,
  "session_456",
  { turningMode: "provider_managed", seedMaxChars: 24_000 },
);
void liveChannelWithOpts;

// channelId is string | undefined before open().
const maybeChannelId: string | undefined = liveChannel.channelId;
void maybeChannelId;

// commitInput() only accepts the typed response-modality distinction the
// contract owns — arbitrary strings are not widened into the generated union.
type LiveCommitInputModality = Parameters<LiveChannel["commitInput"]>[0];
const liveCommitAudio: LiveCommitInputModality = "audio";
const liveCommitText: LiveCommitInputModality = "text";
// @ts-expect-error arbitrary strings are not a live response modality.
const liveCommitArbitrary: LiveCommitInputModality = "speech";
void liveCommitAudio;
void liveCommitText;
void liveCommitArbitrary;

// Image helpers require caller-stable identity as their first image argument.
type LiveSendInputImageArgs = Parameters<LiveChannel["sendInputImage"]>;
const liveImageArgs: LiveSendInputImageArgs = [
  "image-turn-1",
  "image/png",
  "iVBORw0KGgo=",
];
// @ts-expect-error image idempotency identity is required.
const liveImageArgsWithoutIdentity: LiveSendInputImageArgs = [
  "image/png",
  "iVBORw0KGgo=",
];
void liveImageArgs;
void liveImageArgsWithoutIdentity;

// sessionId is always a string.
const liveSessionId: string = liveChannel.sessionId;
void liveSessionId;

// open() returns Promise<LiveOpenResult>.
async function liveChannelOpenShape(ch: LiveChannel) {
  const result = await ch.open();
  const channelId: string = result.channel_id;
  void channelId;
  return result;
}
void liveChannelOpenShape;

// status() returns Promise<LiveStatusResult>.
async function liveChannelStatusShape(ch: LiveChannel) {
  const result = await ch.status();
  const status: import("../src/generated/types.js").WireLiveAdapterStatus = result.status;
  void status;
  return result;
}
void liveChannelStatusShape;

// close() returns Promise<LiveCloseResult>.
async function liveChannelCloseShape(ch: LiveChannel) {
  const result = await ch.close();
  const closed: "closed" = result.status;
  void closed;
  return result;
}
void liveChannelCloseShape;

// refresh() returns Promise<LiveRefreshResult>.
async function liveChannelRefreshShape(ch: LiveChannel) {
  const result = await ch.refresh();
  const queued: "queued" = result.status;
  void queued;
  return result;
}
void liveChannelRefreshShape;
