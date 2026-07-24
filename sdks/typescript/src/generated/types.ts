// Generated wire types for Meerkat SDK
// Contract version: 0.8.6

import { MeerkatError } from "./errors.js";

export const CONTRACT_VERSION = "0.8.6";

export type Value = unknown;


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
  if (typeof value !== "number" || !Number.isSafeInteger(value)) {
    throw wireParseError(context, "expected safe integer");
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

export interface WireToolResult {
  tool_use_id: string;
  content: WireToolResultContent;
  is_error?: boolean;
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
  transport?: McpHttpTransport | null;
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
  server_name?: string | null;
  session_id: string;
}

export interface McpLiveOpResponse {
  applied_at_turn?: number | null;
  operation: "add" | "remove" | "reload";
  persisted: boolean;
  server_name?: string | null;
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
  shadowed_by?: SkillSourceProvenance | null;
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

export interface WireLiveHotSwapSkipReasonNoOpOrOverride {
  kind: "no_op_or_override";
}

export interface WireLiveHotSwapSkipReasonIdentityLookupFailed {
  error: string;
  kind: "identity_lookup_failed";
}

export type WireLiveHotSwapSkipReason = WireLiveHotSwapSkipReasonNoOpOrOverride | WireLiveHotSwapSkipReasonIdentityLookupFailed;

export interface WireLiveChannelRefreshFailureOpenConfigBuildFailed {
  error: string;
  kind: "open_config_build_failed";
}

export interface WireLiveChannelRefreshFailureSnapshotVersionFailed {
  error: string;
  kind: "snapshot_version_failed";
}

export interface WireLiveChannelRefreshFailureEnqueueFailed {
  error: string;
  kind: "enqueue_failed";
}

export interface WireLiveChannelRefreshFailureQueueAcceptanceRejected {
  error: string;
  kind: "queue_acceptance_rejected";
}

export type WireLiveChannelRefreshFailure = WireLiveChannelRefreshFailureOpenConfigBuildFailed | WireLiveChannelRefreshFailureSnapshotVersionFailed | WireLiveChannelRefreshFailureEnqueueFailed | WireLiveChannelRefreshFailureQueueAcceptanceRejected;

export interface WireLiveChannelCloseFailureSignalFailed {
  error: string;
  kind: "signal_failed";
}

export interface WireLiveChannelCloseFailureCloseAuthorityRejected {
  error: string;
  kind: "close_authority_rejected";
}

export interface WireLiveChannelCloseFailureCommitHandoffMissing {
  kind: "commit_handoff_missing";
}

export interface WireLiveChannelCloseFailureHostCommitFailed {
  error: string;
  kind: "host_commit_failed";
}

export type WireLiveChannelCloseFailure = WireLiveChannelCloseFailureSignalFailed | WireLiveChannelCloseFailureCloseAuthorityRejected | WireLiveChannelCloseFailureCommitHandoffMissing | WireLiveChannelCloseFailureHostCommitFailed;

export interface WireLiveHotSwapSkip {
  reason: WireLiveHotSwapSkipReason;
  session_id: string;
}

export interface WireLiveSwapFailure {
  error: string;
  session_id: string;
}

export interface WireLiveRefreshFailure {
  failure: WireLiveChannelRefreshFailure;
  session_id: string;
}

export interface WireLiveCloseFailure {
  failure: WireLiveChannelCloseFailure;
  session_id: string;
}

export interface WireLiveConfigPropagationReport {
  clean: boolean;
  close_failed: WireLiveCloseFailure[];
  closed: string[];
  refresh_failed: WireLiveRefreshFailure[];
  refreshed: string[];
  skipped: WireLiveHotSwapSkip[];
  swap_failed: WireLiveSwapFailure[];
  swapped: string[];
}

export interface CallbackToolDefinition {
  description: string;
  input_schema: unknown;
  name: ToolName;
}

export interface ConfigEnvelope {
  backend?: string | null;
  config: unknown;
  generation: number;
  instance_id?: string | null;
  realm_id?: string | null;
  resolved_paths?: Record<string, unknown> | null;
}

export interface ConfigPatchParams {
  expected_generation?: number | null;
  patch?: unknown;
}

export interface ConfigWriteResult {
  backend?: string | null;
  config: unknown;
  generation: number;
  instance_id?: string | null;
  live_propagation?: WireLiveConfigPropagationReport | null;
  realm_id?: string | null;
  resolved_paths?: Record<string, unknown> | null;
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

export type ConfigSetParams = Record<string, unknown>;

export interface SessionExternalEventEnvelopeGenericJson {
  blocks?: WireContentBlock[] | null;
  event_type: string;
  kind: "generic_json";
  payload: unknown;
}

export interface SessionExternalEventEnvelopePeerResponseTerminal {
  display_name?: PeerName | null;
  kind: "peer_response_terminal";
  peer_id: PeerId;
  request_id: string;
  result: unknown;
  status: "completed" | "failed" | "cancelled";
}

export type SessionExternalEventEnvelope = SessionExternalEventEnvelopeGenericJson | SessionExternalEventEnvelopePeerResponseTerminal;

export interface WireDeviceCompleteResultPending {
  state: "pending";
}

export interface WireDeviceCompleteResultSlowDown {
  state: "slow_down";
}

export interface WireDeviceCompleteResultAccessDenied {
  state: "access_denied";
}

export interface WireDeviceCompleteResultExpired {
  state: "expired";
}

export interface WireDeviceCompleteResultReady {
  auth_binding: WireAuthBindingRef;
  binding_id: string;
  expires_at?: string | null;
  has_refresh_token: boolean;
  profile_id: string;
  provider: string;
  realm_id: string;
  scopes: string[];
  state: "ready";
}

export type WireDeviceCompleteResult = WireDeviceCompleteResultPending | WireDeviceCompleteResultSlowDown | WireDeviceCompleteResultAccessDenied | WireDeviceCompleteResultExpired | WireDeviceCompleteResultReady;

export interface ApprovalDecideParams {
  actor: string;
  approval_id: string;
  decision: "approve" | "deny";
  provenance?: unknown;
  reason?: string | null;
}

export interface ApprovalGetParams {
  approval_id: string;
}

export interface ApprovalListParams {
  filter?: Record<string, unknown>;
}

export interface ApprovalListResult {
  approvals: Record<string, unknown>[];
}

export interface ApprovalRecord {
  allowed_decisions: ("approve" | "deny")[];
  approval_id: string;
  created_at: string;
  decision?: Record<string, unknown> | null;
  expires_at?: string | null;
  metadata: Record<string, unknown>;
  owner: Record<string, unknown>;
  proposed_action: Record<string, unknown>;
  request_body?: unknown;
  request_provenance?: unknown;
  requester: string;
  resource: Record<string, unknown>;
  risk: "low" | "medium" | "high" | "critical";
  status: "pending" | "approved" | "denied" | "expired" | "cancelled";
  updated_at: string;
}

export interface ApprovalRequestParams {
  allowed_decisions: ("approve" | "deny")[];
  expires_at?: string | null;
  metadata?: Record<string, unknown>;
  owner: Record<string, unknown>;
  proposed_action: Record<string, unknown>;
  request_body?: unknown;
  request_provenance?: unknown;
  requester: string;
  resource: Record<string, unknown>;
  risk: "low" | "medium" | "high" | "critical";
}

export interface ArchiveSessionParams {
  session_id: string;
}

export interface ArtifactDownloadParams {
  artifact_id: string;
  expected_media_type?: string | null;
}

export interface ArtifactDownloadResult {
  payload: Record<string, unknown>;
  record: Record<string, unknown>;
}

export interface ArtifactIdParams {
  artifact_id: string;
}

export interface ArtifactListParams {
  label_equals?: Record<string, string>;
  session_id?: string | null;
}

export interface ArtifactListResult {
  artifacts: Record<string, unknown>[];
}

export interface ArtifactRecord {
  artifact_id: string;
  artifact_type: "text" | "log" | "command_output" | "diff" | "patch" | "generated_file" | "test_report" | "screenshot" | "image" | "json" | "binary" | { other: string };
  content_handle: BlobRef | Record<string, unknown>;
  created_at: string;
  handle: Record<string, unknown>;
  hash?: string | null;
  media_type: string;
  metadata?: Record<string, unknown>;
  owner?: Record<string, unknown>;
  producer?: string | null;
  provenance?: Record<string, string>;
  size_bytes: number;
  title: string;
}

export interface BindingIdParams {
  binding_id: string;
  profile_id?: string | null;
  realm_id: string;
}

export interface BlobGetParams {
  blob_id: string;
}

export interface BlobPayload {
  blob_id: BlobId;
  data: string;
  media_type: string;
}

export interface CreateProfileParams {
  auth_method: string;
  binding_id: string;
  profile_id?: string | null;
  realm_id: string;
  secret: string;
}

export interface CreateScheduleRequest {
  description?: string | null;
  labels?: Record<string, string>;
  misfire_policy?: Record<string, unknown>;
  missing_target_policy?: "skip" | "mark_misfired";
  name?: string | null;
  overlap_policy?: "allow_concurrent" | "skip_if_running";
  planning_horizon_days?: number | null;
  planning_horizon_occurrences?: number | null;
  target: Record<string, unknown>;
  trigger: Record<string, unknown>;
}

export interface DeferredCreateResult {
  session_id: string;
  session_ref?: string | null;
}

export interface DeviceCompleteParams {
  binding_id: string;
  device_code: string;
  profile_id?: string | null;
  provider: string;
  realm_id: string;
}

export interface DeviceStartParams {
  binding_id: string;
  profile_id?: string | null;
  provider: string;
  realm_id: string;
}

export interface EventsLatestCursorParams {
  scope: Record<string, unknown>;
}

export interface EventsLatestCursorResult {
  contract_version: ContractVersion;
  cursor: Record<string, unknown>;
}

export interface EventsListSinceParams {
  cursor?: Record<string, unknown> | null;
  limit?: number | null;
  scope: Record<string, unknown>;
}

export interface EventsListSinceResult {
  contract_version: ContractVersion;
  events?: Record<string, unknown>[];
  from_cursor: Record<string, unknown>;
  has_more: boolean;
  latest_cursor: Record<string, unknown>;
  scope: Record<string, unknown>;
}

export interface EventsSnapshotParams {
  scope: Record<string, unknown>;
}

export interface EventsSnapshotResult {
  contract_version: ContractVersion;
  cursor: Record<string, unknown>;
  scope: Record<string, unknown>;
  snapshot: Record<string, unknown>;
}

export interface ForkSessionAtParams {
  message_index: number;
  running_behavior?: "reject";
  session_id: string;
  tool_access_policy?: WireToolAccessPolicy | null;
}

export interface ForkSessionReplaceParams {
  message_index: number;
  replacement: WireTranscriptReplacement;
  running_behavior?: "reject";
  session_id: string;
  tool_access_policy?: WireToolAccessPolicy | null;
}

export interface HelpRequest {
  execution_mode?: "explain_only" | "plan_execution";
  max_tokens?: number | null;
  model?: string | null;
  prompt?: string | null;
  provider?: string | null;
  question: string;
}

export interface HelpResponse {
  extraction_error?: Record<string, unknown> | null;
  schema_warnings?: Record<string, unknown>[] | null;
  session_id: string;
  session_ref?: string | null;
  skill_diagnostics?: Record<string, unknown> | null;
  structured_output?: unknown;
  terminal_cause_kind?: "unknown" | "hook_denied" | "hook_failure" | "llm_failure" | "tool_failure" | "structured_output_validation_failed" | "budget_exhausted" | "time_budget_exceeded" | "retry_exhausted" | "turn_limit_reached" | "runtime_apply_failure" | "fatal_failure" | null;
  text: string;
  tool_calls: number;
  turns: number;
  usage: Record<string, unknown>;
}

export interface InjectSystemContextParams {
  content: Record<string, unknown>;
  idempotency_key?: string | null;
  session_id: string;
  source?: string | null;
}

export interface InjectSystemContextResult {
  status: "applied" | "staged" | "duplicate";
}

export interface InterruptParams {
  session_id: string;
}

export interface ListSessionTranscriptRevisionsParams {
  limit?: number | null;
  offset?: number | null;
  session_id: string;
}

export interface ListSessionsParams {
  labels?: Record<string, string> | null;
  limit?: number | null;
  offset?: number | null;
}

export interface ListSessionsResult {
  sessions: Record<string, unknown>[];
}

export interface LoginCompleteParams {
  binding_id: string;
  code: string;
  profile_id?: string | null;
  provider: string;
  realm_id: string;
  redirect_uri: string;
  state: string;
}

export interface LoginStartParams {
  binding_id: string;
  profile_id?: string | null;
  provider: string;
  realm_id: string;
  redirect_uri: string;
}

export interface ProvisionApiKeyParams {
  access_token: string;
  binding_id?: string | null;
  profile_id?: string | null;
  realm_id?: string | null;
}

export interface ReadSessionHistoryParams {
  limit?: number | null;
  offset?: number | null;
  session_id: string;
}

export interface ReadSessionParams {
  session_id: string;
}

export interface ReadSessionTranscriptRevisionParams {
  limit?: number | null;
  offset?: number | null;
  revision: string;
  session_id: string;
}

export interface SessionInputStateParams {
  selector: Record<string, unknown>;
  session_id: string;
}

export interface SessionInputStateSelector {
}

export interface SessionInputStateResult {
  state?: Record<string, unknown> | null;
}

export interface RealmIdParams {
  realm_id: string;
}

export interface RestoreSessionTranscriptRevisionParams {
  actor?: string | null;
  expected_parent_revision?: string | null;
  reason: TranscriptRewriteReason;
  revision: string;
  running_behavior?: "reject";
  session_id: string;
}

export interface RewriteSessionTranscriptParams {
  actor?: string | null;
  expected_parent_revision?: string | null;
  reason: TranscriptRewriteReason;
  replacement: TranscriptRewriteMessage[];
  running_behavior?: "reject";
  selection: TranscriptRewriteSelection;
  session_id: string;
}

export interface RuntimeHostCapabilities {
  contract_version: ContractVersion;
  features: RuntimeHostFeatureFlags;
}

export interface RuntimeHostFeatureFlags {
  approvals: boolean;
  artifacts: boolean;
  blobs: boolean;
  comms: boolean;
  event_replay: boolean;
  external_members: boolean;
  mcp_live: boolean;
  mobs: boolean;
  multi_host_mobs?: boolean;
  runtime_backed_sessions: boolean;
  schedules: boolean;
  secure_remote_rpc: boolean;
  session_events: boolean;
  session_streams: boolean;
  skills: boolean;
}

export interface RuntimeHostHealth {
  checks?: Record<string, "ok" | "degraded" | "unhealthy">;
  contract_version: ContractVersion;
  status: "ok" | "degraded" | "unhealthy";
}

export interface RuntimeHostInfo {
  capabilities: RuntimeHostCapabilities;
  contract_version: ContractVersion;
  endpoints: Record<string, unknown>;
  health: RuntimeHostHealth;
  host_id: string;
  host_id_scope: "process" | "realm_instance";
  placement_labels?: Record<string, string>;
  policy_profile_summary?: string | null;
  process_name: string;
  process_version: string;
  realm: Record<string, unknown>;
}

export interface Schedule {
  created_at_utc: string;
  deleted_at_utc?: string | null;
  description?: string | null;
  labels?: Record<string, string>;
  misfire_policy: Record<string, unknown>;
  missing_target_policy: "skip" | "mark_misfired";
  name?: string | null;
  next_occurrence_ordinal: number;
  overlap_policy: "allow_concurrent" | "skip_if_running";
  phase: "active" | "paused" | "deleted";
  planning_cursor_utc?: string | null;
  planning_horizon_days: number;
  planning_horizon_occurrences: number;
  revision: number;
  schedule_id: string;
  superseded_ack_ids?: string[];
  target: Record<string, unknown>;
  trigger: Record<string, unknown>;
  updated_at_utc: string;
}

export interface ScheduleToolCallParams {
  arguments?: unknown;
  name: string;
}

export interface ScheduleToolsResult {
  tools: unknown[];
}

export interface SessionForkResult {
  message_count: number;
  session_id: string;
  session_ref?: string | null;
  source_session_id: string;
}

export interface SessionPeerResponseTerminalParams {
  display_name?: PeerName | null;
  peer_id: PeerId;
  request_id: string;
  result: unknown;
  session_id: string;
  status: "completed" | "failed" | "cancelled";
}

export interface SessionTranscriptRewriteResult {
  commit: Record<string, unknown>;
  message_count: number;
  parent_revision: string;
  revision: string;
  session_id: string;
}

export interface WireProvisionApiKeyResult {
  auth_binding: WireAuthBindingRef;
  auth_mode: string;
  binding_id: string;
  has_api_key: boolean;
  profile_id: string;
  provider: string;
  realm_id: string;
  scopes: string[];
}

export interface WireSessionTranscriptRevision {
  has_more: boolean;
  head_revision: string;
  limit?: number | null;
  message_count: number;
  messages: Record<string, unknown>[];
  offset: number;
  revision: string;
  session_id: string;
  session_ref?: string | null;
}

export interface WireSessionTranscriptRevisionList {
  entries: Record<string, unknown>[];
  head_revision: string;
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
  additional_instructions?: string[] | null;
  agent_identity: string;
  auth_binding?: WireAuthBindingRef | null;
  auto_wire_parent?: boolean | null;
  backend?: WireMobBackendKind | null;
  binding?: WireRuntimeBinding | null;
  context?: unknown;
  inherited_tool_filter?: WireToolFilter | null;
  initial_message?: WireContentInput | null;
  labels?: Record<string, string> | null;
  launch_mode?: WireMemberLaunchMode | null;
  mob_id: string;
  model_override?: string | null;
  override_profile?: WireMobProfile | null;
  placement?: string | null;
  profile: string;
  runtime_mode?: WireMobRuntimeMode | null;
  shell_env?: Record<string, string> | null;
  tool_access_policy?: WireToolAccessPolicy | null;
}

export interface MobSpawnResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobSpawnSpecParams {
  additional_instructions?: string[] | null;
  agent_identity: string;
  auth_binding?: WireAuthBindingRef | null;
  backend?: WireMobBackendKind | null;
  context?: unknown;
  initial_message?: WireContentInput | null;
  labels?: Record<string, string> | null;
  model_override?: string | null;
  placement?: WireHostRef | null;
  profile: string;
  runtime_mode?: WireMobRuntimeMode | null;
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
  error?: string | null;
  is_final: boolean;
  labels?: Record<string, string>;
  member_ref: WireMemberRef;
  role: string;
  runtime_mode: WireMobRuntimeMode;
  status: WireMobMemberStatus;
  wired_to?: string[];
}

export interface WireMemberProgressSnapshot {
  health: "healthy" | "degraded" | "wedged" | "unknown";
  in_flight_work: number;
  last_progress_at_ms: number;
  last_progress_event: "execution_advanced" | "became_idle" | "unchanged";
  run_state: "idle" | "run_open" | "unknown";
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
  auto_compact_threshold?: number | null;
  backend?: WireMobBackendKind | null;
  external_addressable?: boolean;
  image_generation_provider?: Provider | null;
  max_inline_peer_notifications?: number | null;
  model: string;
  output_schema?: unknown;
  peer_description?: string;
  provider?: Provider | null;
  provider_params?: unknown | null;
  resume_overrides?: WireMobResumeOverrideField[];
  runtime_mode?: WireMobRuntimeMode;
  self_hosted_server_id?: string | null;
  skills?: string[];
  tools?: WireMobToolConfig;
}

export interface WireMobRun {
  flow_id: string;
  mob_id: string;
  run_id: string;
  status: WireMobRunStatus;
}

export interface WireMobRunResultEnvelope {
  flow_id: string;
  mob_id: string;
  outputs?: Record<string, unknown>;
  result?: unknown;
  run_id: string;
  status: WireMobRunStatus;
}

export interface WirePeerConnectivitySnapshot {
  reachable_peer_count: number;
  unknown_peer_count: number;
  unreachable_peers?: WireUnreachablePeer[];
}

export interface WireUnreachablePeer {
  peer: string;
  reason?: string | null;
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
  initial_message?: WireContentInput | null;
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
  render_metadata?: Record<string, unknown> | null;
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
  render_metadata?: Record<string, unknown> | null;
  spec: MobMemberSpecWire;
}

export interface MobIngressInteractionResult {
  agent_identity: string;
  delivery: MobMemberSendResult;
  ensure_outcome: { spawned: MobSpawnReceiptWire } | { existed: MobMemberListEntryWire };
  events_after_cursor: number;
  latest_event_cursor: number;
  member_ref: WireMemberRef;
  mob_id: string;
}

export interface MobAppendSystemContextParams {
  agent_identity: string;
  idempotency_key?: string | null;
  mob_id: string;
  source?: string | null;
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
  flow_id?: string | null;
  mob_id: string;
  params?: unknown;
  prompt?: string | null;
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
  run?: WireMobRun | null;
}

export interface MobRunResultParams {
  mob_id: string;
  run_id: string;
}

export interface MobRunResult {
  run?: WireMobRunResultEnvelope | null;
}

export interface MobFlowCancelParams {
  mob_id: string;
  run_id: string;
}

export interface MobFlowCancelResult {
  canceled: boolean;
}

export interface MobSpawnHelperParams {
  agent_identity?: string | null;
  auth_binding?: WireAuthBindingRef | null;
  backend?: WireMobBackendKind | null;
  mob_id: string;
  model_override?: string | null;
  prompt: string;
  role_name?: string | null;
  runtime_mode?: WireMobRuntimeMode | null;
}

export interface MobForkHelperParams {
  agent_identity?: string | null;
  auth_binding?: WireAuthBindingRef | null;
  backend?: WireMobBackendKind | null;
  fork_context?: unknown;
  mob_id: string;
  model_override?: string | null;
  prompt: string;
  role_name?: string | null;
  runtime_mode?: WireMobRuntimeMode | null;
  source_member_id: string;
}

export interface MobHelperResult {
  agent_identity: string;
  member_ref: WireMemberRef;
  output?: string | null;
  tokens_used: number;
}

export interface MobForceCancelResult {
  cancelled: boolean;
}

export interface MobTurnStartParams {
  additional_instructions?: string[] | null;
  agent_identity: string;
  auth_binding?: Record<string, unknown> | null;
  injected_context?: WireContentInput[] | null;
  keep_alive?: boolean | null;
  max_tokens?: number | null;
  mob_id: string;
  model?: string | null;
  output_schema?: unknown;
  prompt: WireContentInput;
  provider?: string | null;
  provider_params?: Record<string, unknown> | null;
  self_hosted_server_id?: string | null;
  skill_refs?: SkillKey[] | null;
  structured_output_retries?: number | null;
  system_prompt?: string | null;
  turn_tool_overlay?: PublicTurnToolOverlay | null;
}

export interface MobMemberStatusResult {
  comms_reachability?: WireReachability | null;
  control_reachability?: WireReachability | null;
  current_session_id?: string | null;
  error?: string | null;
  external_member?: unknown;
  freshness_reason?: string | null;
  is_final: boolean;
  kickoff?: unknown;
  last_seen_ms?: number | null;
  lifecycle_capabilities?: WireMemberLifecycleCapabilities | null;
  member_ref: WireMemberRef;
  non_portable_disabled?: WireNonPortableResourceKind[] | null;
  output_preview?: string | null;
  peer_connectivity?: WirePeerConnectivity | null;
  placement?: WireHostRef | null;
  progress?: WireMemberProgressSnapshot | null;
  resolved_capabilities?: WireResolvedModelCapabilities | null;
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
  injected_context?: WireContentInput[] | null;
  member_ref: WireMemberRef;
  objective_id?: string | null;
  origin?: "external" | "internal";
  work_ref?: string | null;
}

export interface MobSubmitWorkResult {
  member_ref: WireMemberRef;
  mob_id: string;
  objective_id?: string | null;
  work_ref: string;
}

export interface MobConcludeObjectiveParams {
  member_ref: WireMemberRef;
  objective_id: string;
  outcome: string;
}

export interface MobConcludeObjectiveResult {
  concluded: boolean;
  member_ref: WireMemberRef;
  objective_id: string;
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
  member_ids?: string[] | null;
  mob_id: string;
  timeout_ms?: number | null;
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
  created_at?: string | null;
  name: string;
  not_found?: boolean;
  profile?: WireMobProfile | null;
  revision?: number | null;
  updated_at?: string | null;
}

export interface MobProfileListResult {
  profiles: MobProfileLookupResult[];
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
  agent_identity?: string | null;
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

export interface MobGrantScopesParams {
  expires_at_ms?: number | null;
  mob_id: string;
  principal: string;
  scopes: WireControlScope[];
}

export interface MobGrantScopesResult {
  record: WireGrantRecord;
}

export interface MobRevokeScopesParams {
  mob_id: string;
  principal: string;
  scopes?: WireControlScope[] | null;
}

export interface MobRevokeScopesResult {
  removed: boolean;
}

export interface MobGrantsResult {
  grants: WireGrantRecord[];
}

export interface MobMemberHistoryParams {
  agent_identity: string;
  from_index?: number | null;
  limit?: number | null;
  mob_id: string;
}

export interface MobMemberHistoryResult {
  generation: number;
  page: WireMemberHistoryPageBody;
  placement?: WireHostRef | null;
  provenance: WireProjectionProvenance;
}

export interface WireMemberHistoryPageBody {
  complete: boolean;
  from_index: number;
  message_count: number;
  messages: WireHistoryRow[];
  next_index?: number | null;
}

export interface MobHostsResult {
  hosts: MobHostStatus[];
}

export interface MobHostStatus {
  authority_epoch?: number | null;
  bind_phase: WireHostBindPhase;
  capabilities?: WireHostCapabilityFlags | null;
  control_reachability?: WireReachability | null;
  endpoint?: string | null;
  freshness_reason?: string | null;
  host_id: WireHostRef;
  last_seen_ms?: number | null;
  materialized_member_count: number;
}

export interface WireHostCapabilityFlags {
  approval_forwarding: boolean;
  autonomous_members: boolean;
  durable_sessions: boolean;
  engine_version: string;
  hard_cancel_member: boolean;
  live_endpoint?: string | null;
  mcp: boolean;
  memory_store: boolean;
  protocol_max: number;
  protocol_min: number;
  resolvable_providers?: string[];
  tracked_input_cancel?: boolean;
}

export interface MobRouteInstallsResult {
  complete: boolean;
  outstanding: WireRouteInstallObligation[];
}

export interface WireRouteInstallObligation {
  edge_a: string;
  edge_b: string;
  host: WireHostRef;
}

export interface MobBindHostParams {
  descriptor: WireHostBindingDescriptor;
  mob_id: string;
}

export interface MobBindHostResult {
  authority_epoch: number;
  capabilities: WireHostCapabilityFlags;
  host_id: WireHostRef;
}

export interface MobRevokeHostParams {
  host_id: WireHostRef;
  mob_id: string;
}

export interface MobRevokeHostResult {
  host_id: WireHostRef;
  released_members: string[];
}

export interface MobHardCancelParams {
  agent_identity: string;
  mob_id: string;
  reason: string;
}

export interface MobHardCancelResult {
  cancelled: boolean;
}

export interface MobMemberLiveOpenParams {
  agent_identity: string;
  mob_id: string;
  transport?: LiveOpenTransport | null;
  turning_mode?: RealtimeTurningMode | null;
}

export interface MobMemberLiveChannelParams {
  agent_identity: string;
  channel_id: string;
  mob_id: string;
}

export interface MobMemberLiveStatusParams {
  agent_identity: string;
  channel_id?: string | null;
  mob_id: string;
}

export interface MobMemberLiveControlParams {
  agent_identity: string;
  channel_id: string;
  mob_id: string;
  verb: BridgeLiveControlVerb;
}

export interface PublicTurnToolOverlay {
  allowed_tools?: ToolName[] | null;
  blocked_tools?: ToolName[] | null;
}

export interface MobDefinitionInput {
  backend?: MobBackendConfigInput;
  event_router?: MobEventRouterConfigInput | null;
  flows?: Record<string, MobFlowSpecInput>;
  id: string;
  image_generation_provider?: Provider | null;
  limits?: MobLimitsSpecInput | null;
  models?: Record<string, CustomModelConfig>;
  orchestrator?: MobOrchestratorInput | null;
  profiles: Record<string, MobProfileBindingInput>;
  skills?: Record<string, MobSkillSourceInput>;
  spawn_policy?: MobSpawnPolicyInput | null;
  supervisor?: MobSupervisorSpecInput | null;
  topology?: MobTopologySpecInput | null;
  wiring?: MobWiringRulesInput;
}

export interface MobBackendConfigInput {
  default?: WireMobBackendKind;
  external?: MobExternalBackendConfigInput | null;
}

export interface MobEventRouterConfigInput {
  buffer_size?: number;
  exclude_patterns?: string[] | null;
  include_patterns?: string[] | null;
}

export interface MobExternalBackendConfigInput {
  address_base: string;
  supervisor_bridge?: unknown | null;
}

export interface MobFlowSpecInput {
  description?: string | null;
  root?: MobFrameSpecInput | null;
  steps?: Record<string, MobFlowStepInput>;
}

export interface MobFlowStepInput {
  allowed_tools?: string[] | null;
  blocked_tools?: string[] | null;
  branch?: string | null;
  collection_policy?: MobCollectionPolicyInput;
  condition?: MobConditionExprInput | null;
  depends_on?: string[];
  depends_on_mode?: MobDependencyModeInput;
  dispatch_mode?: MobDispatchModeInput;
  expected_schema_ref?: string | null;
  message: WireContentInput;
  output_format?: MobStepOutputFormatInput | null;
  role: string;
  timeout_ms?: number | null;
}

export interface MobFrameSpecInput {
  nodes: Record<string, MobFlowNodeInput>;
}

export interface MobLimitsSpecInput {
  cancel_grace_timeout_ms?: number | null;
  max_active_frames?: number | null;
  max_active_nodes?: number | null;
  max_flow_duration_ms?: number | null;
  max_frame_depth?: number | null;
  max_orphaned_turns?: number | null;
  max_step_retries?: number | null;
}

export interface MobOrchestratorInput {
  profile: string;
}

export interface MobProfileInput {
  auto_compact_threshold?: number | null;
  backend?: WireMobBackendKind | null;
  external_addressable?: boolean;
  image_generation_provider?: Provider | null;
  max_inline_peer_notifications?: number | null;
  model: string;
  output_schema?: unknown | null;
  peer_description?: string;
  provider?: Provider | null;
  provider_params?: unknown | null;
  resume_overrides?: WireMobResumeOverrideField[];
  runtime_mode?: WireMobRuntimeMode;
  self_hosted_server_id?: string | null;
  skills?: string[];
  tools?: MobToolConfigInput;
}

export interface MobRoleWiringRuleInput {
  a: string;
  b: string;
}

export interface MobSupervisorSpecInput {
  escalation_threshold: number;
  escalation_turn_timeout_ms?: number | null;
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
  call_timeout_secs?: number | null;
  context_window?: number | null;
  display_name?: string | null;
  max_output_tokens?: number | null;
  provider: Provider;
  vision?: boolean | null;
  web_search?: boolean | null;
}

export interface MobMemberSpecWire {
  additional_instructions?: string[] | null;
  agent_identity: string;
  auto_wire_parent?: boolean | null;
  backend?: WireMobBackendKind | null;
  binding?: WireRuntimeBinding | null;
  context?: unknown;
  initial_message?: WireContentInput | null;
  labels?: Record<string, string> | null;
  placement?: WireHostRef | null;
  profile: string;
  runtime_mode?: WireMobRuntimeMode | null;
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

export interface WireHostBindingDescriptor {
  address: string;
  bootstrap_token: BridgeBootstrapToken;
  identity: WireTrustedPeerIdentity;
  kind: WireHostBindingDescriptorKind;
  live_endpoint?: string | null;
}

export interface WireGrantRecord {
  expires_at_ms?: number | null;
  principal: string;
  scopes: WireControlScope[];
}

export interface WireMemberLifecycleCapabilities {
  resume_after_restart: boolean;
  revisions: boolean;
  transcript_edits: boolean;
}

export interface AttentionBindingRequest {
  binding_id: string;
  namespace?: string | null;
  realm_id?: string | null;
}

export interface AttentionBindingResult {
  attention: WorkAttentionBinding;
}

export interface AttentionListRequest {
  namespace?: string | null;
  realm_id?: string | null;
  status?: WorkAttentionStatus | null;
  target?: WorkAttentionTarget | null;
}

export interface AttentionListResult {
  attention: WorkAttentionBinding[];
}

export interface AttentionProjectionPolicy {
  include_parent_context?: boolean;
  max_text_chars?: number;
}

export interface GoalStatusRequest {
  binding_id: string;
  namespace?: string | null;
  realm_id?: string | null;
}

export interface GoalStatusResult {
  attention: WorkAttentionBinding;
  item: WorkItem;
}

export interface ReadyWorkFilter {
  labels?: string[];
  limit?: number | null;
  namespace?: string | null;
  realm_id?: string | null;
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
  after_seq?: number | null;
  all_namespaces?: boolean;
  limit?: number | null;
  namespace?: string | null;
  realm_id?: string | null;
}

export interface WorkGraphEventsResponse {
  events: WorkGraphEvent[];
}

export interface WorkGraphIdParams {
  id: string;
  namespace?: string | null;
  realm_id?: string | null;
}

export interface WorkGraphItemsResponse {
  items: WorkItem[];
}

export interface WorkGraphSnapshot {
  all_namespaces: boolean;
  attention?: WorkAttentionBinding[];
  captured_at: string;
  edges: WorkEdge[];
  event_high_water_mark?: number | null;
  items: WorkItem[];
  namespace?: string | null;
  ready_item_ids: string[];
  realm_id: string;
}

export interface WorkGraphSnapshotFilter {
  all_namespaces?: boolean;
  include_terminal?: boolean;
  labels?: string[];
  limit?: number | null;
  namespace?: string | null;
  realm_id?: string | null;
  statuses?: WorkStatus[];
}

export interface WorkItem {
  claim?: WorkItemClaim | null;
  completion_policy: WorkCompletionPolicy;
  created_at: string;
  description?: string | null;
  due_at?: string | null;
  evidence_refs?: WorkEvidenceRef[];
  external_refs?: WorkItemExternalRef[];
  id: string;
  labels?: string[];
  machine_state: Record<string, unknown>;
  namespace: string;
  not_before?: string | null;
  owner?: WorkItemOwner | null;
  priority: "low" | "medium" | "high";
  realm_id: string;
  revision: number;
  snoozed_until?: string | null;
  status: "open" | "in_progress" | "blocked" | "completed" | "cancelled" | "failed";
  terminal_at?: string | null;
  title: string;
  updated_at: string;
}

export interface WorkItemFilter {
  all_namespaces?: boolean;
  include_terminal?: boolean;
  labels?: string[];
  limit?: number | null;
  namespace?: string | null;
  realm_id?: string | null;
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
  confirmation_kind?: WorkEvidenceKind | null;
  confirming_owner_key?: WorkOwnerKey | null;
  id: string;
  kind: string;
  label?: string | null;
  summary?: string | null;
}

export interface WorkGraphEvent {
  at: string;
  item_id?: string | null;
  kind: WorkGraphEventKind;
  namespace: string;
  payload?: unknown;
  realm_id: string;
  seq?: number | null;
}

export interface WorkOwnerKey {
  id: string;
  kind: WorkOwnerKind;
}

export interface WorkItemClaim {
  claimed_at: string;
  lease_expires_at?: string | null;
  owner: WorkItemOwner;
}

export interface WorkItemExternalRef {
  id: string;
  kind: string;
  url?: string | null;
}

export interface WorkItemOwner {
  display_name?: string | null;
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
  approval_forwarding?: boolean;
  autonomous_members?: boolean;
  current_protocol_version?: BridgeProtocolVersion;
  default_protocol_version?: BridgeProtocolVersion;
  deliver_member_input?: boolean;
  destroy_member?: boolean;
  durable_sessions?: boolean;
  engine_version?: string;
  hard_cancel_member?: boolean;
  interrupt_member?: boolean;
  mcp?: boolean;
  memory_store?: boolean;
  observe_member?: boolean;
  resolvable_providers?: Provider[];
  retire_member?: boolean;
  supported_protocol_versions?: BridgeProtocolVersion[];
  tracked_input_cancel?: boolean;
  unwire_member?: boolean;
  wire_member?: boolean;
}

export interface BridgeDeliveryPayload {
  content: ContentInput;
  epoch: number;
  expected_member?: BridgeMemberIncarnation | null;
  handling_mode: HandlingMode;
  injected_context?: ContentInput[];
  input_id: string;
  objective_id?: string | null;
  outcome_tracking?: "interaction" | null;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  transcript_interaction_id?: string | null;
  turn?: BridgeTurnDirective | null;
}

export interface BridgeDeliveryResponse {
  canonical_input_id?: string | null;
  input_id: string;
  outcome: BridgeDeliveryOutcome;
}

export interface BridgeDestroyResponse {
  inputs_abandoned: number;
}

export interface BridgeHardCancelPayload {
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  expected_run_id: RunId;
  operation_id: OperationId;
  protocol_version: BridgeProtocolVersion;
  reason: string;
  supervisor: BridgePeerSpec;
}

export interface BridgeObservationResponse {
  accepting_inputs?: boolean | null;
  current_run_id?: string | null;
  last_error?: string | null;
  observed_at: string;
  peer_connectivity?: BridgePeerConnectivity | null;
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
  mob_peer_overlay?: BridgeMobPeerOverlayHandoff | null;
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
  description?: string | null;
  peer: string;
  peer_spec?: BridgePeerSpec | null;
  role?: string | null;
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

export interface BridgeHostCapabilityRequirements {
  autonomous_members: boolean;
  durable_sessions: boolean;
  protocol_v4: boolean;
  tracked_input_cancel: boolean;
}

export interface BridgeMemberIncarnation {
  agent_identity: string;
  binding_generation: number;
  fence_token: number;
  generation: number;
  host_id: string;
  member_session_id: string;
  mob_id: string;
}

export interface BridgeTurnCorrelation {
  run_id: string;
  step_id: string;
}

export interface BridgeTurnDirective {
  correlation: BridgeTurnCorrelation;
  tool_overlay?: PublicTurnToolOverlay | null;
}

export interface BridgeTurnOutcomeAck {
  fence_token: number;
  generation: number;
  input_id: string;
}

export interface WireFlowFailureDetail {
  original_utf8_bytes: number;
  text: string;
  truncated: boolean;
}

export interface BridgeTurnOutcomeRecord {
  fence_token: number;
  generation: number;
  input_id: string;
  outcome: WireFlowTurnOutcome;
  terminal_seq: number;
}

export interface BridgeMobPeerOverlayHandoff {
  peer_specs: BridgePeerSpec[];
  recipient_peer_id: string;
  topology_epoch: number;
}

export type RunId = string;

export type OperationId = string;

export interface BridgeEventCursorTail {
  cursor: "tail";
}

export interface BridgeEventCursorAt {
  cursor: "at";
  generation: number;
  seq: number;
}

export type BridgeEventCursor = BridgeEventCursorTail | BridgeEventCursorAt;

export type BridgeOutboundTaintTarget = "peer_only" | { placed: BridgeMemberIncarnation };

export interface BridgeTrackedInputCancelOutcomeNoEffect {
  outcome: "no_effect";
}

export interface BridgeTrackedInputCancelOutcomeCancelled {
  outcome: "cancelled";
}

export interface BridgeTrackedInputCancelOutcomeTerminal {
  outcome: "terminal";
  record: BridgeTurnOutcomeRecord;
}

export type BridgeTrackedInputCancelOutcome = BridgeTrackedInputCancelOutcomeNoEffect | BridgeTrackedInputCancelOutcomeCancelled | BridgeTrackedInputCancelOutcomeTerminal;

export type LiveOpenTransport = "websocket" | "webrtc";

export interface WireFlowTurnOutcomeExtractionFailedPayload {
  detail: WireFlowFailureDetail;
}

export interface WireFlowTurnOutcomeExtractionFailed {
  extraction_failed: WireFlowTurnOutcomeExtractionFailedPayload;
}

export interface WireFlowTurnOutcomeRunFailedPayload {
  detail: WireFlowFailureDetail;
}

export interface WireFlowTurnOutcomeRunFailed {
  run_failed: WireFlowTurnOutcomeRunFailedPayload;
}

export interface WireFlowTurnOutcomeInteractionFailedPayload {
  detail: WireFlowFailureDetail;
}

export interface WireFlowTurnOutcomeInteractionFailed {
  interaction_failed: WireFlowTurnOutcomeInteractionFailedPayload;
}

export type WireFlowTurnOutcome = "run_completed" | "extraction_succeeded" | "interaction_complete" | "interaction_callback_pending" | "channel_closed" | WireFlowTurnOutcomeExtractionFailed | WireFlowTurnOutcomeRunFailed | WireFlowTurnOutcomeInteractionFailed;

export type AuthErrorKind = "missing_secret" | "unsupported_combination" | "missing_required_metadata" | "workspace_mismatch" | "stale_credential" | "refresh_required" | "lease_absent" | "user_reauth_required" | "expired" | "refresh_failed" | "resolve_required" | "interactive_login_required" | "host_owned_unavailable" | "io" | "other";

export type ConnectionTargetErrorKind = "missing_realm" | "unknown_realm" | "missing_default_binding" | "invalid_realm_id" | "invalid_binding_id" | "realm_config_invalid" | "binding_invalid" | "provider_mismatch" | "realm_chain";

export type WireControlScope = "list" | "read_history" | "subscribe_events" | "send_command" | "cancel" | "retire" | "wire_topology" | "live" | "admin_host" | "admin_grants";

export interface MemberBuildRejectionUnknownProviderForModelPayload {
  model: string;
}

export interface MemberBuildRejectionUnknownProviderForModel {
  unknown_provider_for_model: MemberBuildRejectionUnknownProviderForModelPayload;
}

export interface MemberBuildRejectionBindingUnresolvablePayload {
  kind: ConnectionTargetErrorKind;
}

export interface MemberBuildRejectionBindingUnresolvable {
  binding_unresolvable: MemberBuildRejectionBindingUnresolvablePayload;
}

export interface MemberBuildRejectionProviderAuthPayload {
  kind: AuthErrorKind;
}

export interface MemberBuildRejectionProviderAuth {
  provider_auth: MemberBuildRejectionProviderAuthPayload;
}

export interface MemberBuildRejectionSelfHostedServerMissingPayload {
  server_id: string;
}

export interface MemberBuildRejectionSelfHostedServerMissing {
  self_hosted_server_missing: MemberBuildRejectionSelfHostedServerMissingPayload;
}

export type MemberBuildRejection = MemberBuildRejectionUnknownProviderForModel | MemberBuildRejectionBindingUnresolvable | MemberBuildRejectionProviderAuth | MemberBuildRejectionSelfHostedServerMissing;

export type ToolConfigChangeDomain = "tool_scope" | "deferred_catalog";

export type ToolConfigChangeOperation = "add" | "remove" | "reload";

export type ExternalToolDeltaPhase = "pending" | "applied" | "draining" | "forced" | "failed";

export interface ToolConfigChangeStatusBoundaryApplied {
  base_changed: boolean;
  kind: "boundary_applied";
  revision: number;
  visible_changed: boolean;
}

export interface ToolConfigChangeStatusDeferredCatalogDelta {
  added_hidden_count: number;
  kind: "deferred_catalog_delta";
  pending_source_count: number;
  removed_hidden_count: number;
}

export interface ToolConfigChangeStatusWarningFailedClosed {
  error: string;
  kind: "warning_failed_closed";
}

export interface ToolConfigChangeStatusExternalToolDelta {
  detail?: string | null;
  kind: "external_tool_delta";
  phase: ExternalToolDeltaPhase;
}

export type ToolConfigChangeStatus = ToolConfigChangeStatusBoundaryApplied | ToolConfigChangeStatusDeferredCatalogDelta | ToolConfigChangeStatusWarningFailedClosed | ToolConfigChangeStatusExternalToolDelta;

export interface SystemNoticePeer {
  display_name?: string | null;
  id: PeerId;
}

export interface DeferredCatalogDelta {
  added_hidden_names?: ToolName[];
  pending_sources?: string[];
  removed_hidden_names?: ToolName[];
}

export interface ToolConfigChangedPayload {
  applied_at_turn?: number | null;
  deferred_catalog_delta?: DeferredCatalogDelta | null;
  domain?: ToolConfigChangeDomain | null;
  operation: ToolConfigChangeOperation;
  persisted: boolean;
  status_info: ToolConfigChangeStatus;
  target: string;
}

export interface ScheduleIdParams {
  schedule_id: string;
}

export interface ListSchedulesParams {
  labels?: Record<string, string> | null;
  limit?: number | null;
  offset?: number | null;
}

export interface ScheduleOccurrencesParams {
  include_terminal?: boolean | null;
  schedule_id: string;
}

export interface UpdateScheduleParams {
  description?: string | null;
  expected_revision?: number | null;
  labels?: Record<string, string> | null;
  misfire_policy?: Record<string, unknown> | null;
  missing_target_policy?: "skip" | "mark_misfired" | null;
  name?: string | null;
  overlap_policy?: "allow_concurrent" | "skip_if_running" | null;
  planning_horizon_days?: number | null;
  planning_horizon_occurrences?: number | null;
  schedule_id: string;
  target?: Record<string, unknown> | null;
  trigger?: Record<string, unknown> | null;
}

export interface WireContentBlockText {
  text: string;
  type: "text";
}

export interface WireContentBlockImageInline {
  media_type: string;
  type: "image";
  data: string;
  source: "inline";
}

export interface WireContentBlockImageBlob {
  media_type: string;
  type: "image";
  blob_id: string;
  source: "blob";
}

export interface WireContentBlockVideoInline {
  duration_ms: number;
  media_type: string;
  type: "video";
  data: string;
  source: "inline";
}

export interface WireContentBlockVideoUri {
  duration_ms: number;
  media_type: string;
  type: "video";
  source: "uri";
  uri: string;
}

export interface WireContentBlockStructured {
  data: unknown;
  type: "structured";
}

export interface WireContentBlockUnknown {
  type: "unknown";
}

export type WireContentBlock = WireContentBlockText | WireContentBlockImageInline | WireContentBlockImageBlob | WireContentBlockVideoInline | WireContentBlockVideoUri | WireContentBlockStructured | WireContentBlockUnknown;

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
  bootstrap_token?: BridgeBootstrapToken | null;
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
  branch?: string | null;
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

export type WireHostRef = string;

export type WireHostBindPhase = "requested" | "bound";

export type WireProjectionProvenance = "host_claimed" | "controlling_host_verified";

export type WireReachability = "reachable" | "stale" | "unreachable" | "unknown";

export type WireNonPortableResourceKind = "rust_bundles" | "per_spawn_external_tools" | "mob_default_external_tools" | "default_llm_client_override" | "host_surface_mcp_allowlist" | "workgraph_tools";

export type WireMobRunStatus = "pending" | "running" | "completed" | "failed" | "canceled";

export interface WirePeerConnectivityNotApplicable {
  status: "not_applicable";
}

export interface WirePeerConnectivityProbeTimedOut {
  status: "probe_timed_out";
}

export interface WirePeerConnectivityKnown {
  snapshot: WirePeerConnectivitySnapshot;
  status: "known";
}

export type WirePeerConnectivity = WirePeerConnectivityNotApplicable | WirePeerConnectivityProbeTimedOut | WirePeerConnectivityKnown;

export type WireHostBindingDescriptorKind = "host";

export interface BridgeLiveControlVerbCommitInput {
  verb: "commit_input";
}

export interface BridgeLiveControlVerbInterrupt {
  verb: "interrupt";
}

export interface BridgeLiveControlVerbTruncate {
  audio_played_ms: number;
  content_index: number;
  item_id: string;
  verb: "truncate";
}

export interface BridgeLiveControlVerbRefresh {
  verb: "refresh";
}

export type BridgeLiveControlVerb = BridgeLiveControlVerbCommitInput | BridgeLiveControlVerbInterrupt | BridgeLiveControlVerbTruncate | BridgeLiveControlVerbRefresh;

export interface BridgeLiveControlOutcomeCommitInput {
  status: "committed";
  verb: "commit_input";
}

export interface BridgeLiveControlOutcomeInterrupt {
  status: "interrupted";
  verb: "interrupt";
}

export interface BridgeLiveControlOutcomeTruncate {
  status: "truncated";
  verb: "truncate";
}

export interface BridgeLiveControlOutcomeRefresh {
  status: "queued";
  verb: "refresh";
}

export type BridgeLiveControlOutcome = BridgeLiveControlOutcomeCommitInput | BridgeLiveControlOutcomeInterrupt | BridgeLiveControlOutcomeTruncate | BridgeLiveControlOutcomeRefresh;

export type AttentionDelegatedAuthority = "add_evidence" | "close_own_review_item" | "request_closure" | "close_if_policy_allows";

export interface GoalAttentionTargetSession {
  kind: "session";
  session_id: string;
}

export interface GoalAttentionTargetOwner {
  kind: "owner";
  owner_key: WorkOwnerKey;
}

export type GoalAttentionTarget = GoalAttentionTargetSession | GoalAttentionTargetOwner;

export type WorkAttentionMode = "pursue" | "coordinate" | "review" | "falsify" | "judge" | "observe";

export interface WorkAttentionStatusActive {
  state: "active";
}

export interface WorkAttentionStatusPaused {
  state: "paused";
  until?: string | null;
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

export type RealtimeInputKind = "text" | "audio" | "video" | "image";

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

export interface RealtimeInputChunkImageChunk {
  data: string;
  idempotency_key: string;
  mime_type: string;
  kind: "image_chunk";
}

export type RealtimeInputChunk = RealtimeInputChunkTextChunk | RealtimeInputChunkAudioChunk | RealtimeInputChunkVideoChunk | RealtimeInputChunkImageChunk;

export type RuntimeAcceptOutcomeType = "accepted" | "deduplicated" | "rejected";

export type WireInputLifecycleState = "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";

export type WireStopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

export type WireToolResultContent = string | WireContentBlock[];

export type WireModelTier = "recommended" | "supported";

export interface CommsCommandInput {
  allow_self_session?: boolean | null;
  blocks?: ContentBlock[] | null;
  body: string;
  handling_mode?: HandlingMode | null;
  kind: "input";
  source?: "tcp" | "uds" | "stdin" | "webhook" | "rpc" | null;
  stream?: "none" | "reserve_interaction" | null;
}

export interface CommsCommandPeerMessage {
  blocks?: ContentBlock[] | null;
  body: string;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
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
  blocks?: ContentBlock[] | null;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  stream?: "none" | "reserve_interaction" | null;
  to: PeerId;
}

export interface CommsCommandPeerResponse {
  blocks?: ContentBlock[] | null;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
  in_reply_to: string;
  kind: "peer_response";
  result?: CommsPeerResponseResult | null;
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
  expected_member?: BridgeMemberIncarnation | null;
  handling_mode: HandlingMode;
  injected_context?: ContentInput[];
  input_id: string;
  objective_id?: string | null;
  outcome_tracking?: "interaction" | null;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  transcript_interaction_id?: string | null;
  turn?: BridgeTurnDirective | null;
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
  expected_member?: BridgeMemberIncarnation | null;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandHardCancelMember {
  command: "hard_cancel_member";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  expected_run_id: RunId;
  operation_id: OperationId;
  protocol_version: BridgeProtocolVersion;
  reason: string;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandCancelTrackedMemberInput {
  command: "cancel_tracked_member_input";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  input_id: string;
  protocol_version: BridgeProtocolVersion;
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
  mob_peer_overlay?: BridgeMobPeerOverlayHandoff | null;
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandUnwireMember {
  command: "unwire_member";
  epoch: number;
  mob_peer_overlay?: BridgeMobPeerOverlayHandoff | null;
  peer_spec: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandDeclareMemberOutboundTaint {
  command: "declare_member_outbound_taint";
  epoch: number;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  taint?: SenderContentTaint | null;
  target?: BridgeOutboundTaintTarget | null;
}

export interface BridgeCommandReadMemberHistory {
  command: "read_member_history";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  from_index?: number | null;
  limit?: number | null;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandPollMemberEvents {
  command: "poll_member_events";
  cursor: BridgeEventCursor;
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  max?: number | null;
  max_outcomes?: number | null;
  outcome_acks?: BridgeTurnOutcomeAck[];
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  wait_ms?: number | null;
}

export interface BridgeCommandOpenMemberLiveChannel {
  command: "open_member_live_channel";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  transport?: LiveOpenTransport | null;
  turning_mode?: RealtimeTurningMode | null;
}

export interface BridgeCommandCloseMemberLiveChannel {
  channel_id: string;
  command: "close_member_live_channel";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandMemberLiveChannelStatus {
  channel_id?: string | null;
  command: "member_live_channel_status";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandControlMemberLiveChannel {
  channel_id: string;
  command: "control_member_live_channel";
  epoch: number;
  expected_member: BridgeMemberIncarnation;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
  verb: BridgeLiveControlVerb;
}

export interface BridgeCommandBindHost {
  binding_generation: number;
  bootstrap_proof: string;
  command: "bind_host";
  epoch: number;
  expected_address: string;
  expected_host_peer_id: string;
  mob_id: string;
  protocol_version: BridgeProtocolVersion;
  required_capabilities: BridgeHostCapabilityRequirements;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandRebindHost {
  binding_generation: number;
  command: "rebind_host";
  epoch: number;
  mob_id: string;
  protocol_version: BridgeProtocolVersion;
  required_capabilities: BridgeHostCapabilityRequirements;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandRevokeHost {
  binding_generation: number;
  command: "revoke_host";
  epoch: number;
  mob_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandMaterializeMember {
  binding_generation: number;
  command: "materialize_member";
  epoch: number;
  fence_token: number;
  generation: number;
  launch: { mode: "fresh" } | Record<string, unknown>;
  protocol_version: BridgeProtocolVersion;
  spec: Record<string, unknown>;
  spec_digest: string;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandReleaseMember {
  agent_identity: string;
  binding_generation: number;
  command: "release_member";
  epoch: number;
  fence_token: number;
  generation: number;
  mob_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandInstallPeerTrust {
  agent_identity: string;
  binding_generation: number;
  command: "install_peer_trust";
  epoch: number;
  mob_id: string;
  peer: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandRemovePeerTrust {
  agent_identity: string;
  binding_generation: number;
  command: "remove_peer_trust";
  epoch: number;
  mob_id: string;
  peer: BridgePeerSpec;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandHostStatus {
  binding_generation: number;
  command: "host_status";
  epoch: number;
  mob_id: string;
  protocol_version: BridgeProtocolVersion;
  supervisor: BridgePeerSpec;
}

export interface BridgeCommandMemberOperatorRequest {
  agent_identity: string;
  command: "member_operator_request";
  op: Record<string, unknown> | { op: "list_members" } | { op: "mob_list_flows" };
  protocol_version: BridgeProtocolVersion;
  request_id: string;
  requester_fence_token: number;
  requester_generation: number;
  requester_host_binding_generation: number;
  requester_host_id: string;
  requester_member_session_id: string;
}

export interface BridgeCommandObserveSupervisorRotation {
  command: "observe_supervisor_rotation";
  observer: BridgePeerSpec;
  observer_epoch: number;
  operation_id: string;
  protocol_version: BridgeProtocolVersion;
}

export type BridgeCommand = BridgeCommandBindMember | BridgeCommandAuthorizeSupervisor | BridgeCommandRevokeSupervisor | BridgeCommandDeliverMemberInput | BridgeCommandObserveMember | BridgeCommandInterruptMember | BridgeCommandHardCancelMember | BridgeCommandCancelTrackedMemberInput | BridgeCommandRetireMember | BridgeCommandDestroyMember | BridgeCommandWireMember | BridgeCommandUnwireMember | BridgeCommandDeclareMemberOutboundTaint | BridgeCommandReadMemberHistory | BridgeCommandPollMemberEvents | BridgeCommandOpenMemberLiveChannel | BridgeCommandCloseMemberLiveChannel | BridgeCommandMemberLiveChannelStatus | BridgeCommandControlMemberLiveChannel | BridgeCommandBindHost | BridgeCommandRebindHost | BridgeCommandRevokeHost | BridgeCommandMaterializeMember | BridgeCommandReleaseMember | BridgeCommandInstallPeerTrust | BridgeCommandRemovePeerTrust | BridgeCommandHostStatus | BridgeCommandMemberOperatorRequest | BridgeCommandObserveSupervisorRotation;

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

export interface BridgeDeliveryRejectionCauseTurnDirectiveUnsupported {
  detail: string;
  kind: "turn_directive_unsupported";
}

export interface BridgeDeliveryRejectionCauseOutcomeJournalFull {
  kind: "outcome_journal_full";
  limit: number;
  retained: number;
}

export interface BridgeDeliveryRejectionCauseStaleMemberIncarnation {
  current: BridgeMemberIncarnation;
  kind: "stale_member_incarnation";
}

export interface BridgeDeliveryRejectionCauseStaleMemberResidency {
  current?: BridgeMemberIncarnation | null;
  expected: BridgeMemberIncarnation;
  kind: "stale_member_residency";
}

export type BridgeDeliveryRejectionCause = BridgeDeliveryRejectionCauseNotReady | BridgeDeliveryRejectionCauseDurabilityViolation | BridgeDeliveryRejectionCausePeerHandlingModeInvalid | BridgeDeliveryRejectionCauseInternal | BridgeDeliveryRejectionCauseTurnDirectiveUnsupported | BridgeDeliveryRejectionCauseOutcomeJournalFull | BridgeDeliveryRejectionCauseStaleMemberIncarnation | BridgeDeliveryRejectionCauseStaleMemberResidency;

export type BridgeMemberRuntimeState = "initializing" | "idle" | "attached" | "running" | "retired" | "stopped" | "destroyed";

export type BridgePeerConnectivity = "reachable" | "unreachable" | "unknown";

export type BridgeProtocolVersion = number;

export interface BridgeRejectionCauseStaleCursorPayload {
  generation: number;
  watermark: number;
}

export interface BridgeRejectionCauseStaleCursor {
  stale_cursor: BridgeRejectionCauseStaleCursorPayload;
}

export interface BridgeRejectionCauseOversizedEventPayload {
  durable_seq: number;
  encoded_bytes: number;
  generation: number;
  max_bytes: number;
  next_seq: number;
}

export interface BridgeRejectionCauseOversizedEvent {
  oversized_event: BridgeRejectionCauseOversizedEventPayload;
}

export interface BridgeRejectionCauseHistoryRowTooLargePayload {
  encoded_bytes: number;
  index: number;
  max_bytes: number;
}

export interface BridgeRejectionCauseHistoryRowTooLarge {
  history_row_too_large: BridgeRejectionCauseHistoryRowTooLargePayload;
}

export interface BridgeRejectionCauseScopeDeniedPayload {
  presented: WireControlScope[];
  required: WireControlScope;
}

export interface BridgeRejectionCauseScopeDenied {
  scope_denied: BridgeRejectionCauseScopeDeniedPayload;
}

export interface BridgeRejectionCauseMaterializeBuildRejectedPayload {
  cause: MemberBuildRejection;
}

export interface BridgeRejectionCauseMaterializeBuildRejected {
  materialize_build_rejected: BridgeRejectionCauseMaterializeBuildRejectedPayload;
}

export interface BridgeRejectionCauseModelUnresolvablePayload {
  model: string;
}

export interface BridgeRejectionCauseModelUnresolvable {
  model_unresolvable: BridgeRejectionCauseModelUnresolvablePayload;
}

export interface BridgeRejectionCauseAuthBindingUnresolvablePayload {
  binding: string;
  realm: string;
}

export interface BridgeRejectionCauseAuthBindingUnresolvable {
  auth_binding_unresolvable: BridgeRejectionCauseAuthBindingUnresolvablePayload;
}

export interface BridgeRejectionCauseMcpCommandMissingPayload {
  server: string;
}

export interface BridgeRejectionCauseMcpCommandMissing {
  mcp_command_missing: BridgeRejectionCauseMcpCommandMissingPayload;
}

export interface BridgeRejectionCauseEnvKeyMissingPayload {
  key: string;
}

export interface BridgeRejectionCauseEnvKeyMissing {
  env_key_missing: BridgeRejectionCauseEnvKeyMissingPayload;
}

export interface BridgeRejectionCauseHostEngineVersionChangedPayload {
  bound: string;
  reported: string;
}

export interface BridgeRejectionCauseHostEngineVersionChanged {
  host_engine_version_changed: BridgeRejectionCauseHostEngineVersionChangedPayload;
}

export interface BridgeRejectionCauseModelNotRealtimePayload {
  model: string;
  provider: string;
}

export interface BridgeRejectionCauseModelNotRealtime {
  model_not_realtime: BridgeRejectionCauseModelNotRealtimePayload;
}

export interface BridgeRejectionCauseLiveAdapterUnavailablePayload {
  provider: string;
}

export interface BridgeRejectionCauseLiveAdapterUnavailable {
  live_adapter_unavailable: BridgeRejectionCauseLiveAdapterUnavailablePayload;
}

export interface BridgeRejectionCauseLiveTransportUnsupportedPayload {
  requested: string;
}

export interface BridgeRejectionCauseLiveTransportUnsupported {
  live_transport_unsupported: BridgeRejectionCauseLiveTransportUnsupportedPayload;
}

export interface BridgeRejectionCauseCapabilityMissingPayload {
  capability: string;
}

export interface BridgeRejectionCauseCapabilityMissing {
  capability_missing: BridgeRejectionCauseCapabilityMissingPayload;
}

export interface BridgeRejectionCauseSessionOwnershipConflictPayload {
  session_id: string;
}

export interface BridgeRejectionCauseSessionOwnershipConflict {
  session_ownership_conflict: BridgeRejectionCauseSessionOwnershipConflictPayload;
}

export type BridgeRejectionCause = "not_bound" | "stale_supervisor" | "sender_mismatch" | "already_bound" | "invalid_bootstrap_token" | "unsupported_protocol_version" | "invalid_supervisor_spec" | "invalid_peer_spec" | "address_mismatch" | "unsupported" | "internal" | "stale_fence" | BridgeRejectionCauseStaleCursor | BridgeRejectionCauseOversizedEvent | BridgeRejectionCauseHistoryRowTooLarge | "unavailable" | BridgeRejectionCauseScopeDenied | "spec_digest_mismatch" | BridgeRejectionCauseMaterializeBuildRejected | BridgeRejectionCauseModelUnresolvable | BridgeRejectionCauseAuthBindingUnresolvable | BridgeRejectionCauseMcpCommandMissing | "realm_backend_unavailable" | BridgeRejectionCauseEnvKeyMissing | BridgeRejectionCauseHostEngineVersionChanged | BridgeRejectionCauseModelNotRealtime | BridgeRejectionCauseLiveAdapterUnavailable | "live_transport_unavailable" | "live_channel_already_bound" | "live_channel_not_found" | BridgeRejectionCauseLiveTransportUnsupported | "resume_session_not_found" | BridgeRejectionCauseCapabilityMissing | "launch_mode_unsupported" | "launch_mode_placement_mismatch" | BridgeRejectionCauseSessionOwnershipConflict;

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
  accepting_inputs?: boolean | null;
  current_run_id?: string | null;
  last_error?: string | null;
  observed_at: string;
  peer_connectivity?: BridgePeerConnectivity | null;
  result: "observation";
  state: BridgeMemberRuntimeState;
}

export interface BridgeReplyDelivery {
  canonical_input_id?: string | null;
  input_id: string;
  outcome: BridgeDeliveryOutcome;
  result: "delivery";
}

export interface BridgeReplyTrackedInputCancelled {
  expected_member: BridgeMemberIncarnation;
  input_id: string;
  outcome: BridgeTrackedInputCancelOutcome;
  result: "tracked_input_cancelled";
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

export interface BridgeReplySupervisorRotationFound {
  result: "supervisor_rotation";
  outcome: "found";
  state: Record<string, unknown>;
}

export interface BridgeReplySupervisorRotationNotFound {
  result: "supervisor_rotation";
  operation_id: string;
  outcome: "not_found";
}

export interface BridgeReplyRejected {
  cause: BridgeRejectionCause;
  reason: string;
  result: "rejected";
}

export interface BridgeReplyBindHost {
  address: string;
  binding_generation: number;
  capabilities: BridgeCapabilities;
  host_peer_id: string;
  live_endpoint?: string | null;
  result: "bind_host";
}

export interface BridgeReplyHostRebound {
  binding_generation: number;
  capabilities: BridgeCapabilities;
  host_peer_id: string;
  live_endpoint?: string | null;
  result: "host_rebound";
}

export interface BridgeReplyHostRevoked {
  binding_generation: number;
  epoch: number;
  host_peer_id: string;
  mob_id: string;
  released_members: string[];
  result: "host_revoked";
}

export interface BridgeReplyMemberHistoryPage {
  generation: number;
  page: WireMemberHistoryPageBody;
  result: "member_history_page";
}

export interface BridgeReplyMemberEventsPage {
  events: Record<string, unknown>[];
  fence_token: number;
  from_seq: number;
  generation: number;
  next_seq: number;
  outcomes_complete: boolean;
  result: "member_events_page";
  runtime_incarnation: string;
  turn_outcomes?: BridgeTurnOutcomeRecord[];
  watermark: number;
}

export interface BridgeReplyMemberMaterialized {
  advertised_address: string;
  engine_version: string;
  launch_outcome: "fresh" | "resumed_live" | "resumed_from_snapshot";
  member_peer_id: string;
  member_pubkey: string;
  resolved_auth_binding?: WireAuthBindingRef | null;
  result: "member_materialized";
  session_id: string;
  spec_digest: string;
}

export interface BridgeReplyMemberReleased {
  disposal: Record<string, unknown>;
  result: "member_released";
}

export interface BridgeReplyHostStatus {
  capabilities: BridgeCapabilities;
  members: Record<string, unknown>[];
  result: "host_status";
  runtime_incarnation: string;
}

export interface BridgeReplyMemberLiveChannelOpened {
  open: Record<string, unknown>;
  result: "member_live_channel_opened";
}

export interface BridgeReplyMemberLiveChannelClosed {
  result: "member_live_channel_closed";
  status: "closed";
}

export interface BridgeReplyMemberLiveChannelStatusReport {
  channel_id: string;
  result: "member_live_channel_status_report";
  status: WireLiveAdapterStatus;
}

export interface BridgeReplyMemberLiveChannelControlled {
  outcome: BridgeLiveControlOutcome;
  result: "member_live_channel_controlled";
}

export interface BridgeReplyMemberOperatorReply {
  outcome: Record<string, unknown>;
  request_id: string;
  result: "member_operator_reply";
}

export type BridgeReply = BridgeReplyBindMember | BridgeReplyAck | BridgeReplyObservation | BridgeReplyDelivery | BridgeReplyTrackedInputCancelled | BridgeReplyRetire | BridgeReplyDestroy | BridgeReplySupervisorRotationFound | BridgeReplySupervisorRotationNotFound | BridgeReplyRejected | BridgeReplyBindHost | BridgeReplyHostRebound | BridgeReplyHostRevoked | BridgeReplyMemberHistoryPage | BridgeReplyMemberEventsPage | BridgeReplyMemberMaterialized | BridgeReplyMemberReleased | BridgeReplyHostStatus | BridgeReplyMemberLiveChannelOpened | BridgeReplyMemberLiveChannelClosed | BridgeReplyMemberLiveChannelStatusReport | BridgeReplyMemberLiveChannelControlled | BridgeReplyMemberOperatorReply;

export interface ContentBlockText {
  text: string;
  type: "text";
}

export interface ContentBlockImageInline {
  media_type: string;
  type: "image";
  data: string;
  source: "inline";
}

export interface ContentBlockImageBlob {
  media_type: string;
  type: "image";
  blob_id: BlobId;
  source: "blob";
}

export interface ContentBlockVideoInline {
  duration_ms: number;
  media_type: string;
  type: "video";
  data: string;
  source: "inline";
}

export interface ContentBlockVideoUri {
  duration_ms: number;
  media_type: string;
  type: "video";
  source: "uri";
  uri: string;
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

export type ContentBlock = ContentBlockText | ContentBlockImageInline | ContentBlockImageBlob | ContentBlockVideoInline | ContentBlockVideoUri | ContentBlockStructured | ContentBlockSkillContext;

export type ContentInput = string | ContentBlock[];

export interface CommsSendParamsInput {
  allow_self_session?: boolean | null;
  blocks?: ContentBlock[] | null;
  body: string;
  handling_mode?: HandlingMode | null;
  kind: "input";
  session_id: string;
  source?: "tcp" | "uds" | "stdin" | "webhook" | "rpc" | null;
  stream?: "none" | "reserve_interaction" | null;
}

export interface CommsSendParamsPeerMessage {
  blocks?: ContentBlock[] | null;
  body: string;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
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
  blocks?: ContentBlock[] | null;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
  intent: CommsPeerRequestIntent;
  kind: "peer_request";
  params: CommsPeerRequestParams;
  session_id: string;
  stream?: "none" | "reserve_interaction" | null;
  to: PeerId;
}

export interface CommsSendParamsPeerResponse {
  blocks?: ContentBlock[] | null;
  content_taint?: SendTaintOverride | null;
  handling_mode?: HandlingMode | null;
  in_reply_to: string;
  kind: "peer_response";
  result?: CommsPeerResponseResult | null;
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
  delivery: "acked" | "handed_off" | "queued";
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

export type SenderContentTaint = "clean" | "tainted";

export type SendTaintOverride = { declare: SenderContentTaint } | "undeclared";

export interface WireRenderMetadata {
  class: "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";
  salience?: "background" | "normal" | "important" | "urgent" | null;
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
  audio_input_format?: RealtimeAudioFormat | null;
  audio_output_format?: RealtimeAudioFormat | null;
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

export interface RealtimeImageChunk {
  data: string;
  idempotency_key: string;
  mime_type: string;
}

export interface LiveOpenParams {
  seed_max_chars?: number | null;
  session_id: string;
  transport?: LiveOpenTransport | null;
  turning_mode?: RealtimeTurningMode | null;
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
  http_url?: string | null;
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

export interface LiveSendInputErrorData {
  error_code: WireLiveAdapterErrorCode;
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
  response_modality?: WireLiveResponseModality | null;
}

export type LiveRefreshStatus = "queued";

export interface LiveRefreshResult {
  status: "queued";
}

export type LiveCloseStatus = "closed";

export interface LiveCloseResult {
  status: "closed";
}

export type LiveSendInputStatus = "sent";

export interface LiveSendInputResult {
  status: "sent";
}

export type LiveCommitInputStatus = "committed";

export interface LiveCommitInputResult {
  status: "committed";
}

export type LiveInterruptStatus = "interrupted";

export interface LiveInterruptResult {
  status: "interrupted";
}

export type LiveTruncateStatus = "truncated";

export interface LiveTruncateResult {
  status: "truncated";
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
  idempotency_key: string;
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
  previous_item_id?: string | null;
  response_id?: string | null;
  role: RealtimeTranscriptRole;
  type: "item_observed";
}

export interface RealtimeTranscriptEventItemSkipped {
  item_id: string;
  previous_item_id?: string | null;
  type: "item_skipped";
}

export interface RealtimeTranscriptEventUserTranscriptFinal {
  content_index: number;
  item_id: string;
  previous_item_id?: string | null;
  text: string;
  type: "user_transcript_final";
}

export interface RealtimeTranscriptEventAssistantTextDelta {
  content_index: number;
  delta: string;
  delta_id: string;
  item_id: string;
  previous_item_id?: string | null;
  response_id: string;
  type: "assistant_text_delta";
}

export interface RealtimeTranscriptEventAssistantTranscriptDelta {
  content_index: number;
  delta: string;
  delta_id: string;
  item_id: string;
  previous_item_id?: string | null;
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
  stop_reason: "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";
  type: "assistant_turn_completed";
  usage: Record<string, unknown>;
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
  auth_binding_changed?: boolean;
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

export interface WireLiveConfigRejectionReasonImageInputUnsupportedMime {
  kind: "image_input_unsupported_mime";
  mime_type: string;
}

export interface WireLiveConfigRejectionReasonImageInputContentMismatch {
  kind: "image_input_content_mismatch";
  mime_type: string;
}

export interface WireLiveConfigRejectionReasonImageInputInvalidBase64 {
  kind: "image_input_invalid_base64";
}

export interface WireLiveConfigRejectionReasonImageInputTooLarge {
  actual_bytes: number;
  kind: "image_input_too_large";
  max_bytes: number;
}

export interface WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid {
  actual_bytes: number;
  kind: "image_input_idempotency_key_invalid";
  max_bytes: number;
}

export interface WireLiveConfigRejectionReasonImageInputIdempotencyConflict {
  kind: "image_input_idempotency_conflict";
}

export interface WireLiveConfigRejectionReasonImageInputHistoryBudgetExceeded {
  kind: "image_input_history_budget_exceeded";
  max_decoded_bytes: number;
}

export interface WireLiveConfigRejectionReasonImageInputRequiresCommit {
  kind: "image_input_requires_commit";
}

export interface WireLiveConfigRejectionReasonInputTooLarge {
  actual_bytes: number;
  kind: "input_too_large";
  max_bytes: number;
}

export interface WireLiveConfigRejectionReasonInputBackpressured {
  kind: "input_backpressured";
  max_pending_bytes: number;
}

export interface WireLiveConfigRejectionReasonImageInputBackpressured {
  kind: "image_input_backpressured";
  max_pending_bytes: number;
}

export interface WireLiveConfigRejectionReasonImageInputTransportUnsupported {
  kind: "image_input_transport_unsupported";
  transport: string;
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

export interface WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen {
  kind: "refresh_transcript_rewrite_requires_reopen";
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

export type WireLiveConfigRejectionReason = WireLiveConfigRejectionReasonChannelIdentitySwap | WireLiveConfigRejectionReasonNonRealtimeResolution | WireLiveConfigRejectionReasonImageInputNotImplemented | WireLiveConfigRejectionReasonImageInputUnsupportedMime | WireLiveConfigRejectionReasonImageInputContentMismatch | WireLiveConfigRejectionReasonImageInputInvalidBase64 | WireLiveConfigRejectionReasonImageInputTooLarge | WireLiveConfigRejectionReasonImageInputIdempotencyKeyInvalid | WireLiveConfigRejectionReasonImageInputIdempotencyConflict | WireLiveConfigRejectionReasonImageInputHistoryBudgetExceeded | WireLiveConfigRejectionReasonImageInputRequiresCommit | WireLiveConfigRejectionReasonInputTooLarge | WireLiveConfigRejectionReasonInputBackpressured | WireLiveConfigRejectionReasonImageInputBackpressured | WireLiveConfigRejectionReasonImageInputTransportUnsupported | WireLiveConfigRejectionReasonVideoFrameInputNotImplemented | WireLiveConfigRejectionReasonUnsupportedInputChunkVariant | WireLiveConfigRejectionReasonRefreshModelSwap | WireLiveConfigRejectionReasonRefreshProviderSwap | WireLiveConfigRejectionReasonRefreshAudioConfigMismatch | WireLiveConfigRejectionReasonRefreshTranscriptRewriteRequiresReopen | WireLiveConfigRejectionReasonAudioInputFormatMismatch | WireLiveConfigRejectionReasonOther | WireLiveConfigRejectionReasonUnknown;

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
  content_index?: number | null;
  observation: "user_transcript_final";
  previous_item_id?: string | null;
  provider_item_id?: string | null;
  text: string;
}

export interface WireLiveAdapterObservationAssistantTextDelta {
  content_index?: number | null;
  delta: string;
  delta_id?: string | null;
  observation: "assistant_text_delta";
  previous_item_id?: string | null;
  provider_item_id?: string | null;
  response_id?: string | null;
}

export interface WireLiveAdapterObservationAssistantTranscriptDelta {
  content_index?: number | null;
  delta: string;
  delta_id?: string | null;
  observation: "assistant_transcript_delta";
  previous_item_id?: string | null;
  provider_item_id?: string | null;
  response_id?: string | null;
}

export interface WireLiveAdapterObservationAssistantAudioChunk {
  channels: number;
  content_index?: number | null;
  data: string;
  item_id?: string | null;
  observation: "assistant_audio_chunk";
  response_id?: string | null;
  sample_rate_hz: number;
}

export interface WireLiveAdapterObservationAssistantTranscriptFinal {
  content_index?: number | null;
  observation: "assistant_transcript_final";
  previous_item_id?: string | null;
  provider_item_id: string;
  response_id?: string | null;
  stop_reason: WireStopReason;
  text: string;
  usage: Record<string, unknown>;
}

export interface WireLiveAdapterObservationAssistantTranscriptTruncated {
  content_index?: number | null;
  observation: "assistant_transcript_truncated";
  previous_item_id?: string | null;
  provider_item_id?: string | null;
  response_id?: string | null;
  text?: string | null;
}

export interface WireLiveAdapterObservationRealtimeTranscript {
  event: RealtimeTranscriptEvent;
  observation: "realtime_transcript";
}

export interface WireLiveAdapterObservationUserContentCommitted {
  content_index: number;
  idempotency_key: string;
  item_id: string;
  media_type: string;
  observation: "user_content_committed";
  previous_item_id?: string | null;
}

export interface WireLiveAdapterObservationToolCallRequested {
  arguments: unknown;
  observation: "tool_call_requested";
  provider_call_id: string;
  tool_name: string;
}

export interface WireLiveAdapterObservationTurnInterrupted {
  observation: "turn_interrupted";
  response_id?: string | null;
}

export interface WireLiveAdapterObservationTurnCompleted {
  observation: "turn_completed";
  response_id?: string | null;
  stop_reason: WireStopReason;
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

export type WireLiveAdapterObservation = WireLiveAdapterObservationReady | WireLiveAdapterObservationUserTranscriptFinal | WireLiveAdapterObservationAssistantTextDelta | WireLiveAdapterObservationAssistantTranscriptDelta | WireLiveAdapterObservationAssistantAudioChunk | WireLiveAdapterObservationAssistantTranscriptFinal | WireLiveAdapterObservationAssistantTranscriptTruncated | WireLiveAdapterObservationRealtimeTranscript | WireLiveAdapterObservationUserContentCommitted | WireLiveAdapterObservationToolCallRequested | WireLiveAdapterObservationTurnInterrupted | WireLiveAdapterObservationTurnCompleted | WireLiveAdapterObservationStatusChanged | WireLiveAdapterObservationError | WireLiveAdapterObservationCommandRejected | WireLiveAdapterObservationUnknown;

export interface RuntimeAcceptResult {
  existing_id?: string | null;
  input_id?: string | null;
  outcome_type: "accepted" | "deduplicated" | "rejected";
  policy?: "stage" | "queue" | "immediate" | null;
  reason?: string | null;
  state?: Record<string, unknown> | null;
}

export interface WireInputStateHistoryEntry {
  from: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
  reason?: string | null;
  timestamp: string;
  to: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
}

export interface WireInputState {
  attempt_count?: number;
  created_at: string;
  current_state: "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";
  durability?: "durable" | "volatile" | "ephemeral" | null;
  history?: WireInputStateHistoryEntry[];
  idempotency_key?: string | null;
  input_id: string;
  last_boundary_sequence?: number | null;
  last_run_id?: string | null;
  persisted_input?: Record<string, unknown> | null;
  policy?: "stage" | "queue" | "immediate" | null;
  reconstruction_source?: "live" | "event_store" | "snapshot" | "replay" | null;
  recovery_count?: number;
  terminal_outcome?: "completed" | "abandoned" | "superseded" | "coalesced" | "cancelled" | null;
  updated_at: string;
}

export interface ScheduleListResult {
  schedules: Schedule[];
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
  last_assistant_text?: string | null;
  message_count: number;
  model: string;
  provider: string;
  resolved_capabilities?: WireResolvedModelCapabilities | null;
  session_id: string;
  session_ref?: string | null;
  updated_at: number;
}

export interface WireSessionSummary {
  created_at: number;
  is_active: boolean;
  labels?: Record<string, string>;
  message_count: number;
  session_id: string;
  session_ref?: string | null;
  total_tokens: number;
  updated_at: number;
}

export interface ContractVersion {
  major: number;
  minor: number;
  patch: number;
}

export interface CatalogModelEntry {
  context_window?: number | null;
  display_name: string;
  id: string;
  max_output_tokens?: number | null;
  profile?: Record<string, unknown> | null;
  server_id?: string | null;
  tier: "recommended" | "supported";
}

export interface ProviderCatalog {
  default_model_id: string;
  models: CatalogModelEntry[];
  provider: string;
}

export interface ModelsCatalogResponse {
  contract_version: ContractVersion;
  providers: ProviderCatalog[];
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
  blob_ref: BlobRef;
  height: number;
  image_id: AssistantImageId;
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
  native_metadata: ProviderImageMetadata;
  operation_id: string;
  provider_text: Record<string, unknown>;
  revised_prompt: RevisedPromptDisposition;
  terminal: Record<string, unknown>;
  warnings?: Record<string, unknown>[];
}

export interface WireAuthBindingRef {
  binding: string;
  profile?: string | null;
  realm: string;
}

export interface WireBackendProfile {
  backend_kind: string;
  base_url?: string | null;
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
  default_model?: string | null;
  id: string;
  require_metadata_account?: boolean;
  require_metadata_workspace?: boolean;
}

export interface WireRealmConnectionSet {
  auth_profiles: Record<string, WireAuthProfile>;
  backends: Record<string, WireBackendProfile>;
  bindings: Record<string, WireProviderBinding>;
  default_binding?: string | null;
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
  auth_profile: WireAuthProfile;
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
  expires_at?: string | null;
  has_refresh_token: boolean;
  profile_id: string;
  provider: string;
  realm_id: string;
  scopes: string[];
  state?: string | null;
}

export interface WireDeviceStart {
  device_code: string;
  expires_in: number;
  interval: number;
  provider: string;
  user_code: string;
  verification_uri: string;
  verification_uri_complete?: string | null;
}

export interface WireRealmSummary {
  auth_profile_count: number;
  backend_count: number;
  binding_count: number;
  default_binding?: string | null;
  realm_id: string;
}

export interface WireRealmList {
  realms: WireRealmSummary[];
}

export interface WireAuthProfilesList {
  auth_profiles: WireAuthProfile[];
  backend_profiles: WireBackendProfile[];
  bindings: WireProviderBinding[];
  realm_id: string;
}

export interface WireAuthStatus {
  account_id?: string | null;
  auth_method: string;
  expires_at?: string | null;
  last_error?: Record<string, unknown> | null;
  last_refresh_at?: string | null;
  profile_id: string;
  provider: string;
  state: "valid" | "expiring" | "expired" | "reauth_required" | "refresh_failed" | "released" | "absent" | "missing_credential";
}

export interface WireAuthStatusDetail {
  account_id?: string | null;
  auth_binding: WireAuthBindingRef;
  auth_method: string;
  binding_id: string;
  expires_at?: string | null;
  has_refresh_token: boolean;
  last_refresh_at?: string | null;
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

export interface WireProviderMetaAnthropic {
  provider: "anthropic";
  signature: string;
}

export interface WireProviderMetaAnthropicRedacted {
  data: string;
  provider: "anthropic_redacted";
}

export interface WireProviderMetaAnthropicCompaction {
  content: string;
  provider: "anthropic_compaction";
}

export interface WireProviderMetaGemini {
  provider: "gemini";
  thoughtSignature: string;
}

export interface WireProviderMetaOpenAi {
  encrypted_content?: string | null;
  id: string;
  phase?: string | null;
  provider: "open_ai";
  response_id?: string | null;
}

export interface WireProviderMetaOpenAiResponse {
  provider: "open_ai_response";
  response_id: string;
}

export interface WireProviderMetaUnknown {
  provider: "unknown";
}

export type WireProviderMeta = WireProviderMetaAnthropic | WireProviderMetaAnthropicRedacted | WireProviderMetaAnthropicCompaction | WireProviderMetaGemini | WireProviderMetaOpenAi | WireProviderMetaOpenAiResponse | WireProviderMetaUnknown;

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
  data: { meta?: WireProviderMeta | null; text: string };
}

export interface WireAssistantBlockTranscript {
  block_type: "transcript";
  data: { meta?: WireProviderMeta | null; source: WireTranscriptSource; text: string };
}

export interface WireAssistantBlockReasoning {
  block_type: "reasoning";
  data: { meta?: WireProviderMeta | null; text?: string };
}

export interface WireAssistantBlockToolUse {
  block_type: "tool_use";
  data: { args: unknown; id: string; meta?: WireProviderMeta | null; name: string };
}

export interface WireAssistantBlockServerToolContent {
  block_type: "server_tool_content";
  data: { content: unknown; id?: string | null; kind: WireServerToolKind; meta?: WireProviderMeta | null };
}

export interface WireAssistantBlockImage {
  block_type: "image";
  data: { blob_ref: BlobRef; height: number; image_id: AssistantImageId; media_type: string; meta: ProviderImageMetadata; revised_prompt: RevisedPromptDisposition; width: number };
}

export interface WireAssistantBlockUnknown {
  block_type: "unknown";
}

export type WireAssistantBlock = WireAssistantBlockText | WireAssistantBlockTranscript | WireAssistantBlockReasoning | WireAssistantBlockToolUse | WireAssistantBlockServerToolContent | WireAssistantBlockImage | WireAssistantBlockUnknown;

export interface TranscriptRewriteReason {
  kind: string;
  note?: string | null;
}

export interface CompactionRewriteRange {
  end: number;
  start: number;
}

export interface TranscriptEditRewriteRange {
  end: number;
  start: number;
}

export interface TranscriptRewriteSelectionMessageRange {
  end: number;
  start: number;
  type: "message_range";
}

export interface TranscriptRewriteSelectionEditMessageRange {
  range: TranscriptEditRewriteRange;
  type: "edit_message_range";
}

export interface TranscriptRewriteSelectionCompactionMessageRange {
  range: CompactionRewriteRange;
  type: "compaction_message_range";
}

export type TranscriptRewriteSelection = TranscriptRewriteSelectionMessageRange | TranscriptRewriteSelectionEditMessageRange | TranscriptRewriteSelectionCompactionMessageRange;

export type BackgroundJobTerminalStatus = "completed" | "failed" | "aborted" | "cancelled" | "retired" | "terminated";

export type CommsNoticeKind = string;

export type SystemNoticeDirection = "incoming" | "outgoing" | "internal";

export interface SystemNoticeBlockComms {
  content?: ContentBlock[];
  direction: SystemNoticeDirection;
  intent?: string | null;
  kind: CommsNoticeKind;
  payload?: unknown;
  peer?: SystemNoticePeer | null;
  request_id?: string | null;
  sender_taint?: SenderContentTaint | null;
  status?: string | null;
  summary?: string | null;
  type: "comms";
}

export interface SystemNoticeBlockExternalEvent {
  body?: string | null;
  content?: ContentBlock[];
  event_type: string;
  payload?: unknown;
  source: string;
  summary?: string | null;
  type: "external_event";
}

export interface SystemNoticeBlockToolConfig {
  payload: ToolConfigChangedPayload;
  type: "tool_config";
}

export interface SystemNoticeBlockMcp {
  detail?: string | null;
  operation?: ToolConfigChangeOperation | null;
  pending_sources?: string[];
  persisted?: boolean;
  phase?: ExternalToolDeltaPhase | null;
  server_id?: string | null;
  type: "mcp";
}

export interface SystemNoticeBlockBackgroundJob {
  detail?: string | null;
  display_name?: string | null;
  job_id: string;
  status: BackgroundJobTerminalStatus;
  type: "background_job";
}

export interface SystemNoticeBlockAuth {
  binding?: string | null;
  detail?: string | null;
  state: string;
  type: "auth";
}

export interface SystemNoticeBlockRuntimeNotice {
  category: string;
  detail?: string | null;
  payload?: unknown;
  type: "runtime_notice";
}

export interface SystemNoticeBlockUnknown {
  payload?: unknown;
  summary?: string | null;
  type: "unknown";
}

export type SystemNoticeBlock = SystemNoticeBlockComms | SystemNoticeBlockExternalEvent | SystemNoticeBlockToolConfig | SystemNoticeBlockMcp | SystemNoticeBlockBackgroundJob | SystemNoticeBlockAuth | SystemNoticeBlockRuntimeNotice | SystemNoticeBlockUnknown;

export type SystemNoticeKind = "generic" | "comms" | "external_event" | "mcp_pending" | "mcp" | "background_job" | "tool_scope" | "tool_scope_warning" | "auth_reauth_required";

export type TranscriptUserRole = "conversational" | "compaction_summary" | "injected_context";

export type AssistantImageId = string;

export interface BlobRef {
  blob_id: BlobId;
  media_type: string;
}

export interface GeminiImageMetadata {
  continuity_ref?: string | null;
  response_id?: string | null;
  target_model: string;
}

export interface OpenAiImageMetadata {
  image_generation_call_id?: string | null;
  response_id?: string | null;
  target_model: string;
}

export interface ProviderImageMetadataNotEmitted {
  provider: "not_emitted";
}

export interface ProviderImageMetadataOpenai {
  image_generation_call_id?: string | null;
  response_id?: string | null;
  target_model: string;
  provider: "openai";
}

export interface ProviderImageMetadataGemini {
  continuity_ref?: string | null;
  response_id?: string | null;
  target_model: string;
  provider: "gemini";
}

export type ProviderImageMetadata = ProviderImageMetadataNotEmitted | ProviderImageMetadataOpenai | ProviderImageMetadataGemini;

export interface PromptText {
  content: string;
}

export type RevisedPromptSource = "provider" | "meerkat_projection";

export interface RevisedPromptDispositionNotRequested {
  disposition: "not_requested";
}

export interface RevisedPromptDispositionUnsupportedByBackend {
  disposition: "unsupported_by_backend";
}

export interface RevisedPromptDispositionUnchanged {
  disposition: "unchanged";
}

export interface RevisedPromptDispositionRevised {
  disposition: "revised";
  source: RevisedPromptSource;
  text: PromptText;
}

export type RevisedPromptDisposition = RevisedPromptDispositionNotRequested | RevisedPromptDispositionUnsupportedByBackend | RevisedPromptDispositionUnchanged | RevisedPromptDispositionRevised;

export interface WireServerToolKindWebSearch {
  kind: "web_search";
}

export interface WireServerToolKindGoogleSearch {
  kind: "google_search";
}

export interface WireServerToolKindProviderNative {
  kind: "provider_native";
  name: string;
}

export interface WireServerToolKindUnknown {
  debug: string;
  kind: "unknown";
}

export type WireServerToolKind = WireServerToolKindWebSearch | WireServerToolKindGoogleSearch | WireServerToolKindProviderNative | WireServerToolKindUnknown;

export interface TranscriptRewriteMessageSystem {
  content: string;
  created_at?: string | null;
  role: "system";
}

export interface TranscriptRewriteMessageSystemNotice {
  blocks?: SystemNoticeBlock[];
  body?: string | null;
  created_at?: string | null;
  kind: SystemNoticeKind;
  role: "system_notice";
}

export interface TranscriptRewriteMessageUser {
  content: WireContentInput;
  created_at?: string | null;
  role: "user";
  transcript_role?: TranscriptUserRole;
}

export interface TranscriptRewriteMessageBlockAssistant {
  blocks: WireAssistantBlock[];
  created_at?: string | null;
  role: "block_assistant";
  stop_reason?: WireStopReason;
}

export interface TranscriptRewriteMessageToolResults {
  created_at?: string | null;
  results: WireToolResult[];
  role: "tool_results";
}

export type TranscriptRewriteMessage = TranscriptRewriteMessageSystem | TranscriptRewriteMessageSystemNotice | TranscriptRewriteMessageUser | TranscriptRewriteMessageBlockAssistant | TranscriptRewriteMessageToolResults;

export interface WireTranscriptReplacementMessage {
  message: TranscriptRewriteMessage;
  type: "message";
}

export interface WireTranscriptReplacementUserContentBlock {
  block: WireContentBlock;
  block_index: number;
  type: "user_content_block";
}

export interface WireTranscriptReplacementAssistantBlock {
  block: WireAssistantBlock;
  block_index: number;
  type: "assistant_block";
}

export interface WireTranscriptReplacementToolResultContentBlock {
  block: WireContentBlock;
  block_index: number;
  result_index: number;
  type: "tool_result_content_block";
}

export type WireTranscriptReplacement = WireTranscriptReplacementMessage | WireTranscriptReplacementUserContentBlock | WireTranscriptReplacementAssistantBlock | WireTranscriptReplacementToolResultContentBlock;

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

export interface WireSessionMessageSystem {
  content: string;
  created_at: string;
  role: "system";
}

export interface WireSessionMessageSystemNotice {
  blocks?: SystemNoticeBlock[];
  body?: string | null;
  created_at: string;
  kind: SystemNoticeKind;
  role: "system_notice";
}

export interface WireSessionMessageUser {
  content: WireContentInput;
  created_at: string;
  interaction_id?: string | null;
  role: "user";
  run_id?: RunId | null;
  transcript_role?: TranscriptUserRole;
}

export interface WireSessionMessageBlockAssistant {
  blocks: WireAssistantBlock[];
  created_at: string;
  interaction_id?: string | null;
  role: "block_assistant";
  run_id?: RunId | null;
  stop_reason: WireStopReason;
}

export interface WireSessionMessageToolResults {
  created_at: string;
  results: WireToolResult[];
  role: "tool_results";
}

export type WireSessionMessage = WireSessionMessageSystem | WireSessionMessageSystemNotice | WireSessionMessageUser | WireSessionMessageBlockAssistant | WireSessionMessageToolResults;

export type WireHistoryRow = WireSessionMessage;

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
