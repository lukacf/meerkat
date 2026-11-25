export const CONSOLE_CONTRACT_VERSION = "0.6.0";

export const CONSOLE_REST_PATHS = {
  experience: "/console/experience",
  modules: "/console/modules",
  identities: "/console/identities",
  timeline: "/console/timeline",
  timelineStream: "/console/timeline/stream",
  identityTimelineStreamTemplate: "/console/identity/{identity}/stream",
  gatingStream: "/console/gating/stream",
  legacySend: "/console/send",
  sendRecover: "/console/send/recover",
  mcpAppsResolve: "/console/mcp-apps/resolve",
  mcpAppsReadResource: "/console/mcp-apps/read-resource",
  mcpAppsCallTool: "/console/mcp-apps/call-tool",
} as const;

export const CONSOLE_RPC_PATHS = {
  jsonRpc: "/console/rpc",
  multipartJsonRpc: "/console/rpc/multipart",
} as const;

export const CONSOLE_RPC_METHODS = {
  capabilities: "meta/capabilities",
  status: "meta/status",
  whoami: "meta/console/whoami",
  send: "meta/console/send",
  recoverSend: "meta/console/recover_send",
  listIdentities: "meta/console/list_identities",
  inspectIdentity: "meta/console/inspect_identity",
  queryTimeline: "meta/console/query_timeline",
  blobUpload: "meta/blob/upload",
  retireIdentity: "meta/retire",
  respawnIdentity: "meta/respawn",
  resetIdentity: "meta/reset",
  stopMemberRun: "meta/stop_member_run",
  forkMember: "meta/fork_member",
  scheduleList: "meta/schedule/list",
  scheduleCreate: "meta/schedule/create",
  scheduleUpdate: "meta/schedule/update",
  schedulePause: "meta/schedule/pause",
  scheduleResume: "meta/schedule/resume",
  scheduleDelete: "meta/schedule/delete",
  routingRoutesList: "meta/routing/routes/list",
  deliveryHistory: "meta/delivery/history",
  gatingPending: "meta/gating/pending",
  gatingAudit: "meta/gating/audit",
  gatingDecide: "meta/gating/decide",
  gatingQuestions: "meta/gating/questions",
  decisionsList: "meta/decisions/list",
  decisionsRendering: "meta/decisions/rendering",
  consoleAttention: "meta/console/attention",
  consoleAttentionDismiss: "meta/console/attention/dismiss",
  consoleAudit: "meta/console/audit",
  accessStatus: "meta/access/status",
  accessGet: "meta/access/get",
  accessSet: "meta/access/set",
  accessEnable: "meta/access/enable",
  accessRuleUpsert: "meta/access/rules/upsert",
  accessRuleDelete: "meta/access/rules/delete",
  accessGroupSet: "meta/access/groups/set",
  accessGroupDelete: "meta/access/groups/delete",
  accessPreview: "meta/access/preview",
  memoryPanelRecords: "meta/memory/panel/records",
  memoryPanelRecord: "meta/memory/panel/record",
  memoryPanelQuarantine: "meta/memory/panel/quarantine",
  memoryPanelDreams: "meta/memory/panel/dreams",
  memoryPanelOverview: "meta/memory/panel/overview",
  memoryPanelProposals: "meta/memory/panel/proposals",
  memoryPanelInjections: "meta/memory/panel/injections",
  memoryPanelHarvests: "meta/memory/panel/harvests",
  memoryPanelDreamRuns: "meta/memory/panel/dream_runs",
  memoryPanelAuditVerdicts: "meta/memory/panel/audit_verdicts",
  memoryQuarantineDecide: "meta/memory/quarantine/decide",
  workgraphSnapshot: "meta/workgraph/snapshot",
  workgraphGet: "meta/workgraph/get",
  workgraphEvents: "meta/workgraph/events",
  workgraphGoalStatus: "meta/workgraph/goal/status",
  workgraphClaim: "meta/workgraph/claim",
  workgraphRelease: "meta/workgraph/release",
  workgraphClose: "meta/workgraph/close",
  workgraphGoalConfirm: "meta/workgraph/goal/confirm",
  workgraphGoalRequestClose: "meta/workgraph/goal/request_close",
  workgraphAttentionPause: "meta/workgraph/attention/pause",
  workgraphAttentionResume: "meta/workgraph/attention/resume",
  workgraphAttentionReassign: "meta/workgraph/attention/reassign",
  topologyQuery: "meta/topology/query",
  topologyPlan: "meta/topology/plan",
  topologyApply: "meta/topology/apply",
  topologyOperationGet: "meta/topology/operation/get",
  topologyAuditQuery: "meta/topology/audit/query",
  mobEventsQuery: "meta/mob_events/query",
  identityCreate: "meta/identity/create",
  identityUpdate: "meta/identity/update",
  identityArchive: "meta/identity/archive",
  identityUnarchive: "meta/identity/unarchive",
  identityRemove: "meta/identity/remove",
  identityWithdraw: "meta/identity/withdraw",
  identityGet: "meta/identity/get",
  identityList: "meta/identity/list",
  identityOperation: "meta/identity/operation",
  identityDirectory: "meta/identity/directory",
  identityAdopt: "meta/identity/adopt",
  identityChangesHold: "meta/identity/changes_hold",
  identityPlan: "meta/identity/plan",
  identitySettlePreparation: "meta/identity/settle_preparation",
} as const;

export const CONSOLE_BLOB_PATH_PREFIX = "/blobs/";

export const CONSOLE_TIMELINE_QUERY_MODES = ["since", "recent"] as const;

export const CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE = -32013;

/**
 * The mob of a request (contract v0.6.0, `mob_addressing`).
 *
 * A gateway that hosts more than one mob lists this name in
 * `meta/capabilities` `feature_capabilities`. A client sends `mob_id` or
 * `mobs` only to such a gateway: one that does not list it answers for its
 * one mob whatever a request names.
 */
export const CONSOLE_MOB_ADDRESSING_CAPABILITY = "mob_addressing";

/** The member, query parameter or `params` member that names a request's mob. */
export const CONSOLE_MOB_ID_PARAM = "mob_id";

/** The selector of the three reads over every mob the caller may address. */
export const CONSOLE_MOBS_SELECTOR_PARAM = "mobs";

/** The selector's one value. Closed: a gateway refuses every other value. */
export const CONSOLE_MOBS_SELECTOR_EVERY = "*";

/**
 * `error` (on a route, HTTP 404) and `data.kind` (on a request door,
 * `-32602`) of the one refusal of a mob a door does not serve to its
 * caller. Closed: the contract has no second refusal of a mob id.
 */
export const CONSOLE_MOB_REFUSAL_KIND = "foreign_mob_target";

/** The routes that take `mob_id` as a query parameter. */
export const CONSOLE_MOB_ID_QUERY_ROUTES = [
  CONSOLE_REST_PATHS.experience,
  CONSOLE_REST_PATHS.modules,
  CONSOLE_REST_PATHS.identities,
  CONSOLE_REST_PATHS.timeline,
  CONSOLE_REST_PATHS.timelineStream,
  CONSOLE_REST_PATHS.identityTimelineStreamTemplate,
  CONSOLE_REST_PATHS.gatingStream,
] as const;

/** The routes that take `mob_id` as a member of their JSON body. */
export const CONSOLE_MOB_ID_BODY_ROUTES = [
  CONSOLE_REST_PATHS.legacySend,
  CONSOLE_REST_PATHS.sendRecover,
  CONSOLE_REST_PATHS.mcpAppsResolve,
  CONSOLE_REST_PATHS.mcpAppsReadResource,
  CONSOLE_REST_PATHS.mcpAppsCallTool,
] as const;

/** The routes that take `mobs=*`. `timeline` takes it only without `identity`. */
export const CONSOLE_MOBS_SELECTOR_ROUTES = [
  CONSOLE_REST_PATHS.experience,
  CONSOLE_REST_PATHS.timeline,
  CONSOLE_REST_PATHS.timelineStream,
] as const;

/** The methods that take `mobs: "*"`, only without `identity`. */
export const CONSOLE_MOBS_SELECTOR_METHODS = [
  CONSOLE_RPC_METHODS.queryTimeline,
] as const;

/**
 * The methods of the host: they answer from the host's values and take no
 * mob as a selector of what they answer. A method of
 * {@link CONSOLE_RPC_METHODS} that is neither here nor in
 * {@link CONSOLE_PER_MOB_RPC_METHODS} is one the request doors serve for
 * the launch mob alone.
 */
export const CONSOLE_HOST_SCOPED_RPC_METHODS = [
  CONSOLE_RPC_METHODS.accessEnable,
  CONSOLE_RPC_METHODS.accessGet,
  CONSOLE_RPC_METHODS.accessGroupDelete,
  CONSOLE_RPC_METHODS.accessGroupSet,
  CONSOLE_RPC_METHODS.accessPreview,
  CONSOLE_RPC_METHODS.accessRuleDelete,
  CONSOLE_RPC_METHODS.accessRuleUpsert,
  CONSOLE_RPC_METHODS.accessSet,
  CONSOLE_RPC_METHODS.accessStatus,
  CONSOLE_RPC_METHODS.blobUpload,
  CONSOLE_RPC_METHODS.consoleAttention,
  CONSOLE_RPC_METHODS.consoleAttentionDismiss,
  CONSOLE_RPC_METHODS.consoleAudit,
  CONSOLE_RPC_METHODS.whoami,
  CONSOLE_RPC_METHODS.decisionsList,
  CONSOLE_RPC_METHODS.decisionsRendering,
  CONSOLE_RPC_METHODS.gatingAudit,
  CONSOLE_RPC_METHODS.gatingDecide,
  CONSOLE_RPC_METHODS.gatingPending,
  CONSOLE_RPC_METHODS.gatingQuestions,
] as const;

/**
 * The methods of scope `one_mob` in the contract document: the methods of
 * this contract that the request doors serve for a mob other than the
 * launch mob. The doors serve further methods for the mob a request names
 * that the document does not list; the list is the contract's, not the
 * doors' whole list. A method of {@link CONSOLE_RPC_METHODS} that is
 * neither here nor a method of the host
 * ({@link CONSOLE_HOST_SCOPED_RPC_METHODS}) is one the doors serve for the
 * launch mob alone: a request on it that names another mob is refused. The
 * transport names a request's mob by {@link CONSOLE_MEMBER_PARAMS_BY_METHOD},
 * which holds some of those methods; the door refuses them for another mob.
 * A method joins the list with the contract change that gives it the scope.
 */
export const CONSOLE_PER_MOB_RPC_METHODS = [
  CONSOLE_RPC_METHODS.status,
  CONSOLE_RPC_METHODS.inspectIdentity,
  CONSOLE_RPC_METHODS.queryTimeline,
  CONSOLE_RPC_METHODS.send,
  CONSOLE_RPC_METHODS.recoverSend,
  CONSOLE_RPC_METHODS.stopMemberRun,
  CONSOLE_RPC_METHODS.scheduleList,
  CONSOLE_RPC_METHODS.scheduleCreate,
  CONSOLE_RPC_METHODS.scheduleUpdate,
  CONSOLE_RPC_METHODS.schedulePause,
  CONSOLE_RPC_METHODS.scheduleResume,
  CONSOLE_RPC_METHODS.scheduleDelete,
  CONSOLE_RPC_METHODS.deliveryHistory,
  CONSOLE_RPC_METHODS.workgraphSnapshot,
  CONSOLE_RPC_METHODS.workgraphGet,
  CONSOLE_RPC_METHODS.workgraphEvents,
  CONSOLE_RPC_METHODS.workgraphGoalStatus,
  CONSOLE_RPC_METHODS.workgraphClaim,
  CONSOLE_RPC_METHODS.workgraphRelease,
  CONSOLE_RPC_METHODS.workgraphClose,
  CONSOLE_RPC_METHODS.workgraphGoalConfirm,
  CONSOLE_RPC_METHODS.workgraphGoalRequestClose,
  CONSOLE_RPC_METHODS.workgraphAttentionPause,
  CONSOLE_RPC_METHODS.workgraphAttentionResume,
  CONSOLE_RPC_METHODS.workgraphAttentionReassign,
  CONSOLE_RPC_METHODS.topologyQuery,
  CONSOLE_RPC_METHODS.topologyPlan,
  CONSOLE_RPC_METHODS.topologyApply,
  CONSOLE_RPC_METHODS.topologyOperationGet,
  CONSOLE_RPC_METHODS.topologyAuditQuery,
  CONSOLE_RPC_METHODS.mobEventsQuery,
  CONSOLE_RPC_METHODS.identityCreate,
  CONSOLE_RPC_METHODS.identityUpdate,
  CONSOLE_RPC_METHODS.identityArchive,
  CONSOLE_RPC_METHODS.identityUnarchive,
  CONSOLE_RPC_METHODS.identityRemove,
  CONSOLE_RPC_METHODS.identityWithdraw,
  CONSOLE_RPC_METHODS.identityGet,
  CONSOLE_RPC_METHODS.identityList,
  CONSOLE_RPC_METHODS.identityOperation,
  CONSOLE_RPC_METHODS.identityDirectory,
  CONSOLE_RPC_METHODS.identityAdopt,
  CONSOLE_RPC_METHODS.identityChangesHold,
  CONSOLE_RPC_METHODS.identityPlan,
] as const;

/**
 * The states of a listed mob (`topology.live_snapshot.mobs[].state`).
 * Tolerant: a reader reads a value it does not know as `unknown`.
 */
export const CONSOLE_MOB_STATES = [
  "running",
  "stopped",
  "dormant",
  "unavailable",
  "unknown",
] as const;

/**
 * How a listed mob came to be (`topology.live_snapshot.mobs[].provenance`).
 * Tolerant: an open list. A reader takes a mob's door from `default` and
 * the word `host` alone.
 */
export const CONSOLE_MOB_PROVENANCES = ["host", "member"] as const;

/** The members of a listed mob. `provenance`, `default` and `incarnation` are new in v0.6.0. */
export const CONSOLE_LISTED_MOB_FIELDS = [
  "mob_id",
  "parent_mob_id",
  "host_identity",
  "state",
  "member_count",
  "provenance",
  "default",
  "incarnation",
] as const;

/**
 * The request a console opens a mob with (contract v0.6.0, `opening`). There
 * is no load word: an ordinary request that names `mob_id` loads a mob that
 * is not loaded. A client sends no request of its own accord that names a
 * mob listed `dormant` or `unavailable`.
 */
export const CONSOLE_MOB_OPEN_METHOD = CONSOLE_RPC_METHODS.status;

/**
 * The members of a method's params that name a member of the mob the
 * request is for (a dotted path; `*` stands for every entry). On a request
 * to a hosted mob's door each carries the member's name as that door knows
 * it. A method that serves one mob and is not listed names no member and
 * takes `mob_id` alone.
 */
export const CONSOLE_MEMBER_PARAMS_BY_METHOD: Readonly<Record<string, readonly string[]>> = {
  [CONSOLE_RPC_METHODS.send]: ["identity"],
  [CONSOLE_RPC_METHODS.recoverSend]: ["identity"],
  [CONSOLE_RPC_METHODS.inspectIdentity]: ["identity"],
  [CONSOLE_RPC_METHODS.queryTimeline]: ["identity"],
  [CONSOLE_RPC_METHODS.retireIdentity]: ["identity"],
  [CONSOLE_RPC_METHODS.respawnIdentity]: ["identity"],
  [CONSOLE_RPC_METHODS.resetIdentity]: ["identity"],
  [CONSOLE_RPC_METHODS.stopMemberRun]: ["member_id"],
  [CONSOLE_RPC_METHODS.forkMember]: ["source_member_id", "agent_identity"],
  [CONSOLE_RPC_METHODS.scheduleList]: ["identity"],
  [CONSOLE_RPC_METHODS.scheduleCreate]: ["identity"],
  [CONSOLE_RPC_METHODS.scheduleUpdate]: ["identity"],
  [CONSOLE_RPC_METHODS.schedulePause]: ["identity"],
  [CONSOLE_RPC_METHODS.scheduleResume]: ["identity"],
  [CONSOLE_RPC_METHODS.scheduleDelete]: ["identity"],
  [CONSOLE_RPC_METHODS.memoryPanelRecords]: ["identity"],
  [CONSOLE_RPC_METHODS.memoryQuarantineDecide]: ["identity"],
  [CONSOLE_RPC_METHODS.mobEventsQuery]: ["identity", "member_id"],
  [CONSOLE_RPC_METHODS.workgraphAttentionReassign]: ["target.identity"],
  [CONSOLE_RPC_METHODS.topologyPlan]: ["operations.*.edge.a.identity", "operations.*.edge.b.identity"],
  [CONSOLE_RPC_METHODS.topologyApply]: ["operations.*.edge.a.identity", "operations.*.edge.b.identity"],
  [CONSOLE_RPC_METHODS.identityCreate]: ["agents.*.name"],
  [CONSOLE_RPC_METHODS.identityUpdate]: ["name"],
  [CONSOLE_RPC_METHODS.identityArchive]: ["name"],
  [CONSOLE_RPC_METHODS.identityUnarchive]: ["name"],
  [CONSOLE_RPC_METHODS.identityRemove]: ["name"],
  [CONSOLE_RPC_METHODS.identityGet]: ["name"],
  [CONSOLE_RPC_METHODS.identityAdopt]: ["name"],
  [CONSOLE_RPC_METHODS.identityPlan]: ["name"],
  [CONSOLE_RPC_METHODS.identitySettlePreparation]: ["name"],
};

/**
 * The methods of the host that name a member, and the member of params that
 * names it by its ROW KEY (no `mob_id`): the gateway judges such a request
 * in the member's own mob.
 */
export const CONSOLE_HOST_MEMBER_PARAM_BY_METHOD: Readonly<Record<string, string>> = {
  [CONSOLE_RPC_METHODS.accessPreview]: "identity",
};

/**
 * The names a member row carries of its own member. Each is the row key, so
 * a reader may key a row by any of them.
 */
export const CONSOLE_MEMBER_ROW_NAME_FIELDS = ["identity", "member_id", "agent_id"] as const;

/**
 * The members of a member row that state where its member is. `mob_id` is
 * the door the row was read on, never the member's own mob; the mob the
 * member sits in and its bare name there stand in members of their own, and
 * a reader takes them from these two, never from the row key. Tolerant: a
 * gateway that does not list {@link CONSOLE_MOB_ADDRESSING_CAPABILITY}
 * writes none of the three. An entry of the listing of identities
 * (`meta/console/list_identities`, `GET /console/identities`) and the
 * record an inspect answers carry the same three, `mob_id` being the
 * door's mob.
 */
export const CONSOLE_MEMBER_ROW_DOOR_FIELD = "mob_id";
export const CONSOLE_MEMBER_ROW_MOB_FIELD = "member_mob_id";
export const CONSOLE_MEMBER_ROW_BARE_NAME_FIELD = "member_name";

/**
 * The member of a request's `params` that names the mob a member sits in:
 * with it the member word carries the member's bare name, and the request
 * names the member of exactly that mob. A client sends it only to a gateway
 * that lists {@link CONSOLE_MOB_ADDRESSING_CAPABILITY}.
 */
export const CONSOLE_MEMBER_MOB_ID_PARAM = "member_mob_id";

/**
 * The methods of this contract that take {@link CONSOLE_MEMBER_MOB_ID_PARAM}
 * on `POST /console/rpc`; each reads the member's name from `identity`. The
 * gateway's door lists them, beside four of its own methods that this
 * contract does not list, and refuses the member on every other method.
 */
export const CONSOLE_MEMBER_MOB_ID_METHODS = [
  "meta/console/inspect_identity",
  "meta/console/query_timeline",
  "meta/console/send",
  "meta/retire",
] as const;

/**
 * The refusals of a member's naming on the request doors (`-32602`, by
 * `data.kind`). `required`: one string named a spelling two members share,
 * and the two values are needed. `notAccepted`: the request carried
 * {@link CONSOLE_MEMBER_MOB_ID_PARAM} on a method that does not take it.
 * Closed: a client matches these words and no message.
 */
export const CONSOLE_MEMBER_NAMING_REFUSAL_KINDS = {
  required: "member_mob_id_required",
  notAccepted: "member_mob_id_not_accepted",
} as const;

/**
 * The lists of the host that name members. Each takes `mobs: "*"` in its
 * params: without it the list answers, of the entries members made, those
 * of the launch mob's members alone, with it the entries of every mob the
 * caller may address, each entry a member made naming its mob by the pair
 * a frame carries. The console audit read is one of them.
 * `meta/decisions/list` is not: a decision row names no member and no mob.
 */
export const CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS = [
  CONSOLE_RPC_METHODS.gatingPending,
  CONSOLE_RPC_METHODS.gatingAudit,
  CONSOLE_RPC_METHODS.gatingQuestions,
  CONSOLE_RPC_METHODS.consoleAttention,
  CONSOLE_RPC_METHODS.consoleAudit,
] as const;
