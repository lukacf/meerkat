// Generated wire types for Meerkat SDK
// Contract version: 0.4.13

export const CONTRACT_VERSION = "0.4.13";

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
  content: string;
  is_error?: boolean;
}

export interface WireSessionMessage {
  role: string;
  content?: string;
  tool_calls?: WireToolCall[];
  stop_reason?: string;
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
  host_mode: boolean;
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

export interface MobSendParams {
  content: WireContentInput;
  handling_mode?: WireHandlingMode;
  meerkat_id: string;
  mob_id: string;
  render_metadata?: WireRenderMetadata;
}

export interface MobWireParams {
  member: string;
  mob_id: string;
  peer: MobPeerTarget;
}

export interface MobUnwireParams {
  member: string;
  mob_id: string;
  peer: MobPeerTarget;
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

export interface McpLiveOpResponse {
  applied_at_turn?: number;
  operation: McpLiveOperation;
  persisted: boolean;
  server_name?: string;
  session_id: string;
  status: McpLiveOpStatus;
}

export type InputStateResult = WireInputState | null;

export type WireContentBlock = Record<string, unknown>;

export type WireContentInput = string | WireContentBlock[];

export type McpLiveOperation = "add" | "remove" | "reload";

export type McpLiveOpStatus = "staged" | "applied" | "rejected";

export type MobPeerTarget = { local: string } | { external: WireTrustedPeerSpec };

export type WireHandlingMode = "queue" | "steer";

export type WireRenderClass = "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";

export type WireRenderSalience = "background" | "normal" | "important" | "urgent";

export type WireRuntimeState = "initializing" | "idle" | "attached" | "running" | "recovering" | "retired" | "stopped" | "destroyed";

export type RuntimeAcceptOutcomeType = "accepted" | "deduplicated" | "rejected";

export type WireInputLifecycleState = "accepted" | "queued" | "staged" | "applied" | "applied_pending_consumption" | "consumed" | "superseded" | "coalesced" | "abandoned";

export interface WireRenderMetadata {
  class: WireRenderClass;
  salience?: WireRenderSalience;
}

export interface MobSendResult {
  handling_mode: WireHandlingMode;
  member_id: string;
  session_id: string;
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
  state: WireRuntimeState;
}

export interface RuntimeAcceptResult {
  existing_id?: string;
  input_id?: string;
  outcome_type: RuntimeAcceptOutcomeType;
  policy?: unknown;
  reason?: string;
  state?: WireInputState;
}

export interface RuntimeRetireResult {
  inputs_abandoned: number;
  inputs_pending_drain?: number;
}

export interface RuntimeResetResult {
  inputs_abandoned: number;
}

export interface WireInputStateHistoryEntry {
  from: WireInputLifecycleState;
  reason?: string;
  timestamp: string;
  to: WireInputLifecycleState;
}

export interface WireInputState {
  attempt_count?: number;
  created_at: string;
  current_state: WireInputLifecycleState;
  durability?: unknown;
  history?: WireInputStateHistoryEntry[];
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
