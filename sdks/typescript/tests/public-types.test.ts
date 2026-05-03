import { MeerkatClient } from "../src/index.js";
import type {
  AgentErrorReport,
  MobCreateOptions,
  MobDefinition,
  MobTurnStartOptions,
  PeerCorrelationId,
  RealtimeChannelOpenFrame,
  RealtimeOpenInfo,
  RealtimeProtocolVersion,
  PeerId,
  RunFailedEvent,
  SpawnManySpec,
  SpawnSpec,
  ToolCallRequestedEvent,
} from "../src/index.js";
import type {
  MobSpawnParams as PublicMobSpawnParams,
  MobSpawnSpecParams as PublicMobSpawnSpecParams,
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
  WireConnectionRef,
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
    worker: { model: "claude-sonnet-4-6", tools: { shell: true } },
    reviewer: { realm_profile: "reviewer-default" },
  },
  mcp_servers: {
    browser: {
      command: ["npx", "@modelcontextprotocol/server-browser"],
      env: { NODE_ENV: "test" },
    },
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
  profiles: { worker: { model: "claude-sonnet-4-6" } },
  mcp_servers: {
    browser: {
      // @ts-expect-error MCP server env values are strings in the public contract.
      env: { NODE_ENV: 1 },
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
  connectionRef: { realm: "dev", binding: "default_anthropic" },
};

void publicSpawnSpecWithAdvancedFields;

const publicSpawnManySpec: SpawnManySpec = {
  profile: "worker",
  agentIdentity: "worker-many",
  connectionRef: { realm: "dev", binding: "default_anthropic" },
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
  connectionRef: { realm: "dev", binding: "default_openai" },
  clearConnectionRef: true,
};

const publicMobTurnStartOptionsWithUnknown: MobTurnStartOptions = {
  model: "gpt-test",
  // @ts-expect-error turn_start overrides are explicit and must fail closed.
  unexpectedOverride: true,
};

type MobTurnStartSupportedWireOptionKeys =
  | "additional_instructions"
  | "clear_connection_ref"
  | "clear_provider_params"
  | "connection_ref"
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

const realtimeProtocolVersion: RealtimeProtocolVersion = "2";
const realtimeOpenFrame: RealtimeChannelOpenFrame = {
  open_token: "token-typed",
  protocol_version: realtimeProtocolVersion,
  role: "primary",
  turning_mode: "provider_managed",
};
void realtimeOpenFrame;

const realtimeOpenFrameWithUnknownProtocol: RealtimeChannelOpenFrame = {
  open_token: "token-unknown",
  // @ts-expect-error realtime protocol version truth is generated and typed.
  protocol_version: "999",
  role: "primary",
  turning_mode: "provider_managed",
};
void realtimeOpenFrameWithUnknownProtocol;

const realtimeOpenInfo: RealtimeOpenInfo = {
  ws_url: "ws://127.0.0.1:4900/realtime/ws",
  open_token: "token-info",
  expires_at: "2026-04-15T12:00:00Z",
  target: { type: "session_target", session_id: "session-typed" },
  supported_protocol_versions: [realtimeProtocolVersion],
  default_protocol_version: realtimeProtocolVersion,
  capabilities: {
    input_kinds: ["text"],
    output_kinds: ["text"],
    turning_modes: ["provider_managed"],
    interrupt_supported: true,
    transcript_supported: true,
    tool_lifecycle_events_supported: false,
    video_supported: false,
  },
};
void realtimeOpenInfo;

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
  connection_ref: { realm: "dev", binding: "default_anthropic" },
};

const generatedMobSpawnManyBackend: WireMobBackendKind = "session";
const generatedMobSpawnManyConnectionRef: WireConnectionRef = {
  realm: "dev",
  binding: "default_anthropic",
};

void generatedMobSpawnManyBackend;
void generatedMobSpawnManyConnectionRef;

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
