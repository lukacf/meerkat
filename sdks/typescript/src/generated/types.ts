// Generated wire types for Meerkat SDK
// Contract version: 0.5.1

export const CONTRACT_VERSION = "0.5.1";

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
  structured_output?: unknown;
  schema_warnings?: Array<{ provider: string; path: string; message: string }>;
}

export interface WireProviderMeta {
  provider: string;
  [key: string]: unknown;
}

export interface WireAssistantBlock {
  block_type: string;
  data: Record<string, unknown>;
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
  skill_references: string[];
}

export interface McpAddParams {
  persisted?: boolean;
  server_config: unknown;
  server_name: string;
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

export interface RuntimeStateParams {
  session_id: string;
}

export interface RuntimeAcceptParams {
  input: unknown;
  session_id: string;
}

export interface RuntimeRetireParams {
  session_id: string;
}

export interface RuntimeResetParams {
  session_id: string;
}

export interface InputStateParams {
  input_id: string;
  session_id: string;
}

export interface InputListParams {
  session_id: string;
}

export interface ScheduleIdParams {
  schedule_id: string;
}

export interface ListSchedulesParams {
  labels?: Record<string, unknown>;
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
  labels?: Record<string, unknown>;
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

export interface McpLiveOpResponse {
  applied_at_turn?: number;
  operation: "add" | "remove" | "reload";
  persisted: boolean;
  server_name?: string;
  session_id: string;
  status: "staged" | "applied" | "rejected";
}

export type InputStateResult = WireInputState | null;

export type WireContentBlock = Record<string, unknown>;

export type WireContentInput = string | Record<string, unknown>[];

export type McpLiveOperation = "add" | "remove" | "reload";

export type McpLiveOpStatus = "staged" | "applied" | "rejected";

export type MobPeerTarget = { local: string } | { external: WireTrustedPeerSpec };

export type WireHandlingMode = "queue" | "steer";

export type WireRenderClass = "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";

export type WireRenderSalience = "background" | "normal" | "important" | "urgent";

export type WireRuntimeState = "initializing" | "idle" | "attached" | "running" | "recovering" | "retired" | "stopped" | "destroyed";

export type RuntimeAcceptOutcomeType = "accepted" | "deduplicated" | "rejected";

export type WireInputLifecycleState = "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";

export type WireStopReason = "end_turn" | "tool_use" | "max_tokens" | "stop_sequence" | "content_filter" | "cancelled";

export type WireToolResultContent = string | Record<string, unknown>[];

export type WireModelTier = string;

export interface WireRenderMetadata {
  class: "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";
  salience?: "background" | "normal" | "important" | "urgent";
}

export interface WireTrustedPeerSpec {
  address: string;
  name: string;
  peer_id: string;
}

export interface MobWireResult {
  wired: boolean;
}

export interface MobUnwireResult {
  unwired: boolean;
}

export interface RuntimeStateResult {
  state: "initializing" | "idle" | "attached" | "running" | "recovering" | "retired" | "stopped" | "destroyed";
}

export interface RuntimeAcceptResult {
  existing_id?: string;
  input_id?: string;
  outcome_type: "accepted" | "deduplicated" | "rejected";
  policy?: unknown;
  reason?: string;
  state?: Record<string, unknown>;
}

export interface RuntimeRetireResult {
  inputs_abandoned: number;
  inputs_pending_drain?: number;
}

export interface RuntimeResetResult {
  inputs_abandoned: number;
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
  durability?: unknown;
  history?: Record<string, unknown>[];
  idempotency_key?: string;
  input_id: string;
  last_boundary_sequence?: number;
  last_run_id?: string;
  persisted_input?: unknown;
  policy?: unknown;
  reconstruction_source?: unknown;
  recovery_count?: number;
  terminal_outcome?: unknown;
  updated_at: string;
}

export interface InputListResult {
  input_ids: string[];
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
  labels?: Record<string, unknown>;
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
  labels?: Record<string, unknown>;
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
  tier: string;
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
  inline_video: boolean;
  model_family: string;
  params_schema: unknown;
  supports_reasoning: boolean;
  supports_temperature: boolean;
  supports_thinking: boolean;
}
