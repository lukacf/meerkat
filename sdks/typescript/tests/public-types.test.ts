import type { SpawnSpec } from "../src/index.js";
import type {
  MobEnsureMemberResult,
  MobMemberListEntryWire,
  MobMembersResult,
  MobMemberStatusResult,
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
  WireToolAccessPolicy,
  WireToolFilter,
} from "../src/generated/types.js";

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

const generatedMobSpawn: MobSpawnParams = {
  mob_id: "mob-1",
  profile: "worker",
  agent_identity: "worker-1",
};

void generatedMobSpawn;

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

void generatedMobLaunchMode;
void generatedMobToolAccess;
void generatedMobBudgetSplit;
void generatedMobToolFilter;
void generatedMobProfile;

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
