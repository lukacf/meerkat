import { subscribeTimelineWithRecovery } from "../../../packages/console-core/src/timeline-recovery";
import {
  answerUnderRowKeys,
  ConsoleMobNotLoadedError,
  ConsoleMemberDoorUnknownError,
  createConsoleMobAddressing,
  listedMobsOf,
  memberMobId,
  type ConsoleMobAddressing,
  type MemberDoor,
} from "../../../packages/console-core/src/mob-door";
import type { ConsoleTimelineSubscriptionOptions, ConsoleTransportState } from "../../../packages/console-core/src/timeline-subscription";
import {
  type ConsoleTopologyControlCapabilities,
  type ConsoleWorkbenchTarget,
  type MetaWorkbenchTarget,
} from "@console-core";
import {
  CONSOLE_BLOB_PATH_PREFIX,
  CONSOLE_HOST_SCOPED_RPC_METHODS,
  CONSOLE_MEMBER_PARAMS_BY_METHOD,
  CONSOLE_MOB_OPEN_METHOD,
  CONSOLE_MOBS_SELECTOR_EVERY,
  CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS,
  CONSOLE_MOBS_SELECTOR_PARAM,
  CONSOLE_REST_PATHS,
  CONSOLE_RPC_METHODS,
} from "./contract";
import {
  callConsoleRpc,
  callerScopeParam,
  consoleCallerUnknownFailure,
  fetchJson,
  mobIdParam,
  queryTimeline,
  sendConsole,
  sendConsoleMultipart,
  subscribeTimelineEvents,
  readConsoleBlob,
  uploadConsoleBlobMultipart,
} from "./network";
import type {
  ConsoleExperience,
  ConsoleFrame,
  ConsoleModulesResponse,
  ConsoleTimelineAccepted,
  ConsoleTimelinePage,
} from "../types";

export type ConsoleFactSource =
  | "meta-protocol"
  | "controller-derived"
  | "optimistic"
  | "host-adapter";

export interface ConsoleFact<T> {
  value: T;
  provenance: {
    source: ConsoleFactSource;
    contractVersion?: string;
    routeOrMethod?: string;
    cursor?: string;
    capabilityVersion?: string;
    timestampMs?: number;
    correlationId?: string;
  };
}

export interface ConsoleCapabilities {
  methods: string[];
  /** The feature names the gateway lists (`feature_capabilities`), when it lists any. */
  featureCapabilities?: string[];
  version?: string;
  readOnly?: boolean;
  runtime_capabilities?: unknown;
  method_capabilities?: unknown;
  topologyControl?: ConsoleTopologyControlCapabilities;
}

/** Who the console acts as for this connection (`meta/console/whoami`). */
export interface ConsoleWhoami {
  /** `person`, `service`, `token_principal` or `anonymous`; read others as not a person. */
  actorKind: string;
  principal?: string;
  approverIds: string[];
  canAdminister: boolean;
  callerScope?: string;
  loginExpiresAtMs?: number;
}

export interface ConsoleTimelineQueryInput {
  signal?: AbortSignal;
  /** A read the console makes on its own (a refresh, a repair after a gap
   * in the stream, a refresh after a frame), not a person's act. The stock
   * transport holds it back for a member whose mob is listed as not loaded
   * (`ConsoleMobNotLoadedError`): nothing is sent, so the mob stays not
   * loaded until someone asks for it. */
  automatic?: boolean;
  identity?: string;
  conversationId?: string;
  after?: string;
  before?: string;
  mode?: "since" | "recent";
  limit?: number;
}

export interface ConsoleTimelineSubscribeInput {
  signal?: AbortSignal;
  onTransportState?: (state: ConsoleTransportState) => void;
  identity?: string;
  conversationId?: string;
  after?: string;
  limit?: number;
}

export interface ConsoleSendInput {
  identity: string;
  content: string | Array<Record<string, unknown>>;
  origin: string;
  idempotencyKey: string;
  handlingMode?: "queue" | "steer";
  attachments?: File[];
  /** The caller the request is made for (`meta/console/whoami`'s
   * `caller_scope`), named as `expected_caller_scope`; `null` names none (a
   * connection without whoami). Absent: the transport names its own. */
  expectedCallerScope?: string | null;
}

export interface ConsoleUploadInput {
  blobId?: string;
  file?: File;
  mediaType?: string;
  /** As for a send: the caller the upload is made for. */
  expectedCallerScope?: string | null;
}

export interface ConsoleBlobReadInput {
  blobId: string;
  signal?: AbortSignal;
}

export interface ConsoleUploadResult {
  blob_id: string;
  url?: string;
}

export const CONSOLE_COMMAND_NAMES = {
  inspectIdentity: "inspectIdentity",
  retireIdentity: "retireIdentity",
  respawnIdentity: "respawnIdentity",
  resetIdentity: "resetIdentity",
  stopMemberRun: "stopMemberRun",
  forkMember: "forkMember",
  listSchedules: "listSchedules",
  createSchedule: "createSchedule",
  updateSchedule: "updateSchedule",
  pauseSchedule: "pauseSchedule",
  resumeSchedule: "resumeSchedule",
  deleteSchedule: "deleteSchedule",
  listRoutingRoutes: "listRoutingRoutes",
  listDeliveryHistory: "listDeliveryHistory",
  listGatingPending: "listGatingPending",
  listGatingAudit: "listGatingAudit",
  decideGating: "decideGating",
  listQuestionSets: "listQuestionSets",
  listDecisions: "listDecisions",
  getDecisionRendering: "getDecisionRendering",
  listConsoleAttention: "listConsoleAttention",
  dismissConsoleAttention: "dismissConsoleAttention",
  accessStatus: "accessStatus",
  getAccessConfig: "getAccessConfig",
  setAccessConfig: "setAccessConfig",
  enableAccess: "enableAccess",
  upsertAccessRule: "upsertAccessRule",
  deleteAccessRule: "deleteAccessRule",
  setAccessGroup: "setAccessGroup",
  deleteAccessGroup: "deleteAccessGroup",
  previewAccess: "previewAccess",
  listMemoryRecords: "listMemoryRecords",
  getMemoryRecord: "getMemoryRecord",
  listMemoryQuarantine: "listMemoryQuarantine",
  listMemoryDreams: "listMemoryDreams",
  getMemoryOverview: "getMemoryOverview",
  listMemoryProposals: "listMemoryProposals",
  listMemoryInjections: "listMemoryInjections",
  listMemoryHarvests: "listMemoryHarvests",
  listMemoryDreamRuns: "listMemoryDreamRuns",
  listMemoryAuditVerdicts: "listMemoryAuditVerdicts",
  decideMemoryQuarantine: "decideMemoryQuarantine",
  workgraphSnapshot: "workgraphSnapshot",
  workgraphEvents: "workgraphEvents",
  workgraphGet: "workgraphGet",
  workgraphGoalStatus: "workgraphGoalStatus",
  workgraphClaim: "workgraphClaim",
  workgraphRelease: "workgraphRelease",
  workgraphClose: "workgraphClose",
  workgraphGoalConfirm: "workgraphGoalConfirm",
  workgraphGoalRequestClose: "workgraphGoalRequestClose",
  workgraphAttentionPause: "workgraphAttentionPause",
  workgraphAttentionResume: "workgraphAttentionResume",
  workgraphAttentionReassign: "workgraphAttentionReassign",
  topologyQuery: "topologyQuery",
  topologyPlan: "topologyPlan",
  topologyApply: "topologyApply",
  topologyOperationGet: "topologyOperationGet",
  topologyAuditQuery: "topologyAuditQuery",
  topologyMobEvents: "topologyMobEvents",
  createAgent: "createAgent",
  updateAgent: "updateAgent",
  archiveAgent: "archiveAgent",
  unarchiveAgent: "unarchiveAgent",
  removeAgent: "removeAgent",
  withdrawAgentChange: "withdrawAgentChange",
  getAgentRecord: "getAgentRecord",
  listAgentRecords: "listAgentRecords",
  getAgentChange: "getAgentChange",
  listAgentDirectory: "listAgentDirectory",
  planAgentChange: "planAgentChange",
} as const;

export type ConsoleCommandName = typeof CONSOLE_COMMAND_NAMES[keyof typeof CONSOLE_COMMAND_NAMES];

type ConsoleCommandSpec = {
  method: typeof CONSOLE_RPC_METHODS[keyof typeof CONSOLE_RPC_METHODS];
  targetKinds: ReadonlySet<MetaWorkbenchTarget["kind"]>;
};

const LEGACY_INSPECT_IDENTITY_METHOD = "meta/inspect_identity";

/** What the name of every work graph method begins with. */
const WORKGRAPH_METHOD_PREFIX = "meta/workgraph/";

// The panes an agent's record is changed and read from: the roster and the
// agent's own chat and details.
const IDENTITY_CHANGE_TARGETS: ReadonlySet<MetaWorkbenchTarget["kind"]> = new Set<MetaWorkbenchTarget["kind"]>([
  "meta/roster",
  "meta/identity-chat",
  "meta/identity-inspect",
]);

const CONSOLE_COMMAND_SPECS: Record<ConsoleCommandName, ConsoleCommandSpec> = {
  [CONSOLE_COMMAND_NAMES.inspectIdentity]: {
    method: CONSOLE_RPC_METHODS.inspectIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.retireIdentity]: {
    method: CONSOLE_RPC_METHODS.retireIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.respawnIdentity]: {
    method: CONSOLE_RPC_METHODS.respawnIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.resetIdentity]: {
    method: CONSOLE_RPC_METHODS.resetIdentity,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  // Run-fenced Stop: params carry `member_id`, `run_id` and `reason`
  // explicitly (the run id comes from the member's `run_started`).
  [CONSOLE_COMMAND_NAMES.stopMemberRun]: {
    method: CONSOLE_RPC_METHODS.stopMemberRun,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  // Side chat: params carry `source_member_id` and the side chat's
  // `agent_identity` explicitly; the source is the conversation's agent.
  [CONSOLE_COMMAND_NAMES.forkMember]: {
    method: CONSOLE_RPC_METHODS.forkMember,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  // Console schedules: deferred sends to the target's agent; params
  // carry `identity` (filled from the target when absent).
  [CONSOLE_COMMAND_NAMES.listSchedules]: {
    method: CONSOLE_RPC_METHODS.scheduleList,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.createSchedule]: {
    method: CONSOLE_RPC_METHODS.scheduleCreate,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.updateSchedule]: {
    method: CONSOLE_RPC_METHODS.scheduleUpdate,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.pauseSchedule]: {
    method: CONSOLE_RPC_METHODS.schedulePause,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.resumeSchedule]: {
    method: CONSOLE_RPC_METHODS.scheduleResume,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.deleteSchedule]: {
    method: CONSOLE_RPC_METHODS.scheduleDelete,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>([
      "meta/identity-chat",
      "meta/identity-inspect",
    ]),
  },
  [CONSOLE_COMMAND_NAMES.listRoutingRoutes]: {
    method: CONSOLE_RPC_METHODS.routingRoutesList,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/routing"]),
  },
  [CONSOLE_COMMAND_NAMES.listDeliveryHistory]: {
    method: CONSOLE_RPC_METHODS.deliveryHistory,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/routing"]),
  },
  [CONSOLE_COMMAND_NAMES.listGatingPending]: {
    method: CONSOLE_RPC_METHODS.gatingPending,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.listGatingAudit]: {
    method: CONSOLE_RPC_METHODS.gatingAudit,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.decideGating]: {
    method: CONSOLE_RPC_METHODS.gatingDecide,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  // The open question sets agents asked; a set is answered or dismissed
  // through decideGating.
  [CONSOLE_COMMAND_NAMES.listQuestionSets]: {
    method: CONSOLE_RPC_METHODS.gatingQuestions,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.listDecisions]: {
    method: CONSOLE_RPC_METHODS.decisionsList,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.getDecisionRendering]: {
    method: CONSOLE_RPC_METHODS.decisionsRendering,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  // The caller's own attention items, listed in the inbox beside the
  // approvals.
  [CONSOLE_COMMAND_NAMES.listConsoleAttention]: {
    method: CONSOLE_RPC_METHODS.consoleAttention,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.dismissConsoleAttention]: {
    method: CONSOLE_RPC_METHODS.consoleAttentionDismiss,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/gating"]),
  },
  [CONSOLE_COMMAND_NAMES.accessStatus]: {
    method: CONSOLE_RPC_METHODS.accessStatus,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.getAccessConfig]: {
    method: CONSOLE_RPC_METHODS.accessGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.setAccessConfig]: {
    method: CONSOLE_RPC_METHODS.accessSet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.enableAccess]: {
    method: CONSOLE_RPC_METHODS.accessEnable,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.upsertAccessRule]: {
    method: CONSOLE_RPC_METHODS.accessRuleUpsert,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.deleteAccessRule]: {
    method: CONSOLE_RPC_METHODS.accessRuleDelete,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.setAccessGroup]: {
    method: CONSOLE_RPC_METHODS.accessGroupSet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.deleteAccessGroup]: {
    method: CONSOLE_RPC_METHODS.accessGroupDelete,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.previewAccess]: {
    method: CONSOLE_RPC_METHODS.accessPreview,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/access"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryRecords]: {
    method: CONSOLE_RPC_METHODS.memoryPanelRecords,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.getMemoryRecord]: {
    method: CONSOLE_RPC_METHODS.memoryPanelRecord,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryQuarantine]: {
    method: CONSOLE_RPC_METHODS.memoryPanelQuarantine,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryDreams]: {
    method: CONSOLE_RPC_METHODS.memoryPanelDreams,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.getMemoryOverview]: {
    method: CONSOLE_RPC_METHODS.memoryPanelOverview,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryProposals]: {
    method: CONSOLE_RPC_METHODS.memoryPanelProposals,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryInjections]: {
    method: CONSOLE_RPC_METHODS.memoryPanelInjections,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryHarvests]: {
    method: CONSOLE_RPC_METHODS.memoryPanelHarvests,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryDreamRuns]: {
    method: CONSOLE_RPC_METHODS.memoryPanelDreamRuns,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.listMemoryAuditVerdicts]: {
    method: CONSOLE_RPC_METHODS.memoryPanelAuditVerdicts,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.decideMemoryQuarantine]: {
    method: CONSOLE_RPC_METHODS.memoryQuarantineDecide,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/memory"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphSnapshot]: {
    method: CONSOLE_RPC_METHODS.workgraphSnapshot,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphEvents]: {
    method: CONSOLE_RPC_METHODS.workgraphEvents,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGet]: {
    method: CONSOLE_RPC_METHODS.workgraphGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalStatus]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalStatus,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphClaim]: {
    method: CONSOLE_RPC_METHODS.workgraphClaim,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphRelease]: {
    method: CONSOLE_RPC_METHODS.workgraphRelease,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphClose]: {
    method: CONSOLE_RPC_METHODS.workgraphClose,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalConfirm]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalConfirm,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphGoalRequestClose]: {
    method: CONSOLE_RPC_METHODS.workgraphGoalRequestClose,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionPause]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionPause,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionResume]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionResume,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.workgraphAttentionReassign]: {
    method: CONSOLE_RPC_METHODS.workgraphAttentionReassign,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/workgraph"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyQuery]: {
    method: CONSOLE_RPC_METHODS.topologyQuery,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyPlan]: {
    method: CONSOLE_RPC_METHODS.topologyPlan,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyApply]: {
    method: CONSOLE_RPC_METHODS.topologyApply,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyOperationGet]: {
    method: CONSOLE_RPC_METHODS.topologyOperationGet,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  [CONSOLE_COMMAND_NAMES.topologyAuditQuery]: {
    method: CONSOLE_RPC_METHODS.topologyAuditQuery,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  // The topology's typed lineage (member_spawned with its source member),
  // read only.
  [CONSOLE_COMMAND_NAMES.topologyMobEvents]: {
    method: CONSOLE_RPC_METHODS.mobEventsQuery,
    targetKinds: new Set<MetaWorkbenchTarget["kind"]>(["meta/topology"]),
  },
  // Changes and reads of the team's long-term agents. Params are the
  // family's own request, written as every surface writes it: a change
  // names its agent `name` and carries its `request_id`; nothing is filled
  // in from the target.
  [CONSOLE_COMMAND_NAMES.createAgent]: { method: CONSOLE_RPC_METHODS.identityCreate, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.updateAgent]: { method: CONSOLE_RPC_METHODS.identityUpdate, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.archiveAgent]: { method: CONSOLE_RPC_METHODS.identityArchive, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.unarchiveAgent]: { method: CONSOLE_RPC_METHODS.identityUnarchive, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.removeAgent]: { method: CONSOLE_RPC_METHODS.identityRemove, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.withdrawAgentChange]: { method: CONSOLE_RPC_METHODS.identityWithdraw, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.getAgentRecord]: { method: CONSOLE_RPC_METHODS.identityGet, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.listAgentRecords]: { method: CONSOLE_RPC_METHODS.identityList, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.getAgentChange]: { method: CONSOLE_RPC_METHODS.identityOperation, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.listAgentDirectory]: { method: CONSOLE_RPC_METHODS.identityDirectory, targetKinds: IDENTITY_CHANGE_TARGETS },
  [CONSOLE_COMMAND_NAMES.planAgentChange]: { method: CONSOLE_RPC_METHODS.identityPlan, targetKinds: IDENTITY_CHANGE_TARGETS },
};

/// The RPC method a console command dispatches to. Client-local frames
/// synthesized from operator RPC results stamp this so downstream folds can
/// label the action without re-deriving the mapping.
export function consoleCommandMethod(command: ConsoleCommandName): string {
  return CONSOLE_COMMAND_SPECS[command].method;
}

export interface ConsoleCommandRequest {
  signal?: AbortSignal;
  command: ConsoleCommandName;
  target: ConsoleWorkbenchTarget;
  params?: Record<string, unknown>;
  /** As for a send: the caller the command is made for. */
  expectedCallerScope?: string | null;
  /** As for a timeline read: a command the console makes on its own is held
   * back for a member whose mob is listed as not loaded. */
  automatic?: boolean;
  /**
   * The row the person acted on, by the console's key for it, where the
   * caller knows it. Its door is the command's door, whatever name the
   * params carry: a name taken from an answer is never read as a member
   * of the launch mob, and a command that names no member (a held change
   * by its request id, an agent's directory) still reaches that door.
   */
  about?: string;
  /**
   * The command acts on a value of the launch mob's door that names no
   * row (the docked work graph): it goes to that door, and is not sent
   * when its params name a member behind another door.
   */
  launchDoor?: boolean;
}

export interface ConsoleCommandResult {
  command: ConsoleCommandName;
  accepted: boolean;
  result?: unknown;
}

export interface MetaConsoleTransport {
  loadExperience(): Promise<ConsoleExperience>;
  loadModules?(): Promise<ConsoleModulesResponse>;
  capabilities(): Promise<ConsoleCapabilities>;
  /// Who the console acts as for this connection. Optional: a transport
  /// without it is never treated as a person, so approvals stay read-only.
  whoami?(): Promise<ConsoleWhoami>;
  queryTimeline(input: ConsoleTimelineQueryInput): Promise<ConsoleTimelinePage>;
  subscribeTimeline(
    input: ConsoleTimelineSubscribeInput,
    onFrame: (frame: ConsoleFrame) => void,
    options?: ConsoleTimelineSubscriptionOptions,
  ): () => void;
  send(input: ConsoleSendInput): Promise<ConsoleTimelineAccepted>;
  executeCommand?(input: ConsoleCommandRequest): Promise<ConsoleCommandResult>;
  upload?(input: ConsoleUploadInput): Promise<ConsoleUploadResult>;
  /// A person opened a mob the gateway lists as asleep: one ordinary request
  /// names it, and the mob wakes for it. Nothing is sent for a mob that has
  /// no door of its own.
  openMob?(mobId: string): Promise<void>;
  /// Reads a stored blob's bytes with this transport's own request
  /// authority, the credentials the host gives every Console request.
  /// The Console shows stored images from these bytes; a bare
  /// `/blobs/{id}` load carries no Authorization. Resolves to a Blob typed
  /// with its image media type. `blobUrl` stays a projection-only resolver.
  readBlob?(input: ConsoleBlobReadInput): Promise<Blob>;
  blobUrl?(blobId: string): string;
  /// What this connection knows of the gateway's mobs: whether a request may
  /// name a mob, and the door of each member. The stock transport keeps it
  /// and addresses every member through it; a request path that does not go
  /// through the transport reads the member's door here. A transport without
  /// it names no mob.
  mobAddressing?: ConsoleMobAddressing;
}

export interface MetaConsoleController {
  transport: MetaConsoleTransport;
  commands: ConsoleCommandSurface;
  timeline: ConsoleTimelineController;
  facts: {
    meta<T>(value: T, meta?: Partial<ConsoleFact<T>["provenance"]>): ConsoleFact<T>;
    derived<T>(value: T, meta?: Partial<ConsoleFact<T>["provenance"]>): ConsoleFact<T>;
    optimistic<T>(value: T, correlationId: string): ConsoleFact<T>;
    host<T>(value: T): ConsoleFact<T>;
  };
}

export interface ConsoleCommandSurface {
  sendMessage(
    target: ConsoleWorkbenchTarget,
    input: Omit<ConsoleSendInput, "identity">,
  ): Promise<{
    optimistic: ConsoleFact<{ idempotencyKey: string; targetId: string }>;
    accepted: ConsoleFact<ConsoleTimelineAccepted>;
  }>;
  uploadBlob(input: ConsoleUploadInput): Promise<ConsoleFact<ConsoleUploadResult>>;
  execute(input: ConsoleCommandRequest): Promise<ConsoleCommandResult>;
}

export interface ConsoleTimelineController {
  query(input: ConsoleTimelineQueryInput): Promise<ConsoleFact<ConsoleTimelinePage>>;
  subscribeWithBackfill(
    input: ConsoleTimelineSubscribeInput,
    onFrame: (frame: ConsoleFact<ConsoleFrame>) => void,
    /// Called after the controller re-seeds the stream past a replay gap.
    /// Consumers with identity-scoped cursors repair their own logs here.
    onReplayGap?: () => void,
  ): Promise<() => void>;
}

export function createHttpConsoleTransport({
  baseUrl,
  fetchTimeoutMs,
}: {
  baseUrl: string;
  fetchTimeoutMs?: number | (() => number);
}): MetaConsoleTransport {
  const timeout = () => typeof fetchTimeoutMs === "function" ? fetchTimeoutMs() : fetchTimeoutMs;
  // Who this transport acts as: the caller of its first answered
  // `meta/console/whoami` read. Every command, send and upload names it
  // (`expected_caller_scope`), so the gateway refuses a change made for
  // anyone else instead of making it as them. A later answer for another
  // caller (someone else signed in to this browser) never rebinds the
  // transport; `whoami()` still reads again and says who the gateway serves
  // now. The first change waits for a read when none has answered, and while
  // none has, the change is refused here (`caller_unknown`): nothing is sent.
  // A gateway without whoami (method not found) is not asked again by a
  // change: its changes name no caller. A request that names its caller
  // (`expectedCallerScope`, the console instance's own) names that one.
  let bound: ConsoleWhoami | undefined;
  let reading: Promise<ConsoleWhoami> | undefined;
  let unsupported = false;
  const whoami = (): Promise<ConsoleWhoami> => {
    const read = callConsoleRpc<unknown>(baseUrl, CONSOLE_RPC_METHODS.whoami, {}, timeout()).then(normalizeWhoami);
    reading = read;
    void read.then(
      (value) => { bound ??= value; },
      (error: unknown) => { if (isJsonRpcMethodNotFoundError(error)) unsupported = true; },
    ).finally(() => { if (reading === read) reading = undefined; });
    return read;
  };
  const callerScope = async (named: string | null | undefined): Promise<string | undefined> => {
    if (named !== undefined) return named ?? undefined;
    if (!bound && !unsupported) await (reading ?? whoami()).catch(() => undefined);
    if (bound) return bound.callerScope;
    if (unsupported) return undefined;
    throw consoleCallerUnknownFailure();
  };
  // What this connection knows of the gateway's mobs. It learns whether the
  // gateway judges a request's mob from the answers of the capability reads
  // the console makes anyway, and the door of each listed mob from the
  // experience. Until an answer lists the capability no request names a mob:
  // a gateway that hosts one mob is sent what it was always sent. From then
  // on a member is addressed by its door (the mob and the member as that
  // door knows it), the reads over the host's values ask for every mob the
  // caller may address, and every frame and acceptance is handed on under
  // the console's own key for its member.
  const mobs = createConsoleMobAddressing();
  const doorOf = (identity: string | undefined): MemberDoor | null => {
    const named = identity?.trim();
    return named ? mobs.door(named) : null;
  };
  return {
    loadExperience: async () => {
      const asked = mobs.enabled;
      const experience = await fetchJson<ConsoleExperience>(
        baseUrl,
        `${CONSOLE_REST_PATHS.experience}${asked ? `?${CONSOLE_MOBS_SELECTOR_PARAM}=${CONSOLE_MOBS_SELECTOR_EVERY}` : ""}`,
        timeout(),
      );
      // An answer to a read made before the capability was known lists the
      // launch mob alone: it never replaces the doors of a later answer.
      if (asked === mobs.enabled) {
        const listed = listedMobsOf(experience);
        mobs.noteMobs(listed.mobs, listed.primaryMobId, experience);
      }
      return experience;
    },
    loadModules: () => fetchJson<ConsoleModulesResponse>(baseUrl, CONSOLE_REST_PATHS.modules, timeout()),
    capabilities: async () => {
      let answer: unknown;
      try {
        answer = await callConsoleRpc<unknown>(baseUrl, CONSOLE_RPC_METHODS.capabilities, {}, timeout());
      } catch (error) {
        // A gateway that has no such method has answered: it hosts one mob.
        // Any other failure says nothing of what the gateway hosts.
        if (isJsonRpcMethodNotFoundError(error)) mobs.noteFeatures(null);
        throw error;
      }
      const capabilities = normalizeCapabilities(answer);
      mobs.noteFeatures(capabilities.featureCapabilities);
      return capabilities;
    },
    whoami,
    mobAddressing: mobs,
    openMob: async (mobId) => {
      const door = mobs.table.doors.get(mobId.trim());
      if (!door) return;
      await callConsoleRpc<unknown>(baseUrl, CONSOLE_MOB_OPEN_METHOD, mobIdParam(door), timeout());
    },
    queryTimeline: async (input) => {
      if (input.automatic && input.identity?.trim() && mobs.heldBack(input.identity.trim())) {
        throw new ConsoleMobNotLoadedError(input.identity.trim());
      }
      if (input.identity?.trim() && mobs.sharedKey(input.identity.trim())) {
        throw new ConsoleMemberDoorUnknownError(input.identity.trim(), CONSOLE_RPC_METHODS.queryTimeline);
      }
      const door = doorOf(input.identity);
      const page = await queryTimeline(
        baseUrl,
        door?.mobId ? { ...input, identity: door.identity, mobId: door.mobId } : { ...input, everyMob: !door && mobs.enabled },
        input.limit,
        timeout(),
        input.signal,
      );
      // A frame of a mob the list does not hold is not handed on.
      return mobs.enabled
        ? { ...page, frames: page.frames.flatMap((frame) => { const keyed = mobs.frame(frame, door?.mobId); return keyed ? [keyed] : []; }) }
        : page;
    },
    subscribeTimeline: (input, onFrame, options) => {
      const door = doorOf(input.identity);
      // A member's own stream is served for the launch mob's door only: the
      // host has one stream, whose frames name their mob. For a member
      // behind another door the one stream of every mob is opened (no mob
      // and no member in the request) and that member's frames are handed
      // on, under the console's key.
      const own = door?.mobId ? input.identity?.trim() : undefined;
      return subscribeTimelineEvents(
        baseUrl,
        own ? { ...input, identity: undefined, everyMob: () => mobs.enabled } : { ...input, everyMob: door ? false : () => mobs.enabled },
        (frame) => {
          const keyed = mobs.frame(frame);
          if (keyed && (!own || keyed.identity === own)) onFrame(keyed);
        },
        { signal: input.signal, onTransportState: input.onTransportState, ...options },
      );
    },
    send: async (input) => {
      const handlingMode = input.handlingMode ?? "queue";
      if (mobs.sharedKey(input.identity)) {
        throw new ConsoleMemberDoorUnknownError(input.identity, CONSOLE_RPC_METHODS.send);
      }
      const scope = await callerScope(input.expectedCallerScope);
      const door = mobs.door(input.identity);
      const mobId = door.mobId ?? undefined;
      const accepted = input.attachments?.length
        ? await sendConsoleMultipart(
          baseUrl,
          door.identity,
          input.content,
          input.attachments,
          input.origin,
          input.idempotencyKey,
          handlingMode,
          timeout(),
          scope,
          mobId,
        )
        : await sendConsole(
          baseUrl,
          door.identity,
          input.content,
          input.origin,
          input.idempotencyKey,
          handlingMode,
          timeout(),
          scope,
          mobId,
        );
      // The door answers with the member as it knows it: the console keeps
      // the acceptance under its own key for that member.
      return mobId
        ? { ...accepted, identity: mobs.rowKey({ mob_id: mobId, identity: accepted.identity }) ?? accepted.identity }
        : accepted;
    },
    executeCommand: async (input) => {
      const spec = commandSpec(input.command);
      let params: Record<string, unknown> = { ...(input.params || {}) };
      if (identityCommandMethods.has(spec.method)) {
        const identity = stringValue(params.identity) || identityForCommandTarget(input.target);
        if (!identity) {
          throw new Error(`${input.command} requires an identity-addressed target`);
        }
        params.identity = identity;
      }
      // A command about a member goes to the member's door: the mob, and the
      // member as that door knows it, wherever the contract says the
      // command's method names a member. A method of the host answers from
      // the host's values: it is sent no mob. A command the console makes on
      // its own is not made for a member whose mob is listed as not loaded.
      const hostMethod = hostScopedMethods.has(spec.method);
      const memberPaths = hostMethod ? [] : memberPathsByMethod.get(spec.method) ?? [];
      const members = memberPaths
        .flatMap((path) => valuesAtPath(params, path))
        .filter((name): name is string => typeof name === "string" && name.trim() !== "");
      // The word that names the member at the top of the params: the answer
      // names it by the same word.
      const memberParam = memberPaths.find((path) => path.length === 1 && typeof params[path[0]] === "string")?.[0] ?? null;
      // The row the person acted on, where the caller names it: its door is
      // the command's door.
      const about = !hostMethod && params.mob_id === undefined && typeof input.about === "string" && input.about ? input.about : null;
      // A command on a value of the launch mob's door that names no row is
      // that door's, as one sent for a row behind it is.
      // So is a work graph command that names no row: the graph read with
      // no row is that door's, and a work item is reassigned within the mob
      // whose graph holds it.
      const ofTheLaunchDoor = !hostMethod && params.mob_id === undefined
        && (input.launchDoor === true || (!about && spec.method.startsWith(WORKGRAPH_METHOD_PREFIX)));
      const aboutDoor: MemberDoor | null = about ? mobs.door(about) : ofTheLaunchDoor ? { mobId: null, identity: "" } : null;
      // The doors the command's members are behind. One request reaches one
      // door, and the launch mob's door is a door: a command whose members
      // are behind two is not sent. A member is behind the launch mob's
      // door when its row was read there, or its key names a listed mob
      // served through that door. Where the command is sent for a row of a
      // hosted mob's door, a bare name is the name that door's own answer
      // gave and stays that door's: only a key of a listed mob's member
      // counts as a member behind another door. A key two rows spell for
      // two members is addressed nowhere.
      const shared = [about, ...members].find((name) => name && mobs.sharedKey(name));
      if (params.mob_id === undefined && shared) {
        throw new ConsoleMemberDoorUnknownError(shared, spec.method);
      }
      const doors = new Set<string>();
      if (aboutDoor?.mobId) doors.add(aboutDoor.mobId);
      let behindTheLaunchDoor = aboutDoor !== null && aboutDoor.mobId === null;
      for (const member of members) {
        const own = mobs.door(member).mobId;
        if (own) doors.add(own);
        else if (aboutDoor?.mobId ? memberMobId(member, mobs.table) !== null : mobs.behindTheLaunchDoor(member)) behindTheLaunchDoor = true;
      }
      if (params.mob_id === undefined && (doors.size > 1 || (doors.size === 1 && behindTheLaunchDoor))) {
        throw new ConsoleMemberDoorUnknownError(about ?? members[0] ?? "", spec.method);
      }
      // Any other command that names a member of a mob with a door of its
      // own is not sent: the contract names no member for its method (or
      // the command carries none of the method's member words), and sent as
      // it is it would be answered for a launch member of the same name.
      if (!hostMethod && members.length === 0 && !aboutDoor?.mobId && params.mob_id === undefined && typeof params.identity === "string" && mobs.door(params.identity).mobId) {
        throw new ConsoleMemberDoorUnknownError(params.identity, spec.method);
      }
      if (input.automatic) {
        const held = members.find((member) => mobs.heldBack(member)) ?? (about && mobs.heldBack(about) ? about : undefined);
        if (held) throw new ConsoleMobNotLoadedError(held);
      }
      let doorMobId: string | null = null;
      const doorMob = params.mob_id === undefined ? aboutDoor?.mobId ?? [...doors][0] ?? null : null;
      if (doorMob) {
        // Each member the command names is written as that door knows it:
        // the console's key of a member behind this door is rewritten, a
        // bare name (as the door's own answer gives it) is left as the
        // door's.
        const asTheDoorKnowsIt = (name: string): string => {
          const own = mobs.door(name);
          return own.mobId === doorMob ? own.identity : name;
        };
        for (const path of memberPaths) params = mapAtPath(params, path, asTheDoorKnowsIt) as Record<string, unknown>;
        params = { ...params, ...mobIdParam(doorMob) };
        doorMobId = doorMob;
      }
      // A list of the host that names agents answers, of the entries agents
      // made, those of the launch mob's agents alone unless it is asked for
      // every mob. It is asked so wherever the gateway judges a request's
      // mob; elsewhere the request is the one it always was.
      if (everyMobHostLists.has(spec.method) && params[CONSOLE_MOBS_SELECTOR_PARAM] === undefined) {
        Object.assign(params, mobs.everyMob());
      }
      Object.assign(params, callerScopeParam(await callerScope(input.expectedCallerScope)));
      input.signal?.throwIfAborted();
      let result: unknown;
      try {
        result = await callConsoleRpc<unknown>(baseUrl, spec.method, params, timeout(), input.signal);
      } catch (error) {
        if (
          spec.method !== CONSOLE_RPC_METHODS.inspectIdentity ||
          !isJsonRpcMethodNotFoundError(error)
        ) {
          throw error;
        }
        result = await callConsoleRpc<unknown>(baseUrl, LEGACY_INSPECT_IDENTITY_METHOD, params, timeout(), input.signal);
      }
      return {
        command: input.command,
        accepted: true,
        // The door answered with its members as it knows them.
        result: doorMobId ? answerUnderRowKeys(result, doorMobId, mobs.table, memberParam) : result,
      };
    },
    upload: async (input) => uploadConsoleBlobMultipart(baseUrl, input, timeout(), await callerScope(input.expectedCallerScope)),
    readBlob: (input) => readConsoleBlob(baseUrl, input.blobId, timeout(), input.signal),
    blobUrl: (blobId) => `${baseUrl}${CONSOLE_BLOB_PATH_PREFIX}${encodeURIComponent(blobId)}`,
  };
}

export function createMetaConsoleController({
  transport,
}: {
  transport: MetaConsoleTransport;
}): MetaConsoleController {
  const facts = createFactFactory();
  return {
    transport,
    facts,
    timeline: createTimelineController(transport, facts),
    commands: createConsoleCommandSurface(transport, facts),
  };
}

function isJsonRpcMethodNotFoundError(error: unknown): boolean {
  const rpcError = (error as { rpcError?: { code?: unknown } } | null)?.rpcError;
  return rpcError?.code === -32601;
}

function createConsoleCommandSurface(
  transport: MetaConsoleTransport,
  facts: MetaConsoleController["facts"],
): ConsoleCommandSurface {
  let cachedCapabilities: ConsoleCapabilities | null = null;
  let capabilitiesRequest: Promise<ConsoleCapabilities> | null = null;
  const capabilities = async (force = false) => {
    if (force || !cachedCapabilities) {
      if (!capabilitiesRequest) {
        capabilitiesRequest = transport.capabilities().finally(() => {
          capabilitiesRequest = null;
        });
      }
      cachedCapabilities = await capabilitiesRequest;
    }
    return cachedCapabilities;
  };
  const requireFreshCapability = async (method: string) => {
    let currentCapabilities = await capabilities(true);
    if (!hasCapability(currentCapabilities, method)) {
      currentCapabilities = await capabilities(true);
    }
    requireCapability(currentCapabilities, method);
    return currentCapabilities;
  };
  return {
    async sendMessage(target, input) {
      const identity = identityForSendTarget(target);
      if (!identity) {
        throw new Error(`target ${target.kind} cannot send Meta console messages`);
      }
      const currentCapabilities = await requireFreshCapability(CONSOLE_RPC_METHODS.send);
      const optimistic = facts.optimistic({
        idempotencyKey: input.idempotencyKey,
        targetId: target.id,
      }, input.idempotencyKey);
      const accepted = await transport.send({
        ...input,
        identity,
      });
      return {
        optimistic,
        accepted: facts.meta(accepted, {
          routeOrMethod: CONSOLE_RPC_METHODS.send,
          capabilityVersion: currentCapabilities.version,
          correlationId: input.idempotencyKey,
          cursor: accepted.cursor,
        }),
      };
    },
    async uploadBlob(input) {
      const currentCapabilities = await requireFreshCapability(CONSOLE_RPC_METHODS.blobUpload);
      if (!transport.upload) {
        throw new Error(`transport does not implement ${CONSOLE_RPC_METHODS.blobUpload}`);
      }
      const uploaded = await transport.upload(input);
      return facts.meta(uploaded, {
        routeOrMethod: CONSOLE_RPC_METHODS.blobUpload,
        capabilityVersion: currentCapabilities.version,
      });
    },
    async execute(input) {
      input.signal?.throwIfAborted();
      if (!isMetaTarget(input.target)) {
        throw new Error(`host target ${input.target.kind} cannot execute Meta commands`);
      }
      const spec = commandSpec(input.command);
      if (!spec.targetKinds.has(input.target.kind)) {
        throw new Error(`target ${input.target.kind} cannot execute command ${input.command}`);
      }
      await requireFreshCapability(spec.method);
      input.signal?.throwIfAborted();
      if (!transport.executeCommand) {
        throw new Error(`transport does not implement command ${input.command}`);
      }
      return transport.executeCommand(input);
    },
  };
}

function createTimelineController(
  transport: MetaConsoleTransport,
  facts: MetaConsoleController["facts"],
): ConsoleTimelineController {
  return {
    async query(input) {
      const page = await transport.queryTimeline(input);
      return facts.meta(page, {
        routeOrMethod: CONSOLE_RPC_METHODS.queryTimeline,
        cursor: page.latestCursor || page.nextCursor,
      });
    },
    subscribeWithBackfill(input, onFrame, onReplayGap) {
      return subscribeTimelineWithRecovery(transport, input, (frame) => {
        return onFrame(facts.meta(frame, {
          routeOrMethod: CONSOLE_REST_PATHS.timelineStream,
          cursor: frame.cursor,
        }));
      }, onReplayGap);
    },
  };
}

function createFactFactory(): MetaConsoleController["facts"] {
  const wrap = <T>(
    source: ConsoleFactSource,
    value: T,
    meta: Partial<ConsoleFact<T>["provenance"]> = {},
  ): ConsoleFact<T> => ({
    value,
    provenance: {
      source,
      timestampMs: Date.now(),
      ...meta,
    },
  });
  return {
    meta: (value, meta) => wrap("meta-protocol", value, meta),
    derived: (value, meta) => wrap("controller-derived", value, meta),
    optimistic: (value, correlationId) => wrap("optimistic", value, { correlationId }),
    host: (value) => wrap("host-adapter", value),
  };
}

function normalizeCapabilities(value: unknown): ConsoleCapabilities {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const methods = Array.isArray(record.methods)
    ? Array.from(new Set(record.methods.filter((method): method is string => typeof method === "string" && method.trim().length > 0)))
    : [];
  const features = Array.isArray(record.feature_capabilities)
    ? record.feature_capabilities.filter((name): name is string => typeof name === "string" && name.length > 0)
    : null;
  return {
    methods,
    ...(features ? { featureCapabilities: features } : {}),
    version: typeof record.version === "string" ? record.version : undefined,
    ...(typeof record.read_only === "boolean" ? { readOnly: record.read_only } : {}),
    runtime_capabilities: record.runtime_capabilities,
    method_capabilities: record.method_capabilities,
    ...(record.topology_control && typeof record.topology_control === "object"
      ? { topologyControl: record.topology_control as ConsoleTopologyControlCapabilities }
      : {}),
  };
}

/** A `meta/console/whoami` result; anything malformed reads as not a person. */
export function normalizeWhoami(value: unknown): ConsoleWhoami {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const actor = record.actor && typeof record.actor === "object" ? record.actor as Record<string, unknown> : {};
  const text = (field: unknown) => typeof field === "string" && field.trim() ? field : undefined;
  const expiresAt = record.login_expires_at_ms;
  return {
    actorKind: text(actor.kind) ?? "anonymous",
    ...(text(actor.principal) ? { principal: text(actor.principal) } : {}),
    approverIds: Array.isArray(record.approver_ids)
      ? record.approver_ids.filter((id): id is string => typeof id === "string" && id.trim().length > 0)
      : [],
    canAdminister: record.can_administer === true,
    ...(text(record.caller_scope) ? { callerScope: text(record.caller_scope) } : {}),
    ...(typeof expiresAt === "number" && Number.isFinite(expiresAt) ? { loginExpiresAtMs: expiresAt } : {}),
  };
}

/** Fresh capability evidence, distinct from an unavailable network response. */
export class ConsoleCapabilityUnavailableError extends Error {
  readonly kind = "console-capability-unavailable";
  readonly availableMethods: readonly string[];
  constructor(readonly method: string, capabilities: ConsoleCapabilities) {
    super(`Meta capability missing for ${method}`);
    this.name = "ConsoleCapabilityUnavailableError";
    this.availableMethods = [...capabilities.methods];
  }
}

function requireCapability(capabilities: ConsoleCapabilities, method: string) {
  if (!hasCapability(capabilities, method)) {
    throw new ConsoleCapabilityUnavailableError(method, capabilities);
  }
}

function hasCapability(capabilities: ConsoleCapabilities, method: string): boolean {
  return capabilities.methods.includes(method);
}

function commandSpec(command: ConsoleCommandName): ConsoleCommandSpec {
  if (!isConsoleCommandName(command)) {
    throw new Error(`unknown Meta console command ${String(command)}`);
  }
  return CONSOLE_COMMAND_SPECS[command];
}

function isConsoleCommandName(command: unknown): command is ConsoleCommandName {
  return typeof command === "string" && command in CONSOLE_COMMAND_SPECS;
}

function identityForSendTarget(target: ConsoleWorkbenchTarget): string | null {
  return target.kind === "meta/identity-chat" ? target.identity : null;
}

/** The methods of the host (contract v0.6.0): they take no mob as a selector of what they answer. */
const hostScopedMethods = new Set<string>(CONSOLE_HOST_SCOPED_RPC_METHODS);

/** The lists of the host that name agents (contract v0.6.0): each takes the word for every mob. */
const everyMobHostLists = new Set<string>(CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS);

/**
 * Where each method names a member of the mob its request is for (contract
 * v0.6.0, CONSOLE_MEMBER_PARAMS_BY_METHOD): a path into the params, `*` for
 * every entry of a list. Only these are written as a member's door knows
 * them. A method that is not here names no member: it takes the mob alone
 * (a held change by its request id, the directory of an agent's mob). The
 * console's own "New agent" names its agents in `agents[]` by plain names,
 * which no door is found for: it creates in the launch mob.
 */
const memberPathsByMethod = new Map<string, readonly string[][]>(
  Object.entries(CONSOLE_MEMBER_PARAMS_BY_METHOD).map(([method, paths]) => [method, paths.map((path) => path.split("."))]),
);

/** The values at a path of `value`; `*` stands for every entry of a list. */
function valuesAtPath(value: unknown, path: readonly string[]): unknown[] {
  if (path.length === 0) return [value];
  const [head, ...rest] = path;
  if (head === "*") return Array.isArray(value) ? value.flatMap((entry) => valuesAtPath(entry, rest)) : [];
  if (!value || typeof value !== "object" || Array.isArray(value)) return [];
  return valuesAtPath((value as Record<string, unknown>)[head], rest);
}

/** `value` with every string at a path replaced by `map` of it. What is on the way is copied; the caller's objects are left as they are. */
function mapAtPath(value: unknown, path: readonly string[], map: (name: string) => string): unknown {
  if (path.length === 0) return typeof value === "string" ? map(value) : value;
  const [head, ...rest] = path;
  if (head === "*") return Array.isArray(value) ? value.map((entry) => mapAtPath(entry, rest, map)) : value;
  if (!value || typeof value !== "object" || Array.isArray(value)) return value;
  const fields = value as Record<string, unknown>;
  return head in fields ? { ...fields, [head]: mapAtPath(fields[head], rest, map) } : value;
}

const identityCommandMethods = new Set<string>([
  CONSOLE_RPC_METHODS.inspectIdentity,
  CONSOLE_RPC_METHODS.retireIdentity,
  CONSOLE_RPC_METHODS.respawnIdentity,
  CONSOLE_RPC_METHODS.resetIdentity,
  CONSOLE_RPC_METHODS.scheduleList,
  CONSOLE_RPC_METHODS.scheduleCreate,
  CONSOLE_RPC_METHODS.scheduleUpdate,
  CONSOLE_RPC_METHODS.schedulePause,
  CONSOLE_RPC_METHODS.scheduleResume,
  CONSOLE_RPC_METHODS.scheduleDelete,
]);

function identityForCommandTarget(target: ConsoleWorkbenchTarget): string | null {
  if (target.kind === "meta/identity-chat" || target.kind === "meta/identity-inspect") {
    return target.identity;
  }
  return null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function isMetaTarget(target: ConsoleWorkbenchTarget): target is MetaWorkbenchTarget {
  return target.kind.startsWith("meta/");
}
