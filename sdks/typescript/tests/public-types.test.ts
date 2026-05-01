import { MeerkatClient } from "../src/index.js";
import type {
  MobCreateOptions,
  MobDefinition,
  MobTurnStartOptions,
  PeerCorrelationId,
  PeerId,
  RuntimeTurnMetadata,
  SessionOptions,
  SpawnManySpec,
  SpawnSpec,
  TurnOptions,
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
  MobSpawnManyResult,
  MobSpawnManyResultEntry,
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

const publicRuntimeTurnMetadata: RuntimeTurnMetadata = {
  model: "claude-sonnet-4-6",
  provider: "anthropic",
  skillReferences: [
    { sourceUuid: "00000000-0000-4000-8000-000000000001", skillName: "read" },
  ],
  additionalInstructions: ["stay concise"],
  keepAlive: true,
};

const publicSessionOptions: SessionOptions = {
  turnMetadata: publicRuntimeTurnMetadata,
  labels: { team: "sdk" },
};

void publicSessionOptions;

const publicSessionOptionsWithSplitMetadata: SessionOptions = {
  // @ts-expect-error session creation metadata belongs inside turnMetadata.
  model: "claude-sonnet-4-6",
};

void publicSessionOptionsWithSplitMetadata;

const publicTurnOptions: TurnOptions = {
  turnMetadata: publicRuntimeTurnMetadata,
};

void publicTurnOptions;

const publicTurnOptionsWithSplitMetadata: TurnOptions = {
  // @ts-expect-error turn metadata belongs inside turnMetadata.
  model: "claude-sonnet-4-6",
};

void publicTurnOptionsWithSplitMetadata;

const spawnSpec: SpawnSpec = {
  profile: "worker",
  agentIdentity: "worker-1",
};

void spawnSpec;

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
  turnMetadata: {
    additionalInstructions: ["stay focused"],
    connectionRef: { realm: "dev", binding: "default_anthropic" },
  },
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
};

void publicSpawnSpecWithAdvancedFields;

const publicSpawnManySpec: SpawnManySpec = {
  profile: "worker",
  agentIdentity: "worker-many",
  turnMetadata: {
    connectionRef: { realm: "dev", binding: "default_anthropic" },
  },
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
  turnMetadata: {
    skillReferences: [{ sourceUuid: "00000000-0000-4000-8000-000000000001", skillName: "read" }],
    flowToolOverlay: { allowedTools: ["read"], blockedTools: [] },
    additionalInstructions: ["stay concise"],
    keepAlive: true,
    model: "gpt-test",
    provider: "openai",
    providerParams: { temperature: 0.2 },
    clearProviderParams: true,
    connectionRef: { realm: "dev", binding: "default_openai" },
    clearConnectionRef: true,
  },
};

const publicMobTurnStartOptionsWithUnknown: MobTurnStartOptions = {
  // @ts-expect-error mob turn metadata belongs inside turnMetadata.
  model: "gpt-test",
};

type MobTurnStartSupportedWireOptionKeys = "turn_metadata";
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
  turn_metadata: {
    connection_ref: {
      action: "set",
      value: { realm: "dev", binding: "default_anthropic" },
    },
  },
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
  ok: true,
  agent_identity: "worker-3",
  member_ref: "opaque-member-ref",
};

const generatedMobSpawnManyResult: MobSpawnManyResult = {
  results: [generatedMobSpawnManyResultEntry],
};

void generatedMobSpawnManyParams;
void generatedMobSpawnManyResult;

const generatedMobTurnStart: MobTurnStartParams = {
  mob_id: "mob-1",
  agent_identity: "worker-1",
  prompt: [{ type: "text", text: "continue" }],
  turn_metadata: {
    model: "gpt-test",
    provider_params: { action: "clear" },
  },
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
