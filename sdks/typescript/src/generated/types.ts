// Generated wire types for Meerkat SDK
// Contract version: 0.7.3

import { MeerkatError } from "./errors.js";

export const CONTRACT_VERSION = "0.7.3";


// K21 — shared fail-closed wire parsing helpers (generated; not exported).
function wireParseError(context: string, message: string): MeerkatError {
  return new MeerkatError("INVALID_RESPONSE", `invalid ${context}: ${message}`);
}

function expectWireObject(value: unknown, context: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw wireParseError(context, "expected object");
  }
  return value as Record<string, unknown>;
}

function requireWireField(
  data: Record<string, unknown>,
  key: string,
  context: string,
): unknown {
  const value = data[key];
  if (value === undefined || value === null) {
    throw wireParseError(context, `missing required field \`${key}\``);
  }
  return value;
}

function expectWireString(value: unknown, context: string): string {
  if (typeof value !== "string") {
    throw wireParseError(context, "expected string");
  }
  return value;
}

function expectWireBoolean(value: unknown, context: string): boolean {
  if (typeof value !== "boolean") {
    throw wireParseError(context, "expected boolean");
  }
  return value;
}

function expectWireInteger(value: unknown, context: string): number {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw wireParseError(context, "expected integer");
  }
  return value;
}

function expectWireNumber(value: unknown, context: string): number {
  if (typeof value !== "number" || Number.isNaN(value)) {
    throw wireParseError(context, "expected number");
  }
  return value;
}

function expectWireArray(value: unknown, context: string): unknown[] {
  if (!Array.isArray(value)) {
    throw wireParseError(context, "expected array");
  }
  return value;
}

function expectWireEnum(
  value: unknown,
  values: readonly string[],
  context: string,
): string {
  if (typeof value !== "string" || !values.includes(value)) {
    throw wireParseError(context, `expected one of ${values.join(", ")}`);
  }
  return value;
}

function expectWireConst(value: unknown, expected: unknown, context: string): unknown {
  if (value !== expected) {
    throw wireParseError(context, `expected constant \`${String(expected)}\``);
  }
  return value;
}

export interface WireUsage {
  input_tokens: number;
  output_tokens: number;
  total_tokens: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
}

export interface WireRunResult {
  session_id: string;
  session_ref?: string;
  text: string;
  turns: number;
  tool_calls: number;
  usage: WireUsage;
  terminal_cause_kind?: string;
  structured_output?: unknown;
  extraction_error?: { last_output: string; attempts: number; reason: string };
  schema_warnings?: Array<{ provider: string; path: string; message: string }>;
}

export interface WireProviderMeta {
  provider: string;
  [key: string]: unknown;
}

export interface WireToolResult {
  tool_use_id: string;
  content: WireToolResultContent;
  is_error?: boolean;
}

export interface WireSessionMessage {
  role: string;
  created_at: string;
  kind?: string;
  body?: string;
  content?: WireContentInput;
  stop_reason?: WireStopReason;
  blocks?: WireAssistantBlock[];
  results?: WireToolResult[];
}

export interface WireSessionHistory {
  session_id: string;
  session_ref?: string;
  message_count: number;
  offset: number;
  limit?: number;
  has_more: boolean;
  messages: WireSessionMessage[];
}

export interface WireEvent {
  session_id: string;
  sequence: number;
  event: Record<string, unknown>;
  contract_version: string;
}

export interface CapabilityEntry {
  id: string;
  description: string;
  status: string;
}

export interface CapabilitiesResponse {
  contract_version: string;
  capabilities: CapabilityEntry[];
}

export interface CommsParams {
  keep_alive?: boolean | null;
  comms_name?: string;
  peer_meta?: Record<string, unknown>;
}

export interface SkillsParams {
  skills_enabled: boolean;
  skill_refs: Array<{ source_uuid: string; skill_name: string }>;
}

export interface McpStdioConfig {
  args?: string[];
  command: string;
  env?: Record<string, string>;
}

export interface McpHttpConfig {
  headers?: Record<string, string>;
  transport?: McpHttpTransport;
  url: string;
}

export type McpServerConfig =
  | ({ name: string; connect_timeout_secs?: number } & McpStdioConfig)
  | ({ name: string; connect_timeout_secs?: number } & McpHttpConfig);

export interface McpAddParams {
  persisted?: boolean;
  server_config: McpServerConfig;
  session_id: string;
}

export interface McpRemoveParams {
  persisted?: boolean;
  server_name: string;
  session_id: string;
}

export interface McpReloadParams {
  persisted?: boolean;
  server_name?: string;
  session_id: string;
}

export interface McpLiveOpResponse {
  applied_at_turn?: number;
  operation: "add" | "remove" | "reload";
  persisted: boolean;
  server_name?: string;
  session_id: string;
  status: "staged" | "applied" | "rejected";
}

export type SkillName = string;

export type SkillScope = "builtin" | "project" | "user";

export type SourceIdentityStatus = "active" | "disabled" | "retired";

export type SourceTransportKind = "embedded" | "filesystem" | "git" | "http" | "stdio";

export type SourceUuid = string;

export type ToolName = string;

export interface SkillEntry {
  description: string;
  is_active: boolean;
  key: SkillKey;
  name: string;
  scope: SkillScope;
  shadowed_by?: SkillSourceProvenance;
  source: SkillSourceProvenance;
}

export interface SkillKey {
  skill_name: SkillName;
  source_uuid: SourceUuid;
}

export interface SkillSourceProvenance {
  display_name: string;
  fingerprint: string;
  source_uuid: SourceUuid;
  status?: SourceIdentityStatus;
  transport_kind: SourceTransportKind;
}

export interface CallbackToolDefinition {
  description: string;
  input_schema: unknown;
  name: ToolName;
}

export interface ConfigEnvelope {
  backend?: string;
  config: unknown;
  generation: number;
  instance_id?: string;
  realm_id?: string;
  resolved_paths?: Record<string, unknown>;
}

export interface ConfigPatchParams {
  expected_generation?: number;
  patch?: unknown;
}

export interface ConfigWriteResult {
  backend?: string;
  config: unknown;
  generation: number;
  instance_id?: string;
  live_propagation?: Record<string, unknown>;
  realm_id?: string;
  resolved_paths?: Record<string, unknown>;
}

export interface InterruptResult {
  interrupted: boolean;
  result: "interrupted" | "staged_noop";
  session_id: string;
}

export interface ServerCapabilities {
  contract_version: string;
  methods: string[];
  server_info: Record<string, unknown>;
}

export interface SkillListResponse {
  skills: SkillEntry[];
}

export interface ToolsRegisterParams {
  tools: CallbackToolDefinition[];
}

export interface ToolsRegisterResult {
  registered: number;
}

export interface WorkEventsResult {
  events: unknown[];
}

export interface WorkItemsResult {
  items: unknown[];
}

export type ConfigSetParams = Record<string, unknown> | unknown;

export interface MobWireParams {
  member: string;
  mob_id: string;
  peer: { local: string } | { external: WireTrustedPeerSpec };
}

export interface MobUnwireParams {
  member: string;
  mob_id: string;
  peer: { local: string } | { external: WireTrustedPeerSpec };
}

export interface MobIdParams {
  mob_id: string;
}

export interface MobMemberParams {
  agent_identity: string;
  mob_id: string;
}

export interface MobCreateParams {
  definition: MobDefinitionInput;
}

export interface MobCreateResult {
  mob_id: string;
}

export interface MobListResult {
  mobs: Record<string, unknown>[];
}

export interface MobStatusResult {
  mob_id: string;
  status: "Creating" | "Running" | "Stopped" | "Completed" | "Destroyed";
}

export interface MobLifecycleParams {
  action: "stop" | "resume" | "complete" | "reset" | "destroy";
  mob_id: string;
}

export interface MobLifecycleResult {
  action: "stop" | "resume" | "complete" | "reset" | "destroy";
  destroy_report?: unknown;
  mob_id: string;
  ok: boolean;
}

export interface MobSpawnParams {
  additional_instructions?: string[];
  agent_identity: string;
  auth_binding?: WireAuthBindingRef;
  auto_wire_parent?: boolean;
  backend?: WireMobBackendKind;
  binding?: WireRuntimeBinding;
  budget_split_policy?: WireBudgetSplitPolicy;
  context?: unknown;
  inherited_tool_filter?: WireToolFilter;
  initial_message?: WireContentInput;
  labels?: Record<string, string>;
  launch_mode?: WireMemberLaunchMode;
  mob_id: string;
  override_profile?: WireMobProfile;
  profile: string;
  runtime_mode?: WireMobRuntimeMode;
  shell_env?: Record<string, string>;
  tool_access_policy?: WireToolAccessPolicy;
}

export interface MobSpawnResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobSpawnSpecParams {
  additional_instructions?: string[];
  agent_identity: string;
  auth_binding?: WireAuthBindingRef;
  backend?: WireMobBackendKind;
  context?: unknown;
  initial_message?: WireContentInput;
  labels?: Record<string, string>;
  profile: string;
  runtime_mode?: WireMobRuntimeMode;
}

export interface MobSpawnManyParams {
  mob_id: string;
  specs: MobSpawnSpecParams[];
}

export interface MobSpawnManyResult {
  results: MobSpawnManyResultEntry[];
}

export interface MobSpawnManyResultEntry {
  result: MobSpawnManyResultPayload;
  status: MobSpawnManyResultStatus;
}

export interface MobSpawnManySpawnedResult {
  agent_identity: string;
  member_ref: WireMemberRef;
}

export interface MobSpawnManyFailedResult {
  cause: MobSpawnManyFailureCause;
  message: string;
}

export interface MobSpawnReceiptWire {
  agent_identity: string;
  member_ref: WireMemberRef;
}

export interface MobMemberListEntryWire {
  agent_identity: string;
  error?: string;
  is_final: boolean;
  labels?: Record<string, string>;
  member_ref: WireMemberRef;
  role: string;
  runtime_mode: WireMobRuntimeMode;
  status: WireMobMemberStatus;
  wired_to?: string[];
}

export interface WireMobToolConfig {
  builtins?: boolean;
  comms?: boolean;
  image_generation?: boolean;
  mcp?: string[];
  memory?: boolean;
  mob?: boolean;
  schedule?: boolean;
  shell?: boolean;
  workgraph?: boolean;
}

export interface WireMobProfile {
  auto_compact_threshold?: number;
  backend?: WireMobBackendKind;
  external_addressable?: boolean;
  image_generation_provider?: Provider;
  max_inline_peer_notifications?: number;
  model: string;
  output_schema?: unknown;
  peer_description?: string;
  provider?: Provider;
  provider_params?: unknown;
  resume_overrides?: WireMobResumeOverrideField[];
  runtime_mode?: WireMobRuntimeMode;
  self_hosted_server_id?: string;
  skills?: string[];
  tools?: WireMobToolConfig;
}

export interface WireMobRun {
  flow_id: string;
  mob_id: string;
  run_id: string;
  status: "pending" | "running" | "completed" | "failed" | "canceled";
}

export interface WireMobRunResultEnvelope {
  flow_id: string;
  mob_id: string;
  outputs?: Record<string, unknown>;
  result?: unknown;
  run_id: string;
  status: "pending" | "running" | "completed" | "failed" | "canceled";
}

export interface WireMobRunStatus {
}

export interface WirePeerConnectivity {
}

export interface WirePeerConnectivitySnapshot {
  reachable_peer_count: number;
  unknown_peer_count: number;
  unreachable_peers?: Record<string, unknown>[];
}

export interface WireUnreachablePeer {
  peer: string;
  reason?: string;
}

export interface WireMobError {
  code: MobSpawnManyFailureCause;
  message: string;
}

export interface MobEnsureMemberParams {
  mob_id: string;
  spec: MobMemberSpecWire;
}

export interface MobEnsureMemberResult {
  outcome: { spawned: MobSpawnReceiptWire } | { existed: MobMemberListEntryWire };
}

export interface MobReconcileParams {
  desired?: MobMemberSpecWire[];
  mob_id: string;
  options?: MobReconcileOptionsWire;
}

export interface MobReconcileResult {
  report: MobReconcileReportWire;
}

export interface MobListMembersMatchingParams {
  filter?: Record<string, unknown>;
  mob_id: string;
}

export interface MobListMembersMatchingResult {
  members?: unknown[];
}

export interface MobRetireResult {
  retired: boolean;
}

export interface MobRespawnParams {
  agent_identity: string;
  initial_message?: WireContentInput;
  mob_id: string;
}

export interface MobRespawnResult {
  failed_peer_ids?: string[];
  receipt: Record<string, unknown>;
  status: "completed" | "topology_restore_failed";
}

export interface MobWireResult {
  wired: boolean;
}

export interface MobWireMembersBatchEdge {
  a: string;
  b: string;
}

export interface MobWireMembersBatchParams {
  edges: MobWireMembersBatchEdge[];
  mob_id: string;
}

export interface MobWireMembersBatchResult {
  already_wired: MobWireMembersBatchEdge[];
  requested: number;
  wired: MobWireMembersBatchEdge[];
}

export interface MobUnwireResult {
  unwired: boolean;
}

export interface MobMembersResult {
  members: MobMemberListEntryWire[];
  mob_id: string;
}

export interface MobEventsParams {
  after_cursor?: number;
  limit?: number;
  mob_id: string;
  strict?: boolean;
}

export interface MobEventsResult {
  events: unknown[];
}

export interface MobMemberSendParams {
  agent_identity: string;
  content: WireContentInput;
  handling_mode?: "queue" | "steer";
  mob_id: string;
  render_metadata?: Record<string, unknown>;
}

export interface MobMemberSendResult {
  agent_identity: string;
  handling_mode: "queue" | "steer";
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobIngressInteractionParams {
  content: WireContentInput;
  handling_mode?: "queue" | "steer";
  mob_id: string;
  render_metadata?: Record<string, unknown>;
  spec: MobMemberSpecWire;
}

export interface MobIngressInteractionResult {
  agent_identity: string;
  delivery: Record<string, unknown>;
  ensure_outcome: { spawned: MobSpawnReceiptWire } | { existed: MobMemberListEntryWire };
  events_after_cursor: number;
  latest_event_cursor: number;
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobAppendSystemContextParams {
  agent_identity: string;
  idempotency_key?: string;
  mob_id: string;
  source?: string;
  text: string;
}

export interface MobAppendSystemContextResult {
  agent_identity: string;
  mob_id: string;
  status: "applied" | "staged" | "duplicate";
}

export interface MobFlowsResult {
  flows: string[];
  mob_id: string;
}

export interface MobRunParams {
  flow_id?: string;
  mob_id: string;
  params?: unknown;
  prompt?: string;
}

export interface MobFlowRunParams {
  flow_id: string;
  mob_id: string;
  params?: unknown;
}

export interface MobFlowRunResult {
  run_id: string;
}

export interface MobFlowStatusParams {
  mob_id: string;
  run_id: string;
}

export interface MobFlowStatusResult {
  run?: Record<string, unknown>;
}

export interface MobRunResultParams {
  mob_id: string;
  run_id: string;
}

export interface MobRunResult {
  run?: Record<string, unknown>;
}

export interface MobFlowCancelParams {
  mob_id: string;
  run_id: string;
}

export interface MobFlowCancelResult {
  canceled: boolean;
}

export interface MobSpawnHelperParams {
  agent_identity?: string;
  backend?: WireMobBackendKind;
  mob_id: string;
  prompt: string;
  role_name?: string;
  runtime_mode?: WireMobRuntimeMode;
}

export interface MobForkHelperParams {
  agent_identity?: string;
  backend?: WireMobBackendKind;
  fork_context?: unknown;
  mob_id: string;
  prompt: string;
  role_name?: string;
  runtime_mode?: WireMobRuntimeMode;
  source_member_id: string;
}

export interface MobHelperResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  output?: string;
  tokens_used: number;
}

export interface MobForceCancelResult {
  cancelled: boolean;
}

export interface MobTurnStartParams {
  additional_instructions?: string[];
  agent_identity: string;
  auth_binding?: Record<string, unknown>;
  flow_tool_overlay?: PublicTurnToolOverlay;
  keep_alive?: boolean;
  max_tokens?: number;
  mob_id: string;
  model?: string;
  output_schema?: unknown;
  prompt: WireContentInput;
  provider?: string;
  provider_params?: Record<string, unknown>;
  skill_refs?: SkillKey[];
  structured_output_retries?: number;
  system_prompt?: string;
}

export interface MobMemberStatusResult {
  current_session_id?: string;
  error?: string;
  external_member?: unknown;
  is_final: boolean;
  kickoff?: unknown;
  member_ref: WireMemberRef;
  output_preview?: string;
  peer_connectivity?: Record<string, unknown>;
  resolved_capabilities?: WireResolvedModelCapabilities;
  status: WireMobMemberStatus;
  tokens_used: number;
}

export interface MobSnapshotResult {
  members: MobMemberListEntryWire[];
  mob_id: string;
  status: "Creating" | "Running" | "Stopped" | "Completed" | "Destroyed";
}

export interface MobDestroyResult {
  destroy_report: unknown;
  mob_id: string;
  ok: boolean;
}

export interface SupervisorRotationReportWire {
  current_epoch: number;
  previous_epoch: number;
  public_peer_id: string;
}

export interface MobRotateSupervisorResult {
  mob_id: string;
  ok: boolean;
  report: SupervisorRotationReportWire;
}

export interface MobSubmitWorkParams {
  content: WireContentInput;
  member_ref: WireMemberRef;
  origin?: "external" | "internal";
  work_ref?: string;
}

export interface MobSubmitWorkResult {
  member_ref: WireMemberRef;
  mob_id: string;
  work_ref: string;
}

export interface MobCancelWorkParams {
  mob_id: string;
  work_ref: string;
}

export interface MobCancelWorkResult {
  mob_id: string;
  ok: boolean;
}

export interface MobCancelAllWorkParams {
  member_ref: WireMemberRef;
}

export interface MobCancelAllWorkResult {
  mob_id: string;
  ok: boolean;
}

export interface MobWaitParams {
  member_ids?: string[];
  mob_id: string;
  timeout_ms?: number;
}

export interface MobWaitMembersResult {
  members: unknown[];
}

export interface MobProfileCreateParams {
  name: string;
  profile: MobProfileInput;
}

export interface MobProfileNameParams {
  name: string;
}

export interface MobProfileLookupResult {
  created_at?: string;
  name: string;
  not_found?: boolean;
  profile?: WireMobProfile;
  revision?: number;
  updated_at?: string;
}

export interface MobProfileListResult {
  profiles: Record<string, unknown>[];
}

export interface MobProfileUpdateParams {
  expected_revision: number;
  name: string;
  profile: MobProfileInput;
}

export interface MobProfileDeleteParams {
  expected_revision: number;
  name: string;
}

export interface MobProfileDeleteResult {
  deleted_revision: number;
  name: string;
}

export interface MobStreamOpenParams {
  agent_identity?: string;
  mob_id: string;
}

export interface MobStreamOpenResult {
  opened: boolean;
  stream_id: string;
}

export interface MobStreamCloseParams {
  stream_id: string;
}

export interface MobStreamCloseResult {
  already_closed: boolean;
  closed: boolean;
  stream_id: string;
}

export interface PublicTurnToolOverlay {
  allowed_tools?: ToolName[];
  blocked_tools?: ToolName[];
}

export interface MobDefinitionInput {
  backend?: MobBackendConfigInput;
  event_router?: MobEventRouterConfigInput;
  flows?: Record<string, MobFlowSpecInput>;
  id: string;
  image_generation_provider?: Provider;
  limits?: MobLimitsSpecInput;
  models?: Record<string, CustomModelConfig>;
  orchestrator?: MobOrchestratorInput;
  profiles: Record<string, MobProfileBindingInput>;
  skills?: Record<string, MobSkillSourceInput>;
  spawn_policy?: MobSpawnPolicyInput;
  supervisor?: MobSupervisorSpecInput;
  topology?: MobTopologySpecInput;
  wiring?: MobWiringRulesInput;
}

export interface MobBackendConfigInput {
  default?: WireMobBackendKind;
  external?: MobExternalBackendConfigInput;
}

export interface MobEventRouterConfigInput {
  buffer_size?: number;
  exclude_patterns?: string[];
  include_patterns?: string[];
}

export interface MobExternalBackendConfigInput {
  address_base: string;
  supervisor_bridge?: unknown;
}

export interface MobFlowSpecInput {
  description?: string;
  root?: MobFrameSpecInput;
  steps?: Record<string, MobFlowStepInput>;
}

export interface MobFlowStepInput {
  allowed_tools?: string[];
  blocked_tools?: string[];
  branch?: string;
  collection_policy?: MobCollectionPolicyInput;
  condition?: MobConditionExprInput;
  depends_on?: string[];
  depends_on_mode?: MobDependencyModeInput;
  dispatch_mode?: MobDispatchModeInput;
  expected_schema_ref?: string;
  message: WireContentInput;
  output_format?: MobStepOutputFormatInput;
  role: string;
  timeout_ms?: number;
}

export interface MobFrameSpecInput {
  nodes: Record<string, MobFlowNodeInput>;
}

export interface MobLimitsSpecInput {
  cancel_grace_timeout_ms?: number;
  max_active_frames?: number;
  max_active_nodes?: number;
  max_flow_duration_ms?: number;
  max_frame_depth?: number;
  max_orphaned_turns?: number;
  max_step_retries?: number;
}

export interface MobOrchestratorInput {
  profile: string;
}

export interface MobProfileInput {
  auto_compact_threshold?: number;
  backend?: WireMobBackendKind;
  external_addressable?: boolean;
  image_generation_provider?: Provider;
  max_inline_peer_notifications?: number;
  model: string;
  output_schema?: unknown;
  peer_description?: string;
  provider?: Provider;
  provider_params?: unknown;
  resume_overrides?: WireMobResumeOverrideField[];
  runtime_mode?: WireMobRuntimeMode;
  self_hosted_server_id?: string;
  skills?: string[];
  tools?: MobToolConfigInput;
}

export interface MobRoleWiringRuleInput {
  a: string;
  b: string;
}

export interface MobSupervisorSpecInput {
  escalation_threshold: number;
  escalation_turn_timeout_ms?: number;
  role: string;
}

export interface MobToolConfigInput {
  builtins?: boolean;
  comms?: boolean;
  image_generation?: boolean;
  mcp?: string[];
  memory?: boolean;
  mob?: boolean;
  schedule?: boolean;
  shell?: boolean;
  workgraph?: boolean;
}

export interface MobTopologyRuleInput {
  allowed: boolean;
  from_role: string;
  to_role: string;
}

export interface MobTopologySpecInput {
  mode: MobPolicyModeInput;
  rules: MobTopologyRuleInput[];
}

export interface MobWiringRulesInput {
  auto_wire_orchestrator?: boolean;
  role_wiring?: MobRoleWiringRuleInput[];
}

export interface CustomModelConfig {
  call_timeout_secs?: number;
  context_window?: number;
  display_name?: string;
  max_output_tokens?: number;
  provider: Provider;
  vision?: boolean;
  web_search?: boolean;
}

export interface MobMemberSpecWire {
  additional_instructions?: string[];
  agent_identity: string;
  auto_wire_parent?: boolean;
  backend?: WireMobBackendKind;
  binding?: WireRuntimeBinding;
  context?: unknown;
  initial_message?: WireContentInput;
  labels?: Record<string, string>;
  profile: string;
  runtime_mode?: WireMobRuntimeMode;
}

export interface MobReconcileOptionsWire {
  retire_stale?: boolean;
}

export interface MobReconcileReportWire {
  desired?: string[];
  failures?: MobReconcileFailureWire[];
  retained?: string[];
  retired?: string[];
  spawned?: MobSpawnReceiptWire[];
}

export interface MobReconcileFailureWire {
  agent_identity: string;
  error: WireMobError;
  stage: WireMobReconcileStage;
}

export interface AttentionBindingRequest {
  binding_id: string;
  namespace?: string;
  realm_id?: string;
}

export interface AttentionBindingResult {
  attention: WorkAttentionBinding;
}

export interface AttentionContextProjection {
  authority: ProjectedAttentionAuthority;
  binding_id: string;
  binding_revision: number;
  evidence_refs?: WorkEvidenceRef[];
  item_revision: number;
  mode: WorkAttentionMode;
  parent_context?: Record<string, unknown>[];
  parent_refs?: WorkItemRef[];
  text: AttentionProjectionText;
  work_ref: WorkItemRef;
}

export interface AttentionListRequest {
  namespace?: string;
  realm_id?: string;
  status?: WorkAttentionStatus;
  target?: WorkAttentionTarget;
}

export interface AttentionListResult {
  attention: WorkAttentionBinding[];
}

export interface AttentionProjectionPolicy {
  include_parent_context?: boolean;
  max_text_chars?: number;
}

export interface AttentionProjectionRequest {
  binding_id: string;
  namespace?: string;
  realm_id?: string;
}

export interface AttentionProjectionResult {
  projection: AttentionContextProjection;
}

export interface AttentionProjectionText {
  rendered: string;
  title: string;
  truncated: boolean;
}

export interface AttentionReassignRequest {
  binding_id: string;
  namespace?: string;
  realm_id?: string;
  target: GoalAttentionTarget;
}

export interface GoalStatusRequest {
  binding_id: string;
  namespace?: string;
  realm_id?: string;
}

export interface GoalStatusResult {
  attention: WorkAttentionBinding;
  item: WorkItem;
}

export interface ProjectedAttentionAuthority {
  can_add_evidence: boolean;
  can_block: boolean;
  can_close_if_policy_allows: boolean;
  can_close_own_review_item?: boolean;
  can_create: boolean;
  can_get: boolean;
  can_link: boolean;
  can_link_derived_from: boolean;
  can_link_parent: boolean;
  can_link_related: boolean;
  can_release: boolean;
  can_update: boolean;
}

export interface ReadyWorkFilter {
  labels?: string[];
  limit?: number;
  namespace?: string;
  realm_id?: string;
}

export interface WorkAttentionBinding {
  binding_id: string;
  created_at: string;
  delegated_authority: AttentionDelegatedAuthority;
  machine_state?: Record<string, unknown>;
  mode: WorkAttentionMode;
  projection_policy?: AttentionProjectionPolicy;
  status: WorkAttentionStatus;
  target: WorkAttentionTarget;
  updated_at: string;
  work_ref: WorkItemRef;
}

export interface WorkGraphEventFilter {
  after_seq?: number;
  all_namespaces?: boolean;
  limit?: number;
  namespace?: string;
  realm_id?: string;
}

export interface WorkGraphEventsResponse {
  events: WorkGraphEvent[];
}

export interface WorkGraphIdParams {
  id: string;
  namespace?: string;
  realm_id?: string;
}

export interface WorkGraphItemsResponse {
  items: WorkItem[];
}

export interface WorkGraphSnapshot {
  all_namespaces: boolean;
  attention?: WorkAttentionBinding[];
  captured_at: string;
  edges: WorkEdge[];
  event_high_water_mark?: number;
  items: WorkItem[];
  namespace?: string;
  ready_item_ids: string[];
  realm_id: string;
}

export interface WorkGraphSnapshotFilter {
  all_namespaces?: boolean;
  include_terminal?: boolean;
  labels?: string[];
  limit?: number;
  namespace?: string;
  realm_id?: string;
  statuses?: WorkStatus[];
}

export interface WorkItem {
  claim?: WorkItemClaim;
  completion_policy: WorkCompletionPolicy;
  created_at: string;
  description?: string;
  due_at?: string;
  evidence_refs?: WorkEvidenceRef[];
  external_refs?: WorkItemExternalRef[];
  id: string;
  labels?: string[];
  machine_state: Record<string, unknown>;
  namespace: string;
  not_before?: string;
  owner?: WorkItemOwner;
  priority: "low" | "medium" | "high";
  realm_id: string;
  revision: number;
  snoozed_until?: string;
  status: "open" | "in_progress" | "blocked" | "completed" | "cancelled" | "failed";
  terminal_at?: string;
  title: string;
  updated_at: string;
}

export interface WorkItemFilter {
  all_namespaces?: boolean;
  include_terminal?: boolean;
  labels?: string[];
  limit?: number;
  namespace?: string;
  realm_id?: string;
  statuses?: WorkStatus[];
}

export interface WorkItemRef {
  item_id: string;
  namespace: string;
  realm_id: string;
}

export interface WorkEdge {
  created_at: string;
  from_id: string;
  kind: WorkEdgeKind;
  namespace: string;
  realm_id: string;
  to_id: string;
}

export interface WorkEvidenceRef {
  confirmation_kind?: WorkEvidenceKind;
  confirming_owner_key?: WorkOwnerKey;
  id: string;
  kind: string;
  label?: string;
  summary?: string;
}

export interface WorkGraphEvent {
  at: string;
  item_id?: string;
  kind: WorkGraphEventKind;
  namespace: string;
  payload?: unknown;
  realm_id: string;
  seq?: number;
}

export interface WorkOwnerKey {
  id: string;
  kind: WorkOwnerKind;
}

export interface WorkItemClaim {
  claimed_at: string;
  lease_expires_at?: string;
  owner: WorkItemOwner;
}

export interface WorkItemExternalRef {
  id: string;
  kind: string;
  url?: string;
}

export interface WorkItemOwner {
  display_name?: string;
  key: WorkOwnerKey;
}

export interface BridgeAck {
  ok: boolean;
}

export interface BridgeBindPayload {
  bootstrap_token: BridgeBootstrapToken;
  epoch: number;
  expected_address: string;
  expected_peer_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeBindResponse {
  address: string;
  capabilities: BridgeCapabilities;
  peer_id: string;
}

export interface BridgeCapabilities {
  current_protocol_version?: BridgeProtocolVersion;
  default_protocol_version?: BridgeProtocolVersion;
  deliver_member_input?: boolean;
  destroy_member?: boolean;
  hard_cancel_member?: boolean;
  interrupt_member?: boolean;
  observe_member?: boolean;
  retire_member?: boolean;
  supported_protocol_versions?: BridgeProtocolVersion[];
  unwire_member?: boolean;
  wire_member?: boolean;
}

export interface BridgeDeliveryPayload {
  content: ContentInput;
  epoch: number;
  handling_mode: HandlingMode;
  input_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeDeliveryResponse {
  canonical_input_id?: string;
  input_id: string;
  outcome: BridgeDeliveryOutcome;
}

export interface BridgeDestroyResponse {
  inputs_abandoned: number;
}

export interface BridgeHardCancelPayload {
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  reason: string;
  supervisor: BridgePeerSpec;
}

export interface BridgeObservationResponse {
  accepting_inputs?: boolean;
  current_run_id?: string;
  last_error?: string;
  observed_at: string;
  peer_connectivity?: BridgePeerConnectivity;
  state: BridgeMemberRuntimeState;
}

export interface BridgePeerSpec {
  address: string;
  name: string;
  peer_id: string;
  pubkey?: number[];
}

export interface BridgePeerWiringPayload {
  epoch: number;
  mob_peer_overlay?: Record<string, unknown>;
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeRetireResponse {
  inputs_abandoned: number;
  inputs_pending_drain: number;
}

export interface BridgeSupervisorPayload {
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface CommsChecksumTokenParams {
  subject: string;
}

export interface CommsChecksumTokenResult {
  request_intent: CommsChecksumTokenResultIntent;
  request_subject: string;
  token: string;
}

export interface CommsPeerLifecycleParams {
  description?: string;
  peer: string;
  peer_spec?: BridgePeerSpec;
  role?: string;
}

export interface CommsPeersParams {
  session_id: string;
}

export interface PeerAddress {
  endpoint: string;
  transport: PeerTransport;
}

export interface PeerCapabilitySet {
  extensions?: Record<string, unknown>;
  version?: number;
}

export interface PeerDirectoryEntry {
  address: PeerAddress;
  capabilities: PeerCapabilitySet;
  meta: Record<string, unknown>;
  name: PeerName;
  peer_id: PeerId;
  sendable_kinds: PeerSendability[];
  source: PeerDirectorySource;
}

export interface CommsPeersResult {
  peers: PeerDirectoryEntry[];
}

export interface SessionStreamOpenParams {
  session_id: string;
}

export interface SessionStreamOpenResult {
  opened: boolean;
  session_id: string;
  stream_id: string;
}

export interface SessionStreamCloseParams {
  stream_id: string;
}

export interface SessionStreamCloseResult {
  already_closed: boolean;
  closed: boolean;
  stream_id: string;
}

export interface ScheduleIdParams {
  schedule_id: string;
}

export interface ListSchedulesParams {
  labels?: Record<string, string>;
  limit?: number;
  offset?: number;
}

export interface ScheduleOccurrencesParams {
  include_terminal?: boolean;
  schedule_id: string;
}

export interface UpdateScheduleParams {
  description?: string;
  expected_revision?: number;
  labels?: Record<string, string>;
  misfire_policy?: Record<string, unknown>;
  missing_target_policy?: "skip" | "mark_misfired";
  name?: string;
  overlap_policy?: "allow_concurrent" | "skip_if_running";
  planning_horizon_days?: number;
  planning_horizon_occurrences?: number;
  schedule_id: string;
  target?: Record<string, unknown>;
  trigger?: Record<string, unknown>;
}

export interface WireContentBlockText {
  text: string;
  type: "text";
}

export interface WireContentBlockImage {
  media_type: string;
  type: "image";
}

export interface WireContentBlockVideo {
  duration_ms: number;
  media_type: string;
  type: "video";
}

export interface WireContentBlockStructured {
  data: unknown;
  type: "structured";
}

export interface WireContentBlockUnknown {
  type: "unknown";
}

export type WireContentBlock = WireContentBlockText | WireContentBlockImage | WireContentBlockVideo | WireContentBlockStructured | WireContentBlockUnknown;

export type WireContentInput = string | WireContentBlock[];

export type Provider = "anthropic" | "openai" | "gemini" | "self_hosted" | "other";

export type WireMemberRef = string;

export type WireMobBackendKind = "session" | "external";

export type WireMobResumeOverrideField = "model" | "provider" | "provider_params";

export interface WireRuntimeBindingSession {
  kind: "session";
}

export interface WireRuntimeBindingExternal {
  address: string;
  bootstrap_token?: BridgeBootstrapToken;
  identity: WireTrustedPeerIdentity;
  kind: "external";
}

export type WireRuntimeBinding = WireRuntimeBindingSession | WireRuntimeBindingExternal;

export interface WireMemberLaunchModeFresh {
  mode: "fresh";
}

export interface WireMemberLaunchModeResume {
  bridge_session_id: string;
  mode: "resume";
}

export interface WireMemberLaunchModeFork {
  fork_context?: WireForkContext;
  mode: "fork";
  source_member_id: string;
}

export type WireMemberLaunchMode = WireMemberLaunchModeFresh | WireMemberLaunchModeResume | WireMemberLaunchModeFork;

export interface WireForkContextFullHistory {
  type: "full_history";
}

export interface WireForkContextLastMessages {
  count: number;
  type: "last_messages";
}

export type WireForkContext = WireForkContextFullHistory | WireForkContextLastMessages;

export interface WireToolAccessPolicyInherit {
  type: "inherit";
}

export interface WireToolAccessPolicyAllowList {
  type: "allow_list";
  value: string[];
}

export interface WireToolAccessPolicyDenyList {
  type: "deny_list";
  value: string[];
}

export type WireToolAccessPolicy = WireToolAccessPolicyInherit | WireToolAccessPolicyAllowList | WireToolAccessPolicyDenyList;

export interface WireBudgetSplitPolicyEqual {
  type: "equal";
}

export interface WireBudgetSplitPolicyProportional {
  type: "proportional";
}

export interface WireBudgetSplitPolicyRemaining {
  type: "remaining";
}

export interface WireBudgetSplitPolicyFixed {
  type: "fixed";
  value: number;
}

export type WireBudgetSplitPolicy = WireBudgetSplitPolicyEqual | WireBudgetSplitPolicyProportional | WireBudgetSplitPolicyRemaining | WireBudgetSplitPolicyFixed;

export type WireToolFilter = "All" | { Allow: string[] } | { Deny: string[] };

export type WireMemberState = unknown;

export type WireMobMemberStatus = "active" | "retiring" | "broken" | "completed" | "unknown";

export type WireMobRuntimeMode = "autonomous_host" | "turn_driven";

export type MobSpawnManyFailureCause = "profile_not_found" | "member_not_found" | "member_already_exists" | "not_externally_addressable" | "invalid_transition" | "wiring_error" | "bridge_command_rejected" | "member_restore_failed" | "kickoff_wait_timed_out" | "ready_wait_timed_out" | "definition_error" | "flow_not_found" | "flow_failed" | "run_not_found" | "run_canceled" | "flow_turn_timed_out" | "frame_depth_limit_exceeded" | "frame_atomic_persistence_unavailable" | "spec_revision_conflict" | "schema_validation" | "insufficient_targets" | "topology_violation" | "bridge_delivery_rejected" | "supervisor_escalation" | "unsupported_for_mode" | "missing_member_capability" | "reset_barrier" | "storage_error" | "session_error" | "comms_error" | "callback_pending" | "stale_fence_token" | "stale_event_cursor" | "work_not_found" | "internal";

export type MobSpawnManyResultStatus = "spawned" | "failed";

export type MobSpawnManyResultPayload = MobSpawnManySpawnedResult | MobSpawnManyFailedResult;

export interface MobCollectionPolicyInputAll {
  type: "all";
}

export interface MobCollectionPolicyInputAny {
  type: "any";
}

export interface MobCollectionPolicyInputQuorum {
  n: number;
  type: "quorum";
}

export type MobCollectionPolicyInput = MobCollectionPolicyInputAll | MobCollectionPolicyInputAny | MobCollectionPolicyInputQuorum;

export interface MobConditionExprInputEq {
  op: "eq";
  path: string;
  value: unknown;
}

export interface MobConditionExprInputIn {
  op: "in";
  path: string;
  values: unknown[];
}

export interface MobConditionExprInputGt {
  op: "gt";
  path: string;
  value: unknown;
}

export interface MobConditionExprInputLt {
  op: "lt";
  path: string;
  value: unknown;
}

export interface MobConditionExprInputAnd {
  exprs: MobConditionExprInput[];
  op: "and";
}

export interface MobConditionExprInputOr {
  exprs: MobConditionExprInput[];
  op: "or";
}

export interface MobConditionExprInputNot {
  expr: MobConditionExprInput;
  op: "not";
}

export type MobConditionExprInput = MobConditionExprInputEq | MobConditionExprInputIn | MobConditionExprInputGt | MobConditionExprInputLt | MobConditionExprInputAnd | MobConditionExprInputOr | MobConditionExprInputNot;

export type MobDependencyModeInput = "all" | "any";

export type MobDispatchModeInput = "fan_out" | "one_to_one" | "fan_in";

export interface MobFlowNodeInputStep {
  branch?: string;
  depends_on?: string[];
  depends_on_mode?: MobDependencyModeInput;
  kind: "step";
  step_id: string;
}

export interface MobFlowNodeInputRepeatUntil {
  body: MobFrameSpecInput;
  depends_on?: string[];
  depends_on_mode?: MobDependencyModeInput;
  kind: "repeat_until";
  loop_id: string;
  max_iterations: number;
  until: MobConditionExprInput;
}

export type MobFlowNodeInput = MobFlowNodeInputStep | MobFlowNodeInputRepeatUntil;

export type MobPolicyModeInput = "advisory" | "strict";

export type MobProfileBindingInput = Record<string, unknown> | MobProfileInput;

export interface MobSkillSourceInputInline {
  content: string;
  source: "inline";
}

export interface MobSkillSourceInputPath {
  path: string;
  source: "path";
}

export type MobSkillSourceInput = MobSkillSourceInputInline | MobSkillSourceInputPath;

export interface MobSpawnPolicyInputNone {
  mode: "none";
}

export interface MobSpawnPolicyInputAuto {
  mode: "auto";
  profile_map: Record<string, string>;
}

export type MobSpawnPolicyInput = MobSpawnPolicyInputNone | MobSpawnPolicyInputAuto;

export type MobStepOutputFormatInput = "json" | "text";

export type WireMobReconcileStage = "spawn" | "retire";

export type AttentionDelegatedAuthority = "add_evidence" | "close_own_review_item" | "request_closure" | "close_if_policy_allows";

export interface GoalAttentionTargetSession {
  kind: "session";
  session_id: string;
}

export type GoalAttentionTarget = GoalAttentionTargetSession;

export type WorkAttentionMode = "pursue" | "coordinate" | "review" | "falsify" | "judge" | "observe";

export interface WorkAttentionStatusActive {
  state: "active";
}

export interface WorkAttentionStatusPaused {
  state: "paused";
  until?: string;
}

export interface WorkAttentionStatusSuperseded {
  state: "superseded";
}

export interface WorkAttentionStatusStopped {
  state: "stopped";
}

export type WorkAttentionStatus = WorkAttentionStatusActive | WorkAttentionStatusPaused | WorkAttentionStatusSuperseded | WorkAttentionStatusStopped;

export interface WorkAttentionTargetSession {
  kind: "session";
  session_id: string;
}

export interface WorkAttentionTargetLoweredOwner {
  kind: "lowered_owner";
  owner_key: WorkOwnerKey;
}

export type WorkAttentionTarget = WorkAttentionTargetSession | WorkAttentionTargetLoweredOwner;

export interface WorkCompletionPolicySelfAttest {
  kind: "self_attest";
}

export interface WorkCompletionPolicyHostConfirmed {
  kind: "host_confirmed";
}

export interface WorkCompletionPolicyPrincipalConfirmed {
  kind: "principal_confirmed";
}

export interface WorkCompletionPolicySupervisor {
  kind: "supervisor";
  owner_key: WorkOwnerKey;
}

export interface WorkCompletionPolicyReviewerQuorum {
  kind: "reviewer_quorum";
  threshold: number;
}

export type WorkCompletionPolicy = WorkCompletionPolicySelfAttest | WorkCompletionPolicyHostConfirmed | WorkCompletionPolicyPrincipalConfirmed | WorkCompletionPolicySupervisor | WorkCompletionPolicyReviewerQuorum;

export type WorkEdgeKind = "blocks" | "parent" | "related" | "supersedes" | "derived_from";

export type WorkEvidenceKind = "host_confirmation" | "principal_confirmation" | "supervisor_confirmation" | "reviewer_confirmation" | "self_attest";

export type WorkGraphEventKind = "created" | "updated" | "claimed" | "released" | "blocked" | "closed" | "linked" | "evidence_added" | "attention_created" | "attention_updated";

export type WorkOwnerKind = "principal" | "agent" | "session" | "mob" | "label";

export type WorkStatus = "open" | "in_progress" | "blocked" | "completed" | "cancelled" | "failed";

export const WORK_GRAPH_STATUSES = ["open", "in_progress", "blocked", "completed", "cancelled", "failed"] as const;
export type WorkGraphStatus = typeof WORK_GRAPH_STATUSES[number];

export const WORK_GRAPH_PRIORITIES = ["low", "medium", "high"] as const;
export type WorkGraphPriority = typeof WORK_GRAPH_PRIORITIES[number];

export const WORK_GRAPH_EVENT_KINDS = ["created", "updated", "claimed", "released", "blocked", "closed", "linked", "evidence_added", "attention_created", "attention_updated"] as const;

export type McpLiveOperation = "add" | "remove" | "reload";

export type McpLiveOpStatus = "staged" | "applied" | "rejected";

export type McpHttpTransport = "streamable-http" | "sse";

export type MobPeerTarget = { local: string } | { external: WireTrustedPeerSpec };

export type WireHandlingMode = "queue" | "steer";

export type WireRenderClass = "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";

export type WireRenderSalience = "background" | "normal" | "important" | "urgent";

export type WireRuntimeState = "initializing" | "idle" | "attached" | "running" | "retired" | "stopped" | "destroyed";

export type RealtimeTurningMode = "provider_managed" | "explicit_commit";

export type RealtimeInputKind = "text" | "audio" | "video";

export type RealtimeOutputKind = "text" | "audio" | "video";

export interface RealtimeInputChunkTextChunk {
  text: string;
  kind: "text_chunk";
}

export interface RealtimeInputChunkAudioChunk {
  channels: number;
  data: string;
  mime_type: string;
  sample_rate_hz: number;
  kind: "audio_chunk";
}

export interface RealtimeInputChunkVideoChunk {
  data: string;
  mime_type: string;
  kind: "video_chunk";
}

export type RealtimeInputChunk = RealtimeInputChunkTextChunk | RealtimeInputChunkAudioChunk | RealtimeInputChunkVideoChunk;

export type RuntimeAcceptOutcomeType = "accepted" | "deduplicated" | "rejected";

export type WireInputLifecycleState = "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";

export type WireStopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

export type WireToolResultContent = string | WireContentBlock[];

export type WireModelTier = "recommended" | "supported";

export interface CommsCommandInput {
  allow_self_session?: boolean;
  blocks?: ContentBlock[];
  body: string;
  handling_mode?: HandlingMode;
  kind: "input";
  source?: "tcp" | "uds" | "stdin" | "webhook" | "rpc";
  stream?: "none" | "reserve_interaction";
}

export interface CommsCommandPeerMessage {
  blocks?: ContentBlock[];
  body: string;
  handling_mode?: HandlingMode;
  kind: "peer_message";
  to: PeerId;
}

export interface CommsCommandPeerLifecycle {
  kind: "peer_lifecycle";
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired" | "mob.dismiss";
  params: CommsPeerLifecycleParams;
  to: PeerId;
}

export interface CommsCommandPeerRequest {
  blocks?: ContentBlock[];
  handling_mode?: HandlingMode;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  stream?: "none" | "reserve_interaction";
  to: PeerId;
}

export interface CommsCommandPeerResponse {
  blocks?: ContentBlock[];
  handling_mode?: HandlingMode;
  in_reply_to: string;
  kind: "peer_response";
  result?: CommsPeerResponseResult;
  status: "accepted" | "completed" | "failed";
  to: PeerId;
}

export type CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse;

export type BlobId = string;

export type BridgeBootstrapToken = string;

export interface BridgeCommandBindMember {
  bootstrap_token: BridgeBootstrapToken;
  command: "bind_member";
  epoch: number;
  expected_address: string;
  expected_peer_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandAuthorizeSupervisor {
  command: "authorize_supervisor";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandRevokeSupervisor {
  command: "revoke_supervisor";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandDeliverMemberInput {
  command: "deliver_member_input";
  content: ContentInput;
  epoch: number;
  handling_mode: HandlingMode;
  input_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandObserveMember {
  command: "observe_member";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandInterruptMember {
  command: "interrupt_member";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandHardCancelMember {
  command: "hard_cancel_member";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  reason: string;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandRetireMember {
  command: "retire_member";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandDestroyMember {
  command: "destroy_member";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandWireMember {
  command: "wire_member";
  epoch: number;
  mob_peer_overlay?: unknown;
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandUnwireMember {
  command: "unwire_member";
  epoch: number;
  mob_peer_overlay?: unknown;
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export type BridgeCommand = BridgeCommandBindMember | BridgeCommandAuthorizeSupervisor | BridgeCommandRevokeSupervisor | BridgeCommandDeliverMemberInput | BridgeCommandObserveMember | BridgeCommandInterruptMember | BridgeCommandHardCancelMember | BridgeCommandRetireMember | BridgeCommandDestroyMember | BridgeCommandWireMember | BridgeCommandUnwireMember;

export interface BridgeDeliveryOutcomeAccepted {
  outcome: "accepted";
}

export interface BridgeDeliveryOutcomeDeduplicated {
  existing_input_id: string;
  outcome: "deduplicated";
}

export interface BridgeDeliveryOutcomeRejected {
  cause: BridgeDeliveryRejectionCause;
  outcome: "rejected";
  reason: string;
}

export type BridgeDeliveryOutcome = BridgeDeliveryOutcomeAccepted | BridgeDeliveryOutcomeDeduplicated | BridgeDeliveryOutcomeRejected;

export interface BridgeDeliveryRejectionCauseNotReady {
  kind: "not_ready";
  state: BridgeMemberRuntimeState;
}

export interface BridgeDeliveryRejectionCauseDurabilityViolation {
  detail: string;
  kind: "durability_violation";
}

export interface BridgeDeliveryRejectionCausePeerHandlingModeInvalid {
  detail: string;
  kind: "peer_handling_mode_invalid";
}

export interface BridgeDeliveryRejectionCauseInternal {
  detail: string;
  kind: "internal";
}

export type BridgeDeliveryRejectionCause = BridgeDeliveryRejectionCauseNotReady | BridgeDeliveryRejectionCauseDurabilityViolation | BridgeDeliveryRejectionCausePeerHandlingModeInvalid | BridgeDeliveryRejectionCauseInternal;

export type BridgeMemberRuntimeState = "initializing" | "idle" | "attached" | "running" | "retired" | "stopped" | "destroyed";

export type BridgePeerConnectivity = "reachable" | "unreachable" | "unknown";

export type BridgeProtocolVersion = number;

export type BridgeRejectionCause = "not_bound" | "stale_supervisor" | "sender_mismatch" | "already_bound" | "invalid_bootstrap_token" | "unsupported_protocol_version" | "invalid_supervisor_spec" | "invalid_peer_spec" | "address_mismatch" | "unsupported" | "internal";

export interface BridgeReplyBindMember {
  address: string;
  capabilities: BridgeCapabilities;
  peer_id: string;
  result: "bind_member";
}

export interface BridgeReplyAck {
  ok: boolean;
  result: "ack";
}

export interface BridgeReplyObservation {
  accepting_inputs?: boolean;
  current_run_id?: string;
  last_error?: string;
  observed_at: string;
  peer_connectivity?: BridgePeerConnectivity;
  result: "observation";
  state: BridgeMemberRuntimeState;
}

export interface BridgeReplyDelivery {
  canonical_input_id?: string;
  input_id: string;
  outcome: BridgeDeliveryOutcome;
  result: "delivery";
}

export interface BridgeReplyRetire {
  inputs_abandoned: number;
  inputs_pending_drain: number;
  result: "retire";
}

export interface BridgeReplyDestroy {
  inputs_abandoned: number;
  result: "destroy";
}

export interface BridgeReplyRejected {
  cause: BridgeRejectionCause;
  reason: string;
  result: "rejected";
}

export type BridgeReply = BridgeReplyBindMember | BridgeReplyAck | BridgeReplyObservation | BridgeReplyDelivery | BridgeReplyRetire | BridgeReplyDestroy | BridgeReplyRejected;

export interface ContentBlockText {
  text: string;
  type: "text";
}

export interface ContentBlockImage {
  media_type: string;
  type: "image";
}

export interface ContentBlockVideo {
  duration_ms: number;
  media_type: string;
  type: "video";
}

export interface ContentBlockStructured {
  data: unknown;
  type: "structured";
}

export interface ContentBlockSkillContext {
  skill_key: SkillKey;
  text: string;
  type: "skill_context";
}

export type ContentBlock = ContentBlockText | ContentBlockImage | ContentBlockVideo | ContentBlockStructured | ContentBlockSkillContext;

export type ContentInput = string | ContentBlock[];

export interface CommsSendParamsInput {
  allow_self_session?: boolean;
  blocks?: ContentBlock[];
  body: string;
  handling_mode?: HandlingMode;
  kind: "input";
  session_id: string;
  source?: "tcp" | "uds" | "stdin" | "webhook" | "rpc";
  stream?: "none" | "reserve_interaction";
}

export interface CommsSendParamsPeerMessage {
  blocks?: ContentBlock[];
  body: string;
  handling_mode?: HandlingMode;
  kind: "peer_message";
  session_id: string;
  to: PeerId;
}

export interface CommsSendParamsPeerLifecycle {
  kind: "peer_lifecycle";
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired" | "mob.dismiss";
  params: CommsPeerLifecycleParams;
  session_id: string;
  to: PeerId;
}

export interface CommsSendParamsPeerRequest {
  blocks?: ContentBlock[];
  handling_mode?: HandlingMode;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  session_id: string;
  stream?: "none" | "reserve_interaction";
  to: PeerId;
}

export interface CommsSendParamsPeerResponse {
  blocks?: ContentBlock[];
  handling_mode?: HandlingMode;
  in_reply_to: string;
  kind: "peer_response";
  result?: CommsPeerResponseResult;
  session_id: string;
  status: "accepted" | "completed" | "failed";
  to: PeerId;
}

export type CommsSendParams = CommsSendParamsInput | CommsSendParamsPeerMessage | CommsSendParamsPeerLifecycle | CommsSendParamsPeerRequest | CommsSendParamsPeerResponse;

export interface CommsSendResultInputAccepted {
  interaction_id: string;
  kind: "input_accepted";
  stream_reserved: boolean;
}

export interface CommsSendResultPeerMessageSent {
  acked: boolean;
  envelope_id: string;
  kind: "peer_message_sent";
}

export interface CommsSendResultPeerLifecycleSent {
  envelope_id: string;
  kind: "peer_lifecycle_sent";
}

export interface CommsSendResultPeerRequestSent {
  envelope_id: string;
  interaction_id: string;
  kind: "peer_request_sent";
  request_id: string;
  stream_reserved: boolean;
}

export interface CommsSendResultPeerResponseSent {
  envelope_id: string;
  in_reply_to: string;
  kind: "peer_response_sent";
}

export type CommsSendResult = CommsSendResultInputAccepted | CommsSendResultPeerMessageSent | CommsSendResultPeerLifecycleSent | CommsSendResultPeerRequestSent | CommsSendResultPeerResponseSent;

export type CommsChecksumTokenResultIntent = "checksum_token";

export type CommsPeerRequestIntent = "supervisor.bridge" | "checksum_token";

export type CommsPeerRequestParams = BridgeCommand | CommsChecksumTokenParams;

export type CommsPeerResponseResult = BridgeReply | CommsChecksumTokenResult;

export type HandlingMode = "queue" | "steer";

export type PeerId = string;

export type PeerName = string;

export type PeerTransport = "inproc" | "uds" | "tcp";

export type PeerDirectorySource = "trusted" | "inproc" | "trusted_and_inproc" | "unknown";

export type PeerSendability = "peer_message" | "peer_request" | "peer_response";

export interface WireRenderMetadata {
  class: "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";
  salience?: "background" | "normal" | "important" | "urgent";
}

export interface WireTrustedPeerIdentityEd25519PublicKey {
  kind: "ed25519_public_key";
  public_key: string;
}

export type WireTrustedPeerIdentity = WireTrustedPeerIdentityEd25519PublicKey;

export interface WireTrustedPeerSpec {
  address: string;
  identity: WireTrustedPeerIdentity;
  name: string;
}

export interface RuntimeStateResult {
  state: WireRuntimeState;
}

export interface RealtimeAudioFormat {
  channels: number;
  mime_type: string;
  sample_rate_hz: number;
}

export interface RealtimeCapabilities {
  audio_input_format?: RealtimeAudioFormat;
  audio_output_format?: RealtimeAudioFormat;
  input_kinds?: RealtimeInputKind[];
  interrupt_supported: boolean;
  output_kinds?: RealtimeOutputKind[];
  tool_lifecycle_events_supported: boolean;
  transcript_supported: boolean;
  turning_modes?: RealtimeTurningMode[];
  video_supported: boolean;
}

export interface RealtimeTextChunk {
  text: string;
}

export interface RealtimeAudioChunk {
  channels: number;
  data: string;
  mime_type: string;
  sample_rate_hz: number;
}

export interface RealtimeVideoChunk {
  data: string;
  mime_type: string;
}

export interface LiveOpenParams {
  session_id: string;
  transport?: "websocket" | "webrtc";
  turning_mode?: RealtimeTurningMode;
}

export interface WireLiveChannelCapabilities {
  audio_in: boolean;
  audio_out: boolean;
  barge_in_supported: boolean;
  image_in: boolean;
  provider_native_resume: boolean;
  text_in: boolean;
  text_out: boolean;
  transcript_supported: boolean;
  video_in: boolean;
}

export interface WireLiveContinuityModeFresh {
  mode: "fresh";
}

export interface WireLiveContinuityModeTranscriptOnly {
  mode: "transcript_only";
}

export interface WireLiveContinuityModeDegraded {
  mode: "degraded";
}

export interface WireLiveContinuityModeProviderNativeResume {
  mode: "provider_native_resume";
  provider_session_id: string;
}

export interface WireLiveContinuityModeUnknown {
  debug: string;
  mode: "unknown";
}

export type WireLiveContinuityMode = WireLiveContinuityModeFresh | WireLiveContinuityModeTranscriptOnly | WireLiveContinuityModeDegraded | WireLiveContinuityModeProviderNativeResume | WireLiveContinuityModeUnknown;

export interface WireLiveTransportBootstrapWebsocket {
  token: string;
  transport: "websocket";
  url: string;
}

export interface WireLiveTransportBootstrapWebrtc {
  answer_method: string;
  http_url?: string;
  token: string;
  transport: "webrtc";
}

export interface WireLiveTransportBootstrapUnknown {
  debug: string;
  transport: "unknown";
}

export type WireLiveTransportBootstrap = WireLiveTransportBootstrapWebsocket | WireLiveTransportBootstrapWebrtc | WireLiveTransportBootstrapUnknown;

export interface LiveOpenResult {
  capabilities: WireLiveChannelCapabilities;
  channel_id: string;
  continuity: WireLiveContinuityMode;
  transport: WireLiveTransportBootstrap;
}

export interface LiveWebrtcAnswerParams {
  channel_id: string;
  offer_sdp: string;
  token: string;
}

export interface LiveWebrtcAnswerResult {
  answer_sdp: string;
}

export interface LiveChannelParams {
  channel_id: string;
}

export interface LiveStatusResult {
  channel_id: string;
  status: WireLiveAdapterStatus;
}

export interface LiveSendInputParams {
  channel_id: string;
  chunk: LiveInputChunkWire;
}

export interface LiveTruncateParams {
  audio_played_ms: number;
  channel_id: string;
  content_index: number;
  item_id: string;
}

export interface WireLiveResponseModalityAudio {
  modality: "audio";
}

export interface WireLiveResponseModalityText {
  modality: "text";
}

export interface WireLiveResponseModalityUnknown {
  debug: string;
  modality: "unknown";
}

export type WireLiveResponseModality = WireLiveResponseModalityAudio | WireLiveResponseModalityText | WireLiveResponseModalityUnknown;

export interface LiveCommitInputParams {
  channel_id: string;
  response_modality?: WireLiveResponseModality;
}

export type LiveRefreshStatus = "queued";

export interface LiveRefreshResult {
  status: "queued";
}

export type LiveCloseStatus = "closed";

export interface LiveCloseResult {
  status: "closed";
}

export interface LiveInputChunkWireAudio {
  channels: number;
  data: string;
  kind: "audio";
  sample_rate_hz: number;
}

export interface LiveInputChunkWireText {
  kind: "text";
  text: string;
}

export interface LiveInputChunkWireImage {
  data: string;
  kind: "image";
  mime: string;
}

export interface LiveInputChunkWireVideoFrame {
  codec: string;
  data: string;
  kind: "video_frame";
  timestamp_ms: number;
}

export type LiveInputChunkWire = LiveInputChunkWireAudio | LiveInputChunkWireText | LiveInputChunkWireImage | LiveInputChunkWireVideoFrame;

export interface RealtimeTranscriptEventItemObserved {
  item_id: string;
  previous_item_id?: string;
  response_id?: string;
  role: RealtimeTranscriptRole;
  type: "item_observed";
}

export interface RealtimeTranscriptEventItemSkipped {
  item_id: string;
  previous_item_id?: string;
  type: "item_skipped";
}

export interface RealtimeTranscriptEventUserTranscriptFinal {
  content_index: number;
  item_id: string;
  previous_item_id?: string;
  text: string;
  type: "user_transcript_final";
}

export interface RealtimeTranscriptEventAssistantTextDelta {
  content_index: number;
  delta: string;
  delta_id: string;
  item_id: string;
  previous_item_id?: string;
  response_id: string;
  type: "assistant_text_delta";
}

export interface RealtimeTranscriptEventAssistantTranscriptDelta {
  content_index: number;
  delta: string;
  delta_id: string;
  item_id: string;
  previous_item_id?: string;
  response_id: string;
  type: "assistant_transcript_delta";
}

export interface RealtimeTranscriptEventAssistantTranscriptTruncated {
  content_index: number;
  item_id: string;
  response_id: string;
  text: string;
  type: "assistant_transcript_truncated";
}

export interface RealtimeTranscriptEventAssistantTranscriptFinalText {
  content_index: number;
  item_id: string;
  response_id: string;
  text: string;
  type: "assistant_transcript_final_text";
}

export interface RealtimeTranscriptEventAssistantTurnCompleted {
  response_id: string;
  stop_reason: unknown;
  type: "assistant_turn_completed";
  usage: unknown;
}

export interface RealtimeTranscriptEventAssistantTurnInterrupted {
  response_id: string;
  type: "assistant_turn_interrupted";
}

export type RealtimeTranscriptEvent = RealtimeTranscriptEventItemObserved | RealtimeTranscriptEventItemSkipped | RealtimeTranscriptEventUserTranscriptFinal | RealtimeTranscriptEventAssistantTextDelta | RealtimeTranscriptEventAssistantTranscriptDelta | RealtimeTranscriptEventAssistantTranscriptTruncated | RealtimeTranscriptEventAssistantTranscriptFinalText | RealtimeTranscriptEventAssistantTurnCompleted | RealtimeTranscriptEventAssistantTurnInterrupted;

export type RealtimeTranscriptRole = "user" | "assistant";

export interface WireLiveDegradationReasonRateLimited {
  kind: "rate_limited";
}

export interface WireLiveDegradationReasonProviderThrottled {
  kind: "provider_throttled";
}

export interface WireLiveDegradationReasonNetworkUnstable {
  kind: "network_unstable";
}

export interface WireLiveDegradationReasonOther {
  detail: string;
  kind: "other";
}

export interface WireLiveDegradationReasonUnknown {
  debug: string;
  kind: "unknown";
}

export type WireLiveDegradationReason = WireLiveDegradationReasonRateLimited | WireLiveDegradationReasonProviderThrottled | WireLiveDegradationReasonNetworkUnstable | WireLiveDegradationReasonOther | WireLiveDegradationReasonUnknown;

export interface WireLiveAdapterStatusIdle {
  status: "idle";
}

export interface WireLiveAdapterStatusOpening {
  status: "opening";
}

export interface WireLiveAdapterStatusReady {
  status: "ready";
}

export interface WireLiveAdapterStatusDegraded {
  reason: WireLiveDegradationReason;
  status: "degraded";
}

export interface WireLiveAdapterStatusClosing {
  status: "closing";
}

export interface WireLiveAdapterStatusClosed {
  status: "closed";
}

export interface WireLiveAdapterStatusUnknown {
  debug: string;
  status: "unknown";
}

export type WireLiveAdapterStatus = WireLiveAdapterStatusIdle | WireLiveAdapterStatusOpening | WireLiveAdapterStatusReady | WireLiveAdapterStatusDegraded | WireLiveAdapterStatusClosing | WireLiveAdapterStatusClosed | WireLiveAdapterStatusUnknown;

export interface WireLiveConfigRejectionReasonChannelIdentitySwap {
  from_model: string;
  from_provider: WireProvider;
  kind: "channel_identity_swap";
  to_model: string;
  to_provider: WireProvider;
}

export interface WireLiveConfigRejectionReasonNonRealtimeResolution {
  detail: string;
  kind: "non_realtime_resolution";
}

export interface WireLiveConfigRejectionReasonImageInputNotImplemented {
  kind: "image_input_not_implemented";
}

export interface WireLiveConfigRejectionReasonVideoFrameInputNotImplemented {
  kind: "video_frame_input_not_implemented";
}

export interface WireLiveConfigRejectionReasonUnsupportedInputChunkVariant {
  kind: "unsupported_input_chunk_variant";
}

export interface WireLiveConfigRejectionReasonRefreshModelSwap {
  from_model: string;
  kind: "refresh_model_swap";
  to_model: string;
}

export interface WireLiveConfigRejectionReasonRefreshProviderSwap {
  from_provider: WireProvider;
  kind: "refresh_provider_swap";
  to_provider: WireProvider;
}

export interface WireLiveConfigRejectionReasonRefreshAudioConfigMismatch {
  detail: string;
  kind: "refresh_audio_config_mismatch";
}

export interface WireLiveConfigRejectionReasonAudioInputFormatMismatch {
  actual_channels: number;
  actual_sample_rate_hz: number;
  expected_channels: number;
  expected_sample_rate_hz: number;
  kind: "audio_input_format_mismatch";
}

export interface WireLiveConfigRejectionReasonOther {
  detail: string;
  kind: "other";
}

export interface WireLiveConfigRejectionReasonUnknown {
  debug: string;
  kind: "unknown";
}

export type WireLiveConfigRejectionReason = WireLiveConfigRejectionReasonChannelIdentitySwap | WireLiveConfigRejectionReasonNonRealtimeResolution | WireLiveConfigRejectionReasonImageInputNotImplemented | WireLiveConfigRejectionReasonVideoFrameInputNotImplemented | WireLiveConfigRejectionReasonUnsupportedInputChunkVariant | WireLiveConfigRejectionReasonRefreshModelSwap | WireLiveConfigRejectionReasonRefreshProviderSwap | WireLiveConfigRejectionReasonRefreshAudioConfigMismatch | WireLiveConfigRejectionReasonAudioInputFormatMismatch | WireLiveConfigRejectionReasonOther | WireLiveConfigRejectionReasonUnknown;

export interface WireLiveAdapterErrorCodeConnectionFailed {
  code: "connection_failed";
}

export interface WireLiveAdapterErrorCodeConnectionLost {
  code: "connection_lost";
}

export interface WireLiveAdapterErrorCodeConfigRejected {
  code: "config_rejected";
  reason: WireLiveConfigRejectionReason;
}

export interface WireLiveAdapterErrorCodeProviderError {
  code: "provider_error";
}

export interface WireLiveAdapterErrorCodeAuthenticationFailed {
  code: "authentication_failed";
}

export interface WireLiveAdapterErrorCodeInternalError {
  code: "internal_error";
}

export interface WireLiveAdapterErrorCodeOther {
  code: "other";
  raw: string;
}

export interface WireLiveAdapterErrorCodeUnknown {
  code: "unknown";
  debug: string;
}

export type WireLiveAdapterErrorCode = WireLiveAdapterErrorCodeConnectionFailed | WireLiveAdapterErrorCodeConnectionLost | WireLiveAdapterErrorCodeConfigRejected | WireLiveAdapterErrorCodeProviderError | WireLiveAdapterErrorCodeAuthenticationFailed | WireLiveAdapterErrorCodeInternalError | WireLiveAdapterErrorCodeOther | WireLiveAdapterErrorCodeUnknown;

export interface WireLiveAdapterObservationReady {
  observation: "ready";
}

export interface WireLiveAdapterObservationUserTranscriptFinal {
  content_index?: number;
  observation: "user_transcript_final";
  previous_item_id?: string;
  provider_item_id?: string;
  text: string;
}

export interface WireLiveAdapterObservationAssistantTextDelta {
  content_index?: number;
  delta: string;
  delta_id?: string;
  observation: "assistant_text_delta";
  previous_item_id?: string;
  provider_item_id?: string;
  response_id?: string;
}

export interface WireLiveAdapterObservationAssistantTranscriptDelta {
  content_index?: number;
  delta: string;
  delta_id?: string;
  observation: "assistant_transcript_delta";
  previous_item_id?: string;
  provider_item_id?: string;
  response_id?: string;
}

export interface WireLiveAdapterObservationAssistantAudioChunk {
  channels: number;
  content_index?: number;
  data: string;
  item_id?: string;
  observation: "assistant_audio_chunk";
  response_id?: string;
  sample_rate_hz: number;
}

export interface WireLiveAdapterObservationAssistantTranscriptFinal {
  content_index?: number;
  observation: "assistant_transcript_final";
  previous_item_id?: string;
  provider_item_id: string;
  response_id?: string;
  stop_reason: "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";
  text: string;
  usage: Record<string, unknown>;
}

export interface WireLiveAdapterObservationAssistantTranscriptTruncated {
  content_index?: number;
  observation: "assistant_transcript_truncated";
  previous_item_id?: string;
  provider_item_id?: string;
  response_id?: string;
  text?: string;
}

export interface WireLiveAdapterObservationRealtimeTranscript {
  event: RealtimeTranscriptEvent;
  observation: "realtime_transcript";
}

export interface WireLiveAdapterObservationToolCallRequested {
  arguments: unknown;
  observation: "tool_call_requested";
  provider_call_id: string;
  tool_name: string;
}

export interface WireLiveAdapterObservationTurnInterrupted {
  observation: "turn_interrupted";
  response_id?: string;
}

export interface WireLiveAdapterObservationTurnCompleted {
  observation: "turn_completed";
  response_id?: string;
  stop_reason: "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";
  usage: Record<string, unknown>;
}

export interface WireLiveAdapterObservationStatusChanged {
  observation: "status_changed";
  status: WireLiveAdapterStatus;
}

export interface WireLiveAdapterObservationError {
  code: WireLiveAdapterErrorCode;
  message: string;
  observation: "error";
}

export interface WireLiveAdapterObservationCommandRejected {
  code: WireLiveAdapterErrorCode;
  message: string;
  observation: "command_rejected";
}

export interface WireLiveAdapterObservationUnknown {
  debug: string;
  observation: "unknown";
}

export type WireLiveAdapterObservation = WireLiveAdapterObservationReady | WireLiveAdapterObservationUserTranscriptFinal | WireLiveAdapterObservationAssistantTextDelta | WireLiveAdapterObservationAssistantTranscriptDelta | WireLiveAdapterObservationAssistantAudioChunk | WireLiveAdapterObservationAssistantTranscriptFinal | WireLiveAdapterObservationAssistantTranscriptTruncated | WireLiveAdapterObservationRealtimeTranscript | WireLiveAdapterObservationToolCallRequested | WireLiveAdapterObservationTurnInterrupted | WireLiveAdapterObservationTurnCompleted | WireLiveAdapterObservationStatusChanged | WireLiveAdapterObservationError | WireLiveAdapterObservationCommandRejected | WireLiveAdapterObservationUnknown;

export interface RuntimeAcceptResult {
  existing_id?: string;
  input_id?: string;
  outcome_type: "accepted" | "deduplicated" | "rejected";
  policy?: "stage" | "queue" | "immediate";
  reason?: string;
  state?: Record<string, unknown>;
}

export interface WireInputStateHistoryEntry {
  from: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
  reason?: string;
  timestamp: string;
  to: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
}

export interface WireInputState {
  attempt_count?: number;
  created_at: string;
  current_state: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
  durability?: "durable" | "volatile" | "ephemeral";
  history?: Record<string, unknown>[];
  idempotency_key?: string;
  input_id: string;
  last_boundary_sequence?: number;
  last_run_id?: string;
  persisted_input?: Record<string, unknown>;
  policy?: "stage" | "queue" | "immediate";
  reconstruction_source?: "live" | "event_store" | "snapshot" | "replay";
  recovery_count?: number;
  terminal_outcome?: "completed" | "abandoned" | "superseded" | "coalesced" | "cancelled";
  updated_at: string;
}

export interface ScheduleListResult {
  schedules: Record<string, unknown>[];
}

export interface ScheduleOccurrencesResult {
  occurrences: Record<string, unknown>[];
}

export interface WireResolvedModelCapabilities {
  image_generation?: boolean;
  image_input?: boolean;
  image_tool_results?: boolean;
  inline_video?: boolean;
  realtime?: boolean;
  vision?: boolean;
  web_search?: boolean;
}

export interface WireSessionInfo {
  created_at: number;
  is_active: boolean;
  labels?: Record<string, string>;
  last_assistant_text?: string;
  message_count: number;
  model: string;
  provider: string;
  resolved_capabilities?: WireResolvedModelCapabilities;
  session_id: string;
  session_ref?: string;
  updated_at: number;
}

export interface WireSessionSummary {
  created_at: number;
  is_active: boolean;
  labels?: Record<string, string>;
  message_count: number;
  session_id: string;
  session_ref?: string;
  total_tokens: number;
  updated_at: number;
}

export interface ContractVersion {
}

export interface CatalogModelEntry {
  context_window?: number;
  display_name: string;
  id: string;
  max_output_tokens?: number;
  profile?: Record<string, unknown>;
  server_id?: string;
  tier: "recommended" | "supported";
}

export interface ProviderCatalog {
  default_model_id: string;
  models: Record<string, unknown>[];
  provider: string;
}

export interface ModelsCatalogResponse {
  contract_version: Record<string, unknown>;
  providers: Record<string, unknown>[];
}

export interface WireModelProfile {
  beta_headers?: Record<string, unknown>[];
  image_generation?: boolean;
  image_input?: boolean;
  image_tool_results?: boolean;
  inline_video?: boolean;
  model_family: string;
  params_schema: unknown;
  realtime?: boolean;
  supports_reasoning: boolean;
  supports_temperature: boolean;
  supports_thinking: boolean;
  supports_web_search?: boolean;
  vision?: boolean;
}

export interface WireAssistantImageRef {
  blob_ref: Record<string, unknown>;
  height: number;
  image_id: string;
  media_type: string;
  width: number;
}

export interface WireGenerateImageRequest {
  count: number;
  format: "auto" | "png" | "jpeg" | "webp";
  intent: Record<string, unknown>;
  provider_params?: unknown;
  quality: "auto" | "low" | "medium" | "high";
  size: Record<string, unknown>;
  target: Record<string, unknown>;
}

export interface WireGenerateImageExecutionPlan {
  backend: "hosted_tool" | "provider_api" | "native_model";
  capabilities: Record<string, unknown>;
  max_count: number;
  provider: string;
  provider_plan?: unknown;
  requires_scoped_override: boolean;
}

export interface WireImageGenerationToolResult {
  images?: Record<string, unknown>[];
  native_metadata: Record<string, unknown>;
  operation_id: string;
  provider_text: Record<string, unknown>;
  revised_prompt: Record<string, unknown>;
  terminal: Record<string, unknown>;
  warnings?: Record<string, unknown>[];
}

export interface WireAuthBindingRef {
  binding: string;
  profile?: string;
  realm: string;
}

export interface WireBackendProfile {
  backend_kind: string;
  base_url?: string;
  id: string;
  options?: unknown;
  provider: string;
}

export interface WireAuthProfile {
  auth_method: string;
  id: string;
  provider: string;
  source_kind: string;
}

export interface WireProviderBinding {
  allow_auth_override?: boolean;
  auth_profile: string;
  backend_profile: string;
  default_model?: string;
  id: string;
  require_metadata_account?: boolean;
  require_metadata_workspace?: boolean;
}

export interface WireRealmConnectionSet {
  auth_profiles: Record<string, Record<string, unknown>>;
  backends: Record<string, Record<string, unknown>>;
  bindings: Record<string, Record<string, unknown>>;
  default_binding?: string;
  realm_id: string;
}

export interface WireBindingIdentity {
  auth_binding: WireAuthBindingRef;
  binding_id: string;
  realm_id: string;
}

export interface WireAuthProfileCreated {
  auth_binding: WireAuthBindingRef;
  auth_method: string;
  binding_id: string;
  profile_id: string;
  provider: string;
  realm_id: string;
  stored: boolean;
}

export interface WireAuthProfileDetail {
  auth_binding: WireAuthBindingRef;
  auth_profile: Record<string, unknown>;
  binding_id: string;
  profile_id: string;
}

export interface WireAuthProfileCleared {
  auth_binding: WireAuthBindingRef;
  binding_id: string;
  cleared: boolean;
  profile_id: string;
  realm_id: string;
}

export interface WireLoginStart {
  authorize_url: string;
  provider: string;
  redirect_uri: string;
  state: string;
}

export interface WireLoginReady {
  auth_binding: WireAuthBindingRef;
  binding_id: string;
  expires_at?: string;
  has_refresh_token: boolean;
  profile_id: string;
  provider: string;
  realm_id: string;
  scopes: string[];
  state?: string;
}

export interface WireDeviceStart {
  device_code: string;
  expires_in: number;
  interval: number;
  provider: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete?: string;
}

export interface WireRealmSummary {
  auth_profile_count: number;
  backend_count: number;
  binding_count: number;
  default_binding?: string;
  realm_id: string;
}

export interface WireRealmList {
  realms: Record<string, unknown>[];
}

export interface WireAuthProfilesList {
  auth_profiles: Record<string, unknown>[];
  backend_profiles: Record<string, unknown>[];
  bindings: Record<string, unknown>[];
  realm_id: string;
}

export interface WireAuthStatus {
  account_id?: string;
  auth_method: string;
  expires_at?: string;
  last_error?: Record<string, unknown>;
  last_refresh_at?: string;
  profile_id: string;
  provider: string;
  state: "valid" | "expiring" | "expired" | "reauth_required" | "refresh_failed" | "released" | "absent" | "missing_credential";
}

export interface WireAuthStatusDetail {
  account_id?: string;
  auth_binding: WireAuthBindingRef;
  auth_method: string;
  binding_id: string;
  expires_at?: string;
  has_refresh_token: boolean;
  last_refresh_at?: string;
  profile_id: string;
  provider: string;
  realm_id: string;
  state: "valid" | "expiring" | "expired" | "reauth_required" | "refresh_failed" | "released" | "absent" | "missing_credential";
}

export interface WireAuthErrorMissingSecret {
  kind: "missing_secret";
}

export interface WireAuthErrorUnsupportedCombination {
  auth: string;
  backend: string;
  kind: "unsupported_combination";
}

export interface WireAuthErrorMissingRequiredMetadata {
  field: string;
  kind: "missing_required_metadata";
}

export interface WireAuthErrorWorkspaceMismatch {
  kind: "workspace_mismatch";
}

export interface WireAuthErrorExpired {
  kind: "expired";
}

export interface WireAuthErrorStaleCredential {
  kind: "stale_credential";
}

export interface WireAuthErrorRefreshRequired {
  kind: "refresh_required";
}

export interface WireAuthErrorLeaseAbsent {
  kind: "lease_absent";
}

export interface WireAuthErrorUserReauthRequired {
  kind: "user_reauth_required";
}

export interface WireAuthErrorRefreshFailed {
  detail: string;
  kind: "refresh_failed";
}

export interface WireAuthErrorResolveRequired {
  detail: string;
  kind: "resolve_required";
}

export interface WireAuthErrorInteractiveLoginRequired {
  kind: "interactive_login_required";
}

export interface WireAuthErrorHostOwnedUnavailable {
  kind: "host_owned_unavailable";
}

export interface WireAuthErrorIo {
  detail: string;
  kind: "io";
}

export interface WireAuthErrorOther {
  detail: string;
  kind: "other";
}

export type WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorStaleCredential | WireAuthErrorRefreshRequired | WireAuthErrorLeaseAbsent | WireAuthErrorUserReauthRequired | WireAuthErrorRefreshFailed | WireAuthErrorResolveRequired | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther;

export type WireProvider = "anthropic" | "openai" | "gemini" | "self_hosted" | "other" | "unknown";

export interface WireTranscriptSourceSpoken {
  kind: "spoken";
}

export interface WireTranscriptSourceUnknown {
  debug: string;
  kind: "unknown";
}

export type WireTranscriptSource = WireTranscriptSourceSpoken | WireTranscriptSourceUnknown;

export interface WireAssistantBlockText {
  block_type: "text";
  data: { meta?: Record<string, unknown>; text: string };
}

export interface WireAssistantBlockTranscript {
  block_type: "transcript";
  data: { meta?: Record<string, unknown>; source: WireTranscriptSource; text: string };
}

export interface WireAssistantBlockReasoning {
  block_type: "reasoning";
  data: { meta?: Record<string, unknown>; text?: string };
}

export interface WireAssistantBlockToolUse {
  block_type: "tool_use";
  data: { args: unknown; id: string; meta?: Record<string, unknown>; name: string };
}

export interface WireAssistantBlockServerToolContent {
  block_type: "server_tool_content";
  data: { content: unknown; id?: string; kind: Record<string, unknown>; meta?: Record<string, unknown> };
}

export interface WireAssistantBlockImage {
  block_type: "image";
  data: { blob_ref: Record<string, unknown>; height: number; image_id: string; media_type: string; meta: Record<string, unknown>; revised_prompt: Record<string, unknown>; width: number };
}

export interface WireAssistantBlockUnknown {
  block_type: "unknown";
}

export type WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockTranscript | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockServerToolContent | WireAssistantBlockImage | WireAssistantBlockUnknown;

export interface WireImageOperationPhaseRequested {
  phase: "requested";
}

export interface WireImageOperationPhaseValidating {
  phase: "validating";
}

export interface WireImageOperationPhaseAwaitingApproval {
  approval_id: string;
  phase: "awaiting_approval";
}

export interface WireImageOperationPhasePlanResolved {
  phase: "plan_resolved";
}

export interface WireImageOperationPhaseProjectionSnapshotted {
  phase: "projection_snapshotted";
}

export interface WireImageOperationPhaseScopedOverrideActive {
  phase: "scoped_override_active";
}

export interface WireImageOperationPhaseProviderCallInFlight {
  phase: "provider_call_in_flight";
}

export interface WireImageOperationPhaseProviderResultCaptured {
  phase: "provider_result_captured";
}

export interface WireImageOperationPhaseBlobCommitPending {
  phase: "blob_commit_pending";
}

export interface WireImageOperationPhaseResultCommitted {
  phase: "result_committed";
}

export interface WireImageOperationPhaseRestoringScopedOverride {
  phase: "restoring_scoped_override";
}

export interface WireImageOperationPhaseTerminal {
  phase: "terminal";
  terminal: Record<string, unknown>;
}

export type WireImageOperationPhase = WireImageOperationPhaseRequested | WireImageOperationPhaseValidating | WireImageOperationPhaseAwaitingApproval | WireImageOperationPhasePlanResolved | WireImageOperationPhaseProjectionSnapshotted | WireImageOperationPhaseScopedOverrideActive | WireImageOperationPhaseProviderCallInFlight | WireImageOperationPhaseProviderResultCaptured | WireImageOperationPhaseBlobCommitPending | WireImageOperationPhaseResultCommitted | WireImageOperationPhaseRestoringScopedOverride | WireImageOperationPhaseTerminal;

/** Fail-closed wire parser for WorkItem (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItem(value: unknown): WorkItem {
  const data = expectWireObject(value, "WorkItem");
  return {
    ...(data["claim"] === undefined || data["claim"] === null ? {} : { claim: parseWorkItemClaim(data["claim"]) }),
    completion_policy: parseWorkCompletionPolicy(requireWireField(data, "completion_policy", "WorkItem")),
    created_at: expectWireString(requireWireField(data, "created_at", "WorkItem"), "WorkItem.created_at"),
    ...(data["description"] === undefined || data["description"] === null ? {} : { description: expectWireString(data["description"], "WorkItem.description") }),
    ...(data["due_at"] === undefined || data["due_at"] === null ? {} : { due_at: expectWireString(data["due_at"], "WorkItem.due_at") }),
    ...(data["evidence_refs"] === undefined || data["evidence_refs"] === null ? {} : { evidence_refs: expectWireArray(data["evidence_refs"], "WorkItem.evidence_refs").map((entry) => parseWorkEvidenceRef(entry)) }),
    ...(data["external_refs"] === undefined || data["external_refs"] === null ? {} : { external_refs: expectWireArray(data["external_refs"], "WorkItem.external_refs").map((entry) => parseWorkItemExternalRef(entry)) }),
    id: expectWireString(requireWireField(data, "id", "WorkItem"), "WorkItem.id"),
    ...(data["labels"] === undefined || data["labels"] === null ? {} : { labels: expectWireArray(data["labels"], "WorkItem.labels").map((entry) => expectWireString(entry, "WorkItem.labels[]")) }),
    machine_state: expectWireObject(requireWireField(data, "machine_state", "WorkItem"), "WorkItem.machine_state"),
    namespace: expectWireString(requireWireField(data, "namespace", "WorkItem"), "WorkItem.namespace"),
    ...(data["not_before"] === undefined || data["not_before"] === null ? {} : { not_before: expectWireString(data["not_before"], "WorkItem.not_before") }),
    ...(data["owner"] === undefined || data["owner"] === null ? {} : { owner: parseWorkItemOwner(data["owner"]) }),
    priority: expectWireEnum(requireWireField(data, "priority", "WorkItem"), ["low", "medium", "high"], "WorkItem.priority") as "low" | "medium" | "high",
    realm_id: expectWireString(requireWireField(data, "realm_id", "WorkItem"), "WorkItem.realm_id"),
    revision: expectWireInteger(requireWireField(data, "revision", "WorkItem"), "WorkItem.revision"),
    ...(data["snoozed_until"] === undefined || data["snoozed_until"] === null ? {} : { snoozed_until: expectWireString(data["snoozed_until"], "WorkItem.snoozed_until") }),
    status: expectWireEnum(requireWireField(data, "status", "WorkItem"), ["open", "in_progress", "blocked", "completed", "cancelled", "failed"], "WorkItem.status") as "open" | "in_progress" | "blocked" | "completed" | "cancelled" | "failed",
    ...(data["terminal_at"] === undefined || data["terminal_at"] === null ? {} : { terminal_at: expectWireString(data["terminal_at"], "WorkItem.terminal_at") }),
    title: expectWireString(requireWireField(data, "title", "WorkItem"), "WorkItem.title"),
    updated_at: expectWireString(requireWireField(data, "updated_at", "WorkItem"), "WorkItem.updated_at"),
  };
}

/** Fail-closed wire parser for WorkEdge (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkEdge(value: unknown): WorkEdge {
  const data = expectWireObject(value, "WorkEdge");
  return {
    created_at: expectWireString(requireWireField(data, "created_at", "WorkEdge"), "WorkEdge.created_at"),
    from_id: expectWireString(requireWireField(data, "from_id", "WorkEdge"), "WorkEdge.from_id"),
    kind: parseWorkEdgeKind(requireWireField(data, "kind", "WorkEdge")),
    namespace: expectWireString(requireWireField(data, "namespace", "WorkEdge"), "WorkEdge.namespace"),
    realm_id: expectWireString(requireWireField(data, "realm_id", "WorkEdge"), "WorkEdge.realm_id"),
    to_id: expectWireString(requireWireField(data, "to_id", "WorkEdge"), "WorkEdge.to_id"),
  };
}

/** Fail-closed wire parser for WorkGraphEvent (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkGraphEvent(value: unknown): WorkGraphEvent {
  const data = expectWireObject(value, "WorkGraphEvent");
  return {
    at: expectWireString(requireWireField(data, "at", "WorkGraphEvent"), "WorkGraphEvent.at"),
    ...(data["item_id"] === undefined || data["item_id"] === null ? {} : { item_id: expectWireString(data["item_id"], "WorkGraphEvent.item_id") }),
    kind: parseWorkGraphEventKind(requireWireField(data, "kind", "WorkGraphEvent")),
    namespace: expectWireString(requireWireField(data, "namespace", "WorkGraphEvent"), "WorkGraphEvent.namespace"),
    ...(data["payload"] === undefined || data["payload"] === null ? {} : { payload: data["payload"] }),
    realm_id: expectWireString(requireWireField(data, "realm_id", "WorkGraphEvent"), "WorkGraphEvent.realm_id"),
    ...(data["seq"] === undefined || data["seq"] === null ? {} : { seq: expectWireInteger(data["seq"], "WorkGraphEvent.seq") }),
  };
}

/** Fail-closed wire parser for WorkGraphItemsResponse (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkGraphItemsResponse(value: unknown): WorkGraphItemsResponse {
  const data = expectWireObject(value, "WorkGraphItemsResponse");
  return {
    items: expectWireArray(requireWireField(data, "items", "WorkGraphItemsResponse"), "WorkGraphItemsResponse.items").map((entry) => parseWorkItem(entry)),
  };
}

/** Fail-closed wire parser for WorkGraphEventsResponse (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkGraphEventsResponse(value: unknown): WorkGraphEventsResponse {
  const data = expectWireObject(value, "WorkGraphEventsResponse");
  return {
    events: expectWireArray(requireWireField(data, "events", "WorkGraphEventsResponse"), "WorkGraphEventsResponse.events").map((entry) => parseWorkGraphEvent(entry)),
  };
}

/** Fail-closed wire parser for WorkGraphSnapshot (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkGraphSnapshot(value: unknown): WorkGraphSnapshot {
  const data = expectWireObject(value, "WorkGraphSnapshot");
  return {
    all_namespaces: expectWireBoolean(requireWireField(data, "all_namespaces", "WorkGraphSnapshot"), "WorkGraphSnapshot.all_namespaces"),
    ...(data["attention"] === undefined || data["attention"] === null ? {} : { attention: expectWireArray(data["attention"], "WorkGraphSnapshot.attention").map((entry) => parseWorkAttentionBinding(entry)) }),
    captured_at: expectWireString(requireWireField(data, "captured_at", "WorkGraphSnapshot"), "WorkGraphSnapshot.captured_at"),
    edges: expectWireArray(requireWireField(data, "edges", "WorkGraphSnapshot"), "WorkGraphSnapshot.edges").map((entry) => parseWorkEdge(entry)),
    ...(data["event_high_water_mark"] === undefined || data["event_high_water_mark"] === null ? {} : { event_high_water_mark: expectWireInteger(data["event_high_water_mark"], "WorkGraphSnapshot.event_high_water_mark") }),
    items: expectWireArray(requireWireField(data, "items", "WorkGraphSnapshot"), "WorkGraphSnapshot.items").map((entry) => parseWorkItem(entry)),
    ...(data["namespace"] === undefined || data["namespace"] === null ? {} : { namespace: expectWireString(data["namespace"], "WorkGraphSnapshot.namespace") }),
    ready_item_ids: expectWireArray(requireWireField(data, "ready_item_ids", "WorkGraphSnapshot"), "WorkGraphSnapshot.ready_item_ids").map((entry) => expectWireString(entry, "WorkGraphSnapshot.ready_item_ids[]")),
    realm_id: expectWireString(requireWireField(data, "realm_id", "WorkGraphSnapshot"), "WorkGraphSnapshot.realm_id"),
  };
}

/** Fail-closed wire parser for WorkItemsResult (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItemsResult(value: unknown): WorkItemsResult {
  const data = expectWireObject(value, "WorkItemsResult");
  return {
    items: expectWireArray(requireWireField(data, "items", "WorkItemsResult"), "WorkItemsResult.items"),
  };
}

/** Fail-closed wire parser for WorkEventsResult (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkEventsResult(value: unknown): WorkEventsResult {
  const data = expectWireObject(value, "WorkEventsResult");
  return {
    events: expectWireArray(requireWireField(data, "events", "WorkEventsResult"), "WorkEventsResult.events"),
  };
}

/** Fail-closed wire parser for GoalStatusResult (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseGoalStatusResult(value: unknown): GoalStatusResult {
  const data = expectWireObject(value, "GoalStatusResult");
  return {
    attention: parseWorkAttentionBinding(requireWireField(data, "attention", "GoalStatusResult")),
    item: parseWorkItem(requireWireField(data, "item", "GoalStatusResult")),
  };
}

/** Fail-closed wire parser for AttentionListResult (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseAttentionListResult(value: unknown): AttentionListResult {
  const data = expectWireObject(value, "AttentionListResult");
  return {
    attention: expectWireArray(requireWireField(data, "attention", "AttentionListResult"), "AttentionListResult.attention").map((entry) => parseWorkAttentionBinding(entry)),
  };
}

/** Fail-closed wire parser for WorkItemClaim (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItemClaim(value: unknown): WorkItemClaim {
  const data = expectWireObject(value, "WorkItemClaim");
  return {
    claimed_at: expectWireString(requireWireField(data, "claimed_at", "WorkItemClaim"), "WorkItemClaim.claimed_at"),
    ...(data["lease_expires_at"] === undefined || data["lease_expires_at"] === null ? {} : { lease_expires_at: expectWireString(data["lease_expires_at"], "WorkItemClaim.lease_expires_at") }),
    owner: parseWorkItemOwner(requireWireField(data, "owner", "WorkItemClaim")),
  };
}

/** Fail-closed wire parser for WorkCompletionPolicy (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkCompletionPolicy(value: unknown): WorkCompletionPolicy {
  const data = expectWireObject(value, "WorkCompletionPolicy");
  const tag = expectWireString(requireWireField(data, "kind", "WorkCompletionPolicy"), "WorkCompletionPolicy.kind");
  switch (tag) {
    case "self_attest":
      return {
        kind: "self_attest",
      };
    case "host_confirmed":
      return {
        kind: "host_confirmed",
      };
    case "principal_confirmed":
      return {
        kind: "principal_confirmed",
      };
    case "supervisor":
      return {
        kind: "supervisor",
        owner_key: parseWorkOwnerKey(requireWireField(data, "owner_key", "WorkCompletionPolicy")),
      };
    case "reviewer_quorum":
      return {
        kind: "reviewer_quorum",
        threshold: expectWireInteger(requireWireField(data, "threshold", "WorkCompletionPolicy"), "WorkCompletionPolicy.reviewer_quorum.threshold"),
      };
    default:
      throw wireParseError("WorkCompletionPolicy", `unknown \`kind\` value \`${tag}\``);
  }
}

/** Fail-closed wire parser for WorkEvidenceRef (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkEvidenceRef(value: unknown): WorkEvidenceRef {
  const data = expectWireObject(value, "WorkEvidenceRef");
  return {
    ...(data["confirmation_kind"] === undefined || data["confirmation_kind"] === null ? {} : { confirmation_kind: parseWorkEvidenceKind(data["confirmation_kind"]) }),
    ...(data["confirming_owner_key"] === undefined || data["confirming_owner_key"] === null ? {} : { confirming_owner_key: parseWorkOwnerKey(data["confirming_owner_key"]) }),
    id: expectWireString(requireWireField(data, "id", "WorkEvidenceRef"), "WorkEvidenceRef.id"),
    kind: expectWireString(requireWireField(data, "kind", "WorkEvidenceRef"), "WorkEvidenceRef.kind"),
    ...(data["label"] === undefined || data["label"] === null ? {} : { label: expectWireString(data["label"], "WorkEvidenceRef.label") }),
    ...(data["summary"] === undefined || data["summary"] === null ? {} : { summary: expectWireString(data["summary"], "WorkEvidenceRef.summary") }),
  };
}

/** Fail-closed wire parser for WorkItemExternalRef (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItemExternalRef(value: unknown): WorkItemExternalRef {
  const data = expectWireObject(value, "WorkItemExternalRef");
  return {
    id: expectWireString(requireWireField(data, "id", "WorkItemExternalRef"), "WorkItemExternalRef.id"),
    kind: expectWireString(requireWireField(data, "kind", "WorkItemExternalRef"), "WorkItemExternalRef.kind"),
    ...(data["url"] === undefined || data["url"] === null ? {} : { url: expectWireString(data["url"], "WorkItemExternalRef.url") }),
  };
}

/** Fail-closed wire parser for WorkItemOwner (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItemOwner(value: unknown): WorkItemOwner {
  const data = expectWireObject(value, "WorkItemOwner");
  return {
    ...(data["display_name"] === undefined || data["display_name"] === null ? {} : { display_name: expectWireString(data["display_name"], "WorkItemOwner.display_name") }),
    key: parseWorkOwnerKey(requireWireField(data, "key", "WorkItemOwner")),
  };
}

/** Fail-closed wire parser for WorkEdgeKind (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkEdgeKind(value: unknown): WorkEdgeKind {
  return expectWireEnum(value, ["blocks", "parent", "related", "supersedes", "derived_from"], "WorkEdgeKind") as WorkEdgeKind;
}

/** Fail-closed wire parser for WorkGraphEventKind (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkGraphEventKind(value: unknown): WorkGraphEventKind {
  return expectWireEnum(value, ["created", "updated", "claimed", "released", "blocked", "closed", "linked", "evidence_added", "attention_created", "attention_updated"], "WorkGraphEventKind") as WorkGraphEventKind;
}

/** Fail-closed wire parser for WorkAttentionBinding (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkAttentionBinding(value: unknown): WorkAttentionBinding {
  const data = expectWireObject(value, "WorkAttentionBinding");
  return {
    binding_id: expectWireString(requireWireField(data, "binding_id", "WorkAttentionBinding"), "WorkAttentionBinding.binding_id"),
    created_at: expectWireString(requireWireField(data, "created_at", "WorkAttentionBinding"), "WorkAttentionBinding.created_at"),
    delegated_authority: parseAttentionDelegatedAuthority(requireWireField(data, "delegated_authority", "WorkAttentionBinding")),
    ...(data["machine_state"] === undefined || data["machine_state"] === null ? {} : { machine_state: expectWireObject(data["machine_state"], "WorkAttentionBinding.machine_state") }),
    mode: parseWorkAttentionMode(requireWireField(data, "mode", "WorkAttentionBinding")),
    ...(data["projection_policy"] === undefined || data["projection_policy"] === null ? {} : { projection_policy: parseAttentionProjectionPolicy(data["projection_policy"]) }),
    status: parseWorkAttentionStatus(requireWireField(data, "status", "WorkAttentionBinding")),
    target: parseWorkAttentionTarget(requireWireField(data, "target", "WorkAttentionBinding")),
    updated_at: expectWireString(requireWireField(data, "updated_at", "WorkAttentionBinding"), "WorkAttentionBinding.updated_at"),
    work_ref: parseWorkItemRef(requireWireField(data, "work_ref", "WorkAttentionBinding")),
  };
}

/** Fail-closed wire parser for WorkOwnerKey (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkOwnerKey(value: unknown): WorkOwnerKey {
  const data = expectWireObject(value, "WorkOwnerKey");
  return {
    id: expectWireString(requireWireField(data, "id", "WorkOwnerKey"), "WorkOwnerKey.id"),
    kind: parseWorkOwnerKind(requireWireField(data, "kind", "WorkOwnerKey")),
  };
}

/** Fail-closed wire parser for WorkEvidenceKind (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkEvidenceKind(value: unknown): WorkEvidenceKind {
  return expectWireEnum(value, ["host_confirmation", "principal_confirmation", "supervisor_confirmation", "reviewer_confirmation", "self_attest"], "WorkEvidenceKind") as WorkEvidenceKind;
}

/** Fail-closed wire parser for AttentionDelegatedAuthority (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseAttentionDelegatedAuthority(value: unknown): AttentionDelegatedAuthority {
  return expectWireEnum(value, ["add_evidence", "close_own_review_item", "request_closure", "close_if_policy_allows"], "AttentionDelegatedAuthority") as AttentionDelegatedAuthority;
}

/** Fail-closed wire parser for WorkAttentionMode (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkAttentionMode(value: unknown): WorkAttentionMode {
  return expectWireEnum(value, ["pursue", "coordinate", "review", "falsify", "judge", "observe"], "WorkAttentionMode") as WorkAttentionMode;
}

/** Fail-closed wire parser for AttentionProjectionPolicy (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseAttentionProjectionPolicy(value: unknown): AttentionProjectionPolicy {
  const data = expectWireObject(value, "AttentionProjectionPolicy");
  return {
    ...(data["include_parent_context"] === undefined || data["include_parent_context"] === null ? {} : { include_parent_context: expectWireBoolean(data["include_parent_context"], "AttentionProjectionPolicy.include_parent_context") }),
    ...(data["max_text_chars"] === undefined || data["max_text_chars"] === null ? {} : { max_text_chars: expectWireInteger(data["max_text_chars"], "AttentionProjectionPolicy.max_text_chars") }),
  };
}

/** Fail-closed wire parser for WorkAttentionStatus (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkAttentionStatus(value: unknown): WorkAttentionStatus {
  const data = expectWireObject(value, "WorkAttentionStatus");
  const tag = expectWireString(requireWireField(data, "state", "WorkAttentionStatus"), "WorkAttentionStatus.state");
  switch (tag) {
    case "active":
      return {
        state: "active",
      };
    case "paused":
      return {
        state: "paused",
        ...(data["until"] === undefined || data["until"] === null ? {} : { until: expectWireString(data["until"], "WorkAttentionStatus.paused.until") }),
      };
    case "superseded":
      return {
        state: "superseded",
      };
    case "stopped":
      return {
        state: "stopped",
      };
    default:
      throw wireParseError("WorkAttentionStatus", `unknown \`state\` value \`${tag}\``);
  }
}

/** Fail-closed wire parser for WorkAttentionTarget (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkAttentionTarget(value: unknown): WorkAttentionTarget {
  const data = expectWireObject(value, "WorkAttentionTarget");
  const tag = expectWireString(requireWireField(data, "kind", "WorkAttentionTarget"), "WorkAttentionTarget.kind");
  switch (tag) {
    case "session":
      return {
        kind: "session",
        session_id: expectWireString(requireWireField(data, "session_id", "WorkAttentionTarget"), "WorkAttentionTarget.session.session_id"),
      };
    case "lowered_owner":
      return {
        kind: "lowered_owner",
        owner_key: parseWorkOwnerKey(requireWireField(data, "owner_key", "WorkAttentionTarget")),
      };
    default:
      throw wireParseError("WorkAttentionTarget", `unknown \`kind\` value \`${tag}\``);
  }
}

/** Fail-closed wire parser for WorkItemRef (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkItemRef(value: unknown): WorkItemRef {
  const data = expectWireObject(value, "WorkItemRef");
  return {
    item_id: expectWireString(requireWireField(data, "item_id", "WorkItemRef"), "WorkItemRef.item_id"),
    namespace: expectWireString(requireWireField(data, "namespace", "WorkItemRef"), "WorkItemRef.namespace"),
    realm_id: expectWireString(requireWireField(data, "realm_id", "WorkItemRef"), "WorkItemRef.realm_id"),
  };
}

/** Fail-closed wire parser for WorkOwnerKind (K21): throws MeerkatError(INVALID_RESPONSE). */
export function parseWorkOwnerKind(value: unknown): WorkOwnerKind {
  return expectWireEnum(value, ["principal", "agent", "session", "mob", "label"], "WorkOwnerKind") as WorkOwnerKind;
}
