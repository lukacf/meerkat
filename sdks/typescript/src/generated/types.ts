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

export type WireContentInput = string | Record<string, unknown>[];

export type McpLiveOperation = "add" | "remove" | "reload";

export type McpLiveOpStatus = "staged" | "applied" | "rejected";

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
  supported: string[];
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
  protocol_version: string;
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
  protocol_version: string;
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

export type WireToolResultContent = string | Record<string, unknown>[];

export type WireModelTier = "recommended" | "supported";

export interface CommsCommandInput {
  allow_self_session?: boolean;
  blocks?: Record<string, unknown>[];
  body: string;
  handling_mode?: "queue" | "steer";
  kind: "input";
  source?: "tcp" | "uds" | "stdin" | "webhook" | "rpc";
  stream?: "none" | "reserve_interaction";
}

export interface CommsCommandPeerMessage {
  blocks?: Record<string, unknown>[];
  body: string;
  handling_mode?: "queue" | "steer";
  kind: "peer_message";
  to: string;
}

export interface CommsCommandPeerLifecycle {
  kind: "peer_lifecycle";
  lifecycle_kind: "mob.peer_added" | "mob.peer_retired" | "mob.peer_unwired";
  params?: unknown;
  to: string;
}

export interface CommsCommandPeerRequest {
  handling_mode?: "queue" | "steer";
  intent: string;
  kind: "peer_request";
  params?: unknown;
  stream?: "none" | "reserve_interaction";
  to: string;
}

export interface CommsCommandPeerResponse {
  handling_mode?: "queue" | "steer";
  in_reply_to: string;
  kind: "peer_response";
  result?: unknown;
  status: "accepted" | "completed" | "failed";
  to: string;
}

export type CommsCommandRequest = CommsCommandInput | CommsCommandPeerMessage | CommsCommandPeerLifecycle | CommsCommandPeerRequest | CommsCommandPeerResponse;

export interface WireRenderMetadata {
  class: "user_prompt" | "peer_message" | "peer_request" | "peer_response" | "external_event" | "flow_step" | "continuation" | "system_notice" | "tool_scope_notice" | "ops_progress";
  salience?: "background" | "normal" | "important" | "urgent";
}

export interface WireTrustedPeerSpec {
  address: string;
  name: string;
  peer_id: string;
  pubkey?: number[];
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

export interface RuntimeRealtimeAttachmentStatusResult {
  status: "unattached" | "intent_present_unbound" | "binding_not_ready" | "binding_ready" | "replacement_pending" | "reattach_required";
}

export interface RealtimeReconnectPolicy {
  initial_backoff_ms: number;
  max_attempts: number;
  max_backoff_ms: number;
  max_total_ms: number;
}

export interface RealtimeChannelConfig {
  tool_timeout_ms?: number;
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
  default_protocol_version: string;
  expires_at: string;
  open_token: string;
  supported_protocol_versions?: string[];
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
  protocol_version: string;
  role: RealtimeChannelRole;
  turning_mode: RealtimeTurningMode;
}

export interface RealtimeChannelInputFrame {
  chunk: RealtimeInputChunk;
}

export interface RealtimeChannelOpenedFrame {
  capabilities: RealtimeCapabilities;
  protocol_version: string;
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

export interface InputListResult {
  inputs: Record<string, unknown>[];
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

export interface WireConnectionRef {
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
  auth_profiles: Record<string, unknown>;
  backends: Record<string, unknown>;
  bindings: Record<string, unknown>;
  default_binding?: string;
  realm_id: string;
}

export interface WireBindingIdentity {
  binding_id: string;
  connection_ref: Record<string, unknown>;
  realm_id: string;
}

export interface WireAuthProfileCreated {
  auth_method: string;
  binding_id: string;
  connection_ref: Record<string, unknown>;
  profile_id: string;
  provider: string;
  realm_id: string;
  stored: boolean;
}

export interface WireAuthProfileDetail {
  auth_profile: Record<string, unknown>;
  binding_id: string;
  connection_ref: Record<string, unknown>;
  profile_id: string;
}

export interface WireAuthProfileCleared {
  binding_id: string;
  cleared: boolean;
  connection_ref: Record<string, unknown>;
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
  binding_id: string;
  connection_ref: Record<string, unknown>;
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
  state: string;
}

export interface WireAuthStatusDetail {
  account_id?: string;
  auth_method: string;
  binding_id: string;
  connection_ref: Record<string, unknown>;
  expires_at?: string;
  has_refresh_token: boolean;
  last_refresh_at?: string;
  profile_id: string;
  provider: string;
  realm_id: string;
  state: string;
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

export type WireAuthError = WireAuthErrorMissingSecret | WireAuthErrorUnsupportedCombination | WireAuthErrorMissingRequiredMetadata | WireAuthErrorWorkspaceMismatch | WireAuthErrorExpired | WireAuthErrorRefreshFailed | WireAuthErrorInteractiveLoginRequired | WireAuthErrorHostOwnedUnavailable | WireAuthErrorIo | WireAuthErrorOther;

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
