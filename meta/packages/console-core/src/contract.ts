export const CONSOLE_CONTRACT_VERSION = "0.5.0";

export const CONSOLE_REST_PATHS = {
  experience: "/console/experience",
  modules: "/console/modules",
  identities: "/console/identities",
  timeline: "/console/timeline",
  timelineStream: "/console/timeline/stream",
  identityTimelineStreamTemplate: "/console/identity/{identity}/stream",
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
  routingRoutesList: "meta/routing/routes/list",
  deliveryHistory: "meta/delivery/history",
  gatingPending: "meta/gating/pending",
  gatingAudit: "meta/gating/audit",
  gatingDecide: "meta/gating/decide",
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
} as const;

export const CONSOLE_BLOB_PATH_PREFIX = "/blobs/";

export const CONSOLE_TIMELINE_QUERY_MODES = ["since", "recent"] as const;

export const CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE = -32013;
