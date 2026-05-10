import { MeerkatClient } from "../src/index.js";
import type {
  AgentErrorReport,
  CommsCommand,
  ContentBlock,
  MobCreateOptions,
  MobDefinition,
  MobRotateSupervisorResult,
  MobTurnStartOptions,
  PeerCorrelationId,
  PeerId,
  RunFailedEvent,
  SpawnManySpec,
  SpawnSpec,
  SupervisorRotationReportWire,
  ToolCallRequestedEvent,
} from "../src/index.js";
import type {
  MobSpawnParams as PublicMobSpawnParams,
  MobSpawnSpecParams as PublicMobSpawnSpecParams,
} from "../src/index.js";
import type {
  LiveInputChunkWire,
  LiveSendInputParams,
  WireAssistantBlock,
  WireLiveAdapterErrorCode,
  WireLiveAdapterObservation,
  WireLiveAdapterObservationAssistantAudioChunk,
  WireLiveAdapterObservationCommandRejected,
  WireLiveConfigRejectionReason,
} from "../src/index.js";
import type {
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
  WireBudgetSplitPolicy,
  WireAuthBindingRef,
  WireMemberLaunchMode,
  WireMemberRef,
  WireMobBackendKind,
  WireMobProfile,
  WireMemberState,
  WireMobMemberStatus,
  WireMobRuntimeMode,
  WireMobToolConfig,
  WireToolAccessPolicy,
  WireToolFilter,
} from "../src/generated/types.js";

const spawnSpec: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-1",
};

void spawnSpec;

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
  budgetSplitPolicy: { type: "remaining" },
  inheritedToolFilter: { Allow: ["grep"] },
  overrideProfile: {
    model: "claude-sonnet-4-6",
    tools: { shell: true },
  },
  authBinding: { realm: "dev", binding: "default_anthropic" },
};

void publicSpawnSpecWithAdvancedFields;

const publicSpawnManySpec: SpawnManySpec = {
  profile: "worker",
  agentIdentity: "worker-many",
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
  flowToolOverlay: { allowedTools: ["read"], blockedTools: [] },
  additionalInstructions: ["stay concise"],
  keepAlive: true,
  model: "gpt-test",
  provider: "openai",
  maxTokens: 128,
  systemPrompt: "system",
  outputSchema: { type: "object" },
  structuredOutputRetries: 2,
  providerParams: { temperature: 0.2 },
  clearProviderParams: true,
  authBinding: { realm: "dev", binding: "default_openai" },
  clearAuthBinding: true,
};

const publicMobTurnStartOptionsWithUnknown: MobTurnStartOptions = {
  model: "gpt-test",
  // @ts-expect-error turn_start overrides are explicit and must fail closed.
  unexpectedOverride: true,
};

type MobTurnStartSupportedWireOptionKeys =
  | "additional_instructions"
  | "clear_auth_binding"
  | "clear_provider_params"
  | "auth_binding"
  | "flow_tool_overlay"
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
  budget_split_policy: { type: "remaining" },
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
const generatedMobBudgetSplit: WireBudgetSplitPolicy = { type: "remaining" };
const generatedMobToolFilter: WireToolFilter = { Allow: ["grep"] };
const generatedMobProfile: WireMobProfile = { model: "claude-sonnet-4-6" };
const generatedMobOverrideTools: WireMobToolConfig = { shell: true };
const generatedMobOverrideToolsWithRustBundles: WireMobToolConfig = {
  // @ts-expect-error rust_bundles is runtime-internal and not public spawn input.
  rust_bundles: ["internal-only"],
};

void generatedMobLaunchMode;
void generatedMobToolAccess;
void generatedMobBudgetSplit;
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
const generatedMobSpawnManyFailureCause: MobSpawnManyFailureCause = "profile_not_found";
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

const generatedMobTurnStart: MobTurnStartParams = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  prompt: [{ type: "text", text: "continue" }],
  model: "gpt-test",
  clear_provider_params: true,
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
  tokens_used: 0,
  is_final: false,
};

void generatedMobMemberStatus;

const generatedMobMemberRef: WireMemberRef = "opaque-member-ref";

const generatedMobRuntimeMode: WireMobRuntimeMode = "turn_driven";
const generatedMobMemberState: WireMemberState = "active";
const generatedMobMemberListStatus: WireMobMemberStatus = "active";

void generatedMobRuntimeMode;
void generatedMobMemberState;
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
  state: "active",
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
  state: "active",
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

const sdkCommsPeerMessageWithBlocks: CommsCommand = {
  kind: "peer_message",
  to: "agent-a",
  body: "describe",
  blocks: [{ type: "text", text: "describe" }, sdkCommsImageBlock],
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
    handling_mode: "steer",
    input_id: "input-1",
    protocol_version: 1,
    supervisor: {
      address: "inproc://supervisor",
      name: "supervisor",
      peer_id: "pictionary/supervisor/supervisor",
    },
  },
  blocks: [{ type: "text", text: "describe" }, sdkCommsImageBlock],
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
  mime: "image/png",
  data: "iVBORw0KGgo=",
};
const liveVideoFrameChunk: LiveInputChunkWire = {
  kind: "video_frame",
  codec: "vp8",
  data: "AQID",
  timestamp_ms: 1_234,
};

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

// Type narrowing on the discriminator: each branch sees the right
// payload without an `as` cast.
function readAudioIdentity(
  obs: WireLiveAdapterObservation,
): { item_id?: string; response_id?: string; content_index?: number } | null {
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
void liveObsUntyped;
void readAudioIdentity;
void readRejectionCode;

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
    case "image_input_not_implemented":
    case "video_frame_input_not_implemented":
    case "unsupported_input_chunk_variant":
      return reason.kind;
    case "unknown":
      return reason.debug;
    default:
      return "exhaustive";
  }
}

void reasonChannelIdentitySwap;
void reasonNonRealtime;
void reasonImageInput;
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

// LiveChannel with options
const liveChannelWithOpts: LiveChannel = LiveChannel.session(
  mockClient,
  "session_456",
  { turningMode: "provider_managed" },
);
void liveChannelWithOpts;

// channelId is string | undefined before open().
const maybeChannelId: string | undefined = liveChannel.channelId;
void maybeChannelId;

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

// refresh() returns Promise<LiveRefreshResult>.
async function liveChannelRefreshShape(ch: LiveChannel) {
  const result = await ch.refresh();
  const queued: "queued" = result.status;
  void queued;
  return result;
}
void liveChannelRefreshShape;
