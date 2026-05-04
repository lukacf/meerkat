// Generated wire types for Meerkat SDK
// Contract version: 0.6.0

export const CONTRACT_VERSION = "0.6.0";

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
  schema_warnings?: Array<{ provider: string; path: string; message: string }>;
}

export interface WireProviderMeta {
  provider: string;
  [key: string]: unknown;
}

export interface WireToolCall {
  id: string;
  name: string;
  args: unknown;
}

export interface WireToolResult {
  tool_use_id: string;
  content: WireToolResultContent;
  is_error?: boolean;
}

export interface WireSessionMessage {
  role: string;
  created_at: string;
  content?: WireContentInput;
  tool_calls?: WireToolCall[];
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
  status: string;
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
  state: WireMemberState;
  status: WireMobMemberStatus;
  wired_to?: string[];
}

export interface WireMobToolConfig {
  builtins?: boolean;
  comms?: boolean;
  mcp?: string[];
  memory?: boolean;
  mob?: boolean;
  mob_tasks?: boolean;
  schedule?: boolean;
  shell?: boolean;
}

export interface WireMobProfile {
  backend?: WireMobBackendKind;
  external_addressable?: boolean;
  max_inline_peer_notifications?: number;
  model: string;
  output_schema?: unknown;
  peer_description?: string;
  provider_params?: unknown;
  runtime_mode?: WireMobRuntimeMode;
  skills?: string[];
  tools?: WireMobToolConfig;
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
  status: string;
}

export interface MobWireResult {
  wired: boolean;
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
  status: string;
}

export interface MobFlowsResult {
  flows: string[];
  mob_id: string;
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
  run: unknown;
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
  auth_binding?: WireAuthBindingRef;
  clear_auth_binding?: boolean;
  clear_provider_params?: boolean;
  flow_tool_overlay?: Record<string, unknown>;
  keep_alive?: boolean;
  max_tokens?: number;
  mob_id: string;
  model?: string;
  output_schema?: unknown;
  prompt: WireContentInput;
  provider?: string;
  provider_params?: unknown;
  skill_refs?: Record<string, unknown>[];
  structured_output_retries?: number;
  system_prompt?: string;
}

export interface MobMemberStatusResult {
  current_session_id?: string;
  error?: string;
  external_member?: unknown;
  is_final: boolean;
  kickoff?: unknown;
  output_preview?: string;
  peer_connectivity?: unknown;
  realtime_attachment_status?: string;
  status: WireMobMemberStatus;
  tokens_used: number;
}

export interface MobSnapshotResult {
  members: unknown[];
  mob_id: string;
  status: string;
}

export interface MobDestroyResult {
  destroy_report: unknown;
  mob_id: string;
  ok: boolean;
}

export interface MobRotateSupervisorResult {
  mob_id: string;
  ok: boolean;
  report: unknown;
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
  profile?: unknown;
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

export interface MobDefinitionInput {
  backend?: MobBackendConfigInput;
  event_router?: MobEventRouterConfigInput;
  flows?: Record<string, MobFlowSpecInput>;
  id: string;
  limits?: MobLimitsSpecInput;
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
  backend?: WireMobBackendKind;
  external_addressable?: boolean;
  max_inline_peer_notifications?: number;
  model: string;
  output_schema?: unknown;
  peer_description?: string;
  provider_params?: unknown;
  runtime_mode?: WireMobRuntimeMode;
  skills?: string[];
  tools?: MobToolConfigInput;
}

export interface MobRoleWiringRuleInput {
  a: string;
  b: string;
}

export interface MobSupervisorSpecInput {
  escalation_threshold: number;
  role: string;
}

export interface MobToolConfigInput {
  builtins?: boolean;
  comms?: boolean;
  mcp?: string[];
  memory?: boolean;
  mob?: boolean;
  mob_tasks?: boolean;
  schedule?: boolean;
  shell?: boolean;
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
  error: string;
  stage: WireMobReconcileStage;
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
  last_unreachable_reason?: PeerReachabilityReason;
  meta: Record<string, unknown>;
  name: PeerName;
  peer_id: PeerId;
  reachability: PeerReachability;
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

export interface RuntimeRealtimeAttachmentStatusParams {
  session_id: string;
}

export interface RealtimeOpenRequest {
  channel_config?: RealtimeChannelConfig;
  reconnect_policy?: RealtimeReconnectPolicy;
  role: RealtimeChannelRole;
  target: RealtimeChannelTarget;
  turning_mode: RealtimeTurningMode;
}

export interface RealtimeStatusParams {
  target: RealtimeChannelTarget;
}

export interface RealtimeCapabilitiesParams {
  target: RealtimeChannelTarget;
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

export interface WireContentBlockUnknown {
  type: "unknown";
}

export type WireContentBlock = WireContentBlockText | WireContentBlockImage | WireContentBlockVideo | WireContentBlockUnknown;

export type WireContentInput = string | WireContentBlock[];

export type WireMemberRef = string;

export type WireMobBackendKind = "session" | "external";

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

export type WireMemberState = "active" | "retiring";

export type WireMobMemberStatus = "active" | "retiring" | "broken" | "completed" | "unknown";

export type WireMobRuntimeMode = "autonomous_host" | "turn_driven";

export type MobSpawnManyFailureCause = "profile_not_found" | "member_not_found" | "member_already_exists" | "not_externally_addressable" | "invalid_transition" | "wiring_error" | "bridge_command_rejected" | "member_restore_failed" | "kickoff_wait_timed_out" | "ready_wait_timed_out" | "definition_error" | "flow_not_found" | "flow_failed" | "run_not_found" | "run_canceled" | "flow_turn_timed_out" | "frame_depth_limit_exceeded" | "frame_atomic_persistence_unavailable" | "spec_revision_conflict" | "schema_validation" | "insufficient_targets" | "topology_violation" | "bridge_delivery_rejected" | "supervisor_escalation" | "unsupported_for_mode" | "reset_barrier" | "storage_error" | "session_error" | "comms_error" | "callback_pending" | "stale_fence_token" | "stale_event_cursor" | "work_not_found" | "internal";

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

export type McpLiveOperation = "add" | "remove" | "reload";

export type McpLiveOpStatus = "staged" | "applied" | "rejected";

export type McpHttpTransport = "streamable-http" | "sse";

export type MobPeerTarget = { local: string } | { external: WireTrustedPeerSpec };

export type WireHandlingMode = "queue" | "steer";

export type WireRenderClass = "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";

export type WireRenderSalience = "background" | "normal" | "important" | "urgent";

export type WireRuntimeState = "initializing" | "idle" | "attached" | "running" | "retired" | "stopped" | "destroyed";

export type WireRealtimeAttachmentStatus = "unattached" | "intent_present_unbound" | "binding_not_ready" | "binding_ready" | "replacement_pending" | "reattach_required";

export interface RealtimeChannelTargetSessionTarget {
  session_id: string;
  type: "session_target";
}

export interface RealtimeChannelTargetMobMember {
  agent_identity: string;
  mob_id: string;
  type: "mob_member";
}

export type RealtimeChannelTarget = RealtimeChannelTargetSessionTarget | RealtimeChannelTargetMobMember;

export type RealtimeChannelRole = "primary" | "observer";

export type RealtimeTurningMode = "provider_managed" | "explicit_commit";

export type RealtimeProtocolVersion = "2";

export type RealtimeInputKind = "text" | "audio" | "video";

export type RealtimeOutputKind = "text" | "audio" | "video";

export type RealtimeChannelState = "opening" | "ready" | "interrupted" | "reconnecting" | "closed" | "error";

export type RealtimeErrorCode = "invalid_frame" | "expected_channel_open" | "invalid_open_token" | "open_token_expired" | "role_mismatch" | "turning_mode_mismatch" | "unsupported_turning_mode" | "target_busy" | "unsupported_protocol_version" | "audio_format_mismatch" | "unauthorized_realm" | "tool_call_timeout" | "internal_error" | "reconnect_exhausted" | "invalid_target" | "channel_not_bound" | "runtime_internal" | "runtime_not_ready" | "provider_session_closed" | "provider_session_failed" | "provider_session_unavailable" | "unsupported_input_kind" | "no_pending_turn" | "observer_read_only" | "unexpected_channel_open" | "commit_turn_unavailable" | "channel_reconnecting" | "binding_released" | "authentication_failed" | "content_filtered" | "model_not_found" | "invalid_request";

export interface RealtimeErrorDetailsAudioFormatMismatch {
  actual: RealtimeAudioFormat;
  expected: RealtimeAudioFormat;
  kind: "audio_format_mismatch";
}

export interface RealtimeErrorDetailsToolCallTimeout {
  call_id: string;
  elapsed_ms: number;
  timeout_ms: number;
  kind: "tool_call_timeout";
}

export interface RealtimeErrorDetailsUnsupportedProtocolVersion {
  kind: "unsupported_protocol_version";
  requested: string;
  supported: RealtimeProtocolVersion[];
}

export type RealtimeErrorDetails = RealtimeErrorDetailsAudioFormatMismatch | RealtimeErrorDetailsToolCallTimeout | RealtimeErrorDetailsUnsupportedProtocolVersion;

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

export interface RealtimeOutputChunkTextDelta {
  delta: string;
  kind: "text_delta";
}

export interface RealtimeOutputChunkAudioChunk {
  channels: number;
  data: string;
  mime_type: string;
  sample_rate_hz: number;
  kind: "audio_chunk";
}

export interface RealtimeOutputChunkVideoChunk {
  data: string;
  mime_type: string;
  kind: "video_chunk";
}

export type RealtimeOutputChunk = RealtimeOutputChunkTextDelta | RealtimeOutputChunkAudioChunk | RealtimeOutputChunkVideoChunk;

export interface RealtimeEventInputTranscriptPartial {
  text: string;
  type: "input_transcript_partial";
}

export interface RealtimeEventInputTranscriptFinal {
  prosody_hint?: string;
  text: string;
  type: "input_transcript_final";
}

export interface RealtimeEventTurnStarted {
  type: "turn_started";
}

export interface RealtimeEventTurnCommitted {
  type: "turn_committed";
}

export interface RealtimeEventTurnCompleted {
  type: "turn_completed";
}

export interface RealtimeEventOutputTextDelta {
  delta: string;
  type: "output_text_delta";
}

export interface RealtimeEventOutputAudioChunk {
  chunk: RealtimeAudioChunk;
  type: "output_audio_chunk";
}

export interface RealtimeEventOutputVideoChunk {
  chunk: RealtimeVideoChunk;
  type: "output_video_chunk";
}

export interface RealtimeEventInterrupted {
  type: "interrupted";
}

export interface RealtimeEventToolCallRequested {
  call_id: string;
  tool_name: string;
  type: "tool_call_requested";
}

export interface RealtimeEventToolCallCompleted {
  call_id: string;
  type: "tool_call_completed";
}

export interface RealtimeEventToolCallFailed {
  call_id: string;
  error: string;
  type: "tool_call_failed";
}

export interface RealtimeEventToolCallTimedOut {
  call_id: string;
  elapsed_ms: number;
  type: "tool_call_timed_out";
}

export interface RealtimeEventAssistantTranscriptTruncated {
  audio_played_ms: number;
  item_id: string;
  truncated_text?: string;
  type: "assistant_transcript_truncated";
}

export interface RealtimeEventStatusChanged {
  status: RealtimeChannelStatus;
  type: "status_changed";
}

export interface RealtimeEventNeedsReattach {
  type: "needs_reattach";
}

export type RealtimeEvent = RealtimeEventInputTranscriptPartial | RealtimeEventInputTranscriptFinal | RealtimeEventTurnStarted | RealtimeEventTurnCommitted | RealtimeEventTurnCompleted | RealtimeEventOutputTextDelta | RealtimeEventOutputAudioChunk | RealtimeEventOutputVideoChunk | RealtimeEventInterrupted | RealtimeEventToolCallRequested | RealtimeEventToolCallCompleted | RealtimeEventToolCallFailed | RealtimeEventToolCallTimedOut | RealtimeEventAssistantTranscriptTruncated | RealtimeEventStatusChanged | RealtimeEventNeedsReattach;

export interface RealtimeClientFrameChannelOpen {
  open_token: string;
  protocol_version: RealtimeProtocolVersion;
  role: RealtimeChannelRole;
  turning_mode: RealtimeTurningMode;
  type: "channel.open";
}

export interface RealtimeClientFrameChannelInput {
  chunk: RealtimeInputChunk;
  type: "channel.input";
}

export interface RealtimeClientFrameChannelCommitTurn {
  type: "channel.commit_turn";
}

export interface RealtimeClientFrameChannelInterrupt {
  type: "channel.interrupt";
}

export interface RealtimeClientFrameChannelBargeInTruncate {
  audio_played_ms: number;
  content_index: number;
  item_id: string;
  type: "channel.barge_in_truncate";
}

export interface RealtimeClientFrameChannelClose {
  type: "channel.close";
}

export type RealtimeClientFrame = RealtimeClientFrameChannelOpen | RealtimeClientFrameChannelInput | RealtimeClientFrameChannelCommitTurn | RealtimeClientFrameChannelInterrupt | RealtimeClientFrameChannelBargeInTruncate | RealtimeClientFrameChannelClose;

export interface RealtimeServerFrameChannelOpened {
  capabilities: RealtimeCapabilities;
  protocol_version: RealtimeProtocolVersion;
  role: RealtimeChannelRole;
  status: RealtimeChannelStatus;
  type: "channel.opened";
}

export interface RealtimeServerFrameChannelStatus {
  status: RealtimeChannelStatus;
  type: "channel.status";
}

export interface RealtimeServerFrameChannelEvent {
  event: RealtimeEvent;
  type: "channel.event";
}

export interface RealtimeServerFrameChannelError {
  code: RealtimeErrorCode;
  details?: RealtimeErrorDetails;
  message: string;
  type: "channel.error";
}

export interface RealtimeServerFrameChannelClosed {
  reason?: string;
  type: "channel.closed";
}

export type RealtimeServerFrame = RealtimeServerFrameChannelOpened | RealtimeServerFrameChannelStatus | RealtimeServerFrameChannelEvent | RealtimeServerFrameChannelError | RealtimeServerFrameChannelClosed;

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
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired";
  params: CommsPeerLifecycleParams;
  to: PeerId;
}

export interface CommsCommandPeerRequest {
  handling_mode?: HandlingMode;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  stream?: "none" | "reserve_interaction";
  to: PeerId;
}

export interface CommsCommandPeerResponse {
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
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandUnwireMember {
  command: "unwire_member";
  epoch: number;
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

export type ContentBlock = ContentBlockText | ContentBlockImage | ContentBlockVideo;

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
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired";
  params: CommsPeerLifecycleParams;
  session_id: string;
  to: PeerId;
}

export interface CommsSendParamsPeerRequest {
  handling_mode?: HandlingMode;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  session_id: string;
  stream?: "none" | "reserve_interaction";
  to: PeerId;
}

export interface CommsSendParamsPeerResponse {
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

export type PeerReachability = "unknown" | "reachable" | "unreachable";

export type PeerReachabilityReason = "offline_or_no_ack" | "transport_error" | "admission_dropped";

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

export interface RuntimeRealtimeAttachmentStatusResult {
  status: "unattached" | "intent_present_unbound" | "binding_not_ready" | "binding_ready" | "replacement_pending" | "reattach_required";
}

export interface RealtimeReconnectPolicy {
  initial_backoff_ms: number;
  max_attempts: number;
  max_backoff_ms: number;
  max_total_ms: number;
}

export interface RealtimeToolTimeoutPolicyDefault {
  type: "default";
}

export interface RealtimeToolTimeoutPolicyDisabled {
  type: "disabled";
}

export interface RealtimeToolTimeoutPolicyFinite {
  timeout_ms: number;
  type: "finite";
}

export type RealtimeToolTimeoutPolicy = RealtimeToolTimeoutPolicyDefault | RealtimeToolTimeoutPolicyDisabled | RealtimeToolTimeoutPolicyFinite;

export interface RealtimeChannelConfig {
  tool_timeout?: RealtimeToolTimeoutPolicy;
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

export interface RealtimeChannelStatus {
  attempt_count?: number;
  deadline_at?: string;
  next_retry_at?: string;
  reason?: string;
  state: RealtimeChannelState;
}

export interface RealtimeOpenInfo {
  capabilities: RealtimeCapabilities;
  default_protocol_version: RealtimeProtocolVersion;
  expires_at: string;
  open_token: string;
  supported_protocol_versions?: RealtimeProtocolVersion[];
  target: RealtimeChannelTarget;
  ws_url: string;
}

export interface RealtimeStatusResult {
  status: RealtimeChannelStatus;
}

export interface RealtimeCapabilitiesResult {
  capabilities: RealtimeCapabilities;
}

export interface RealtimeTextChunk {
  text: string;
}

export interface RealtimeTextDelta {
  delta: string;
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

export interface RealtimeBargeInTruncateFrame {
  audio_played_ms: number;
  content_index: number;
  item_id: string;
}

export interface AudioFormatMismatchContext {
  actual: RealtimeAudioFormat;
  expected: RealtimeAudioFormat;
}

export interface ToolCallTimeoutContext {
  call_id: string;
  elapsed_ms: number;
  timeout_ms: number;
}

export interface RealtimeChannelOpenFrame {
  open_token: string;
  protocol_version: RealtimeProtocolVersion;
  role: RealtimeChannelRole;
  turning_mode: RealtimeTurningMode;
}

export interface RealtimeChannelInputFrame {
  chunk: RealtimeInputChunk;
}

export interface RealtimeChannelOpenedFrame {
  capabilities: RealtimeCapabilities;
  protocol_version: RealtimeProtocolVersion;
  role: RealtimeChannelRole;
  status: RealtimeChannelStatus;
}

export interface RealtimeChannelStatusFrame {
  status: RealtimeChannelStatus;
}

export interface RealtimeChannelEventFrame {
  event: RealtimeEvent;
}

export interface RealtimeChannelErrorFrame {
  code: RealtimeErrorCode;
  details?: RealtimeErrorDetails;
  message: string;
}

export interface RealtimeChannelClosedFrame {
  reason?: string;
}

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

export interface WireSessionInfo {
  created_at: number;
  is_active: boolean;
  labels?: Record<string, string>;
  last_assistant_text?: string;
  message_count: number;
  model: string;
  provider: string;
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
  inline_video: boolean;
  model_family: string;
  params_schema: unknown;
  supports_reasoning: boolean;
  supports_temperature: boolean;
  supports_thinking: boolean;
  supports_web_search?: boolean;
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
  state: "valid" | "expiring" | "expired" | "reauth_required" | "refresh_failed" | "unknown";
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
  state: "valid" | "expiring" | "expired" | "reauth_required" | "refresh_failed" | "unknown";
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

export type WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorStaleCredential | WireAuthErrorRefreshRequired | WireAuthErrorLeaseAbsent | WireAuthErrorUserReauthRequired | WireAuthErrorRefreshFailed | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther;

export interface WireAssistantBlockText {
  block_type: "text";
  data: Record<string, unknown>;
}

export interface WireAssistantBlockReasoning {
  block_type: "reasoning";
  data: Record<string, unknown>;
}

export interface WireAssistantBlockToolUse {
  block_type: "tool_use";
  data: Record<string, unknown>;
}

export interface WireAssistantBlockImage {
  block_type: "image";
  data: Record<string, unknown>;
}

export interface WireAssistantBlockUnknown {
  block_type: "unknown";
}

export type WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockImage | WireAssistantBlockUnknown;

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
