// Generated direct-session contracts for @rkat/web
// Source: artifacts/schemas/wire-types.json
// Regenerate with: python3 tools/sdk-codegen/generate.py

export type AppendSystemContextStatus = "applied" | "duplicate";

export type CapabilityId = string;

export type ExtractionError = {
  attempts: number;
  last_output: string;
  reason: string;
};

export type InjectSystemContextResult = {
  status: AppendSystemContextStatus;
};

export type PresentedTokenConvention = "anthropic_disjoint_input_components" | "open_ai_input_includes_cached_subset" | "gemini_prompt_includes_cached_subset" | "open_ai_compatible_prompt_includes_cache_details" | "host_declared_inclusive_input_total";

export type Provider = "anthropic" | "openai" | "gemini" | "self_hosted" | "other";

export type ProviderTokenAccounting = {
  aggregation: TokenAggregationProvenance;
  convention: PresentedTokenConvention;
  model: string;
  presented_tokens: number;
  provider: Provider;
};

export type QuarantinedSkillIdentity = {
  raw_id: string;
  source_uuid: SourceUuid;
};

export type RuntimeProfileCapability = "in_memory_persistence" | "durable_persistence" | "foreground_execution" | "background_execution" | "keep_alive" | "comms" | "transient_turn_context" | "shell" | "process_spawn" | "mcp_stdio" | "remote_member_placement" | "hooks" | "runtime_skills" | "file_schema_resolution" | "embedded_skills" | "schedule" | "work_graph" | "semantic_memory" | "image_generation" | "fallback_web_search" | "mcp_client" | "tcp_comms" | "uds_comms";

export type RuntimeProfileClearingAction = "use_persistent_runtime" | "use_background_runtime" | "use_host_process_runtime" | "use_local_member_placement" | "use_hook_runtime" | "use_skill_runtime" | "use_inline_schema" | "use_schedule_runtime" | "use_work_graph_runtime" | "use_semantic_memory_runtime" | "use_image_generation_runtime" | "use_web_search_runtime" | "use_mcp_runtime" | "use_in_process_comms";

export type RuntimeProfileId = "browser";

export type RuntimeProfileRefusal = {
  code: RuntimeProfileRefusalCode;
  data: RuntimeProfileRefusalData;
  message: string;
};

export type RuntimeProfileRefusalCode = "CAPABILITY_UNAVAILABLE";

export type RuntimeProfileRefusalData = {
  capability: RuntimeProfileCapability;
  clearing_action: RuntimeProfileClearingAction;
  profile: RuntimeProfileId;
};

export type SchemaWarning = {
  message: string;
  path: string;
  provider: Provider;
};

export type SessionId = string;

export type SkillKey = {
  skill_name: SkillName;
  source_uuid: SourceUuid;
};

export type SkillName = string;

export type SkillQuarantineDiagnostic = {
  error_class: string;
  error_code: string;
  first_seen_unix_secs: number;
  identity: QuarantinedSkillIdentity;
  last_seen_unix_secs: number;
  location: string;
  message: string;
};

export type SkillResolutionFailureReason = {
  key: SkillKey;
  reason_type: "not_found";
} | {
  capability: CapabilityId;
  key: SkillKey;
  reason_type: "capability_unavailable";
} | {
  message: string;
  reason_type: "load";
} | {
  message: string;
  reason_type: "parse";
} | {
  existing_fingerprint: string;
  new_fingerprint: string;
  reason_type: "source_uuid_collision";
  source_uuid: string;
} | {
  existing_source_uuid: string;
  fingerprint: string;
  mutated_source_uuid: string;
  reason_type: "source_uuid_mutation_without_lineage";
} | {
  event_id: string;
  event_kind: string;
  reason_type: "missing_skill_remaps";
} | {
  from_skill_name: string;
  from_source_uuid: string;
  reason_type: "remap_without_lineage";
  to_skill_name: string;
  to_source_uuid: string;
} | {
  alias: string;
  reason_type: "unknown_skill_alias";
} | {
  reason_type: "remap_cycle";
  skill_name: string;
  source_uuid: string;
} | {
  reason_type: "no_skill_engine";
  requested: SkillKey[];
} | {
  message: string;
  reason_type: "unknown";
};

export type SkillRuntimeDiagnostics = {
  collection_fault?: SkillResolutionFailureReason | null;
  quarantined: SkillQuarantineDiagnostic[];
  source_health: SourceHealthSnapshot;
};

export type SourceHealthSnapshot = {
  failure_streak: number;
  handshake_failed: boolean;
  invalid_count: number;
  invalid_ratio: number;
  state: SourceHealthState;
  total_count: number;
};

export type SourceHealthState = "healthy" | "degraded" | "unhealthy";

export type SourceUuid = string;

export type TokenAggregationProvenance = "sum_disjoint_provider_components" | "provider_inclusive_input_total";

export type TurnRequestContext = string;

export type TurnTerminalCauseKind = "unknown" | "hook_denied" | "hook_failure" | "llm_failure" | "tool_failure" | "structured_output_validation_failed" | "budget_exhausted" | "time_budget_exceeded" | "retry_exhausted" | "turn_limit_reached" | "runtime_apply_failure" | "fatal_failure";

export type WireHandlingMode = "queue" | "steer";

export type WireResolvedModelCapabilities = {
  image_generation?: boolean;
  image_input?: boolean;
  image_tool_results?: boolean;
  inline_video?: boolean;
  mid_conversation_system_messages?: boolean;
  realtime?: boolean;
  vision?: boolean;
  web_search?: boolean;
};

export type WireRunResult = {
  extraction_error?: ExtractionError | null;
  request_usage?: WireTurnUsage[];
  run_usage?: WireUsage | null;
  schema_warnings?: SchemaWarning[] | null;
  session_id: SessionId;
  session_ref?: string | null;
  skill_diagnostics?: SkillRuntimeDiagnostics | null;
  structured_output?: unknown;
  terminal_cause_kind?: TurnTerminalCauseKind | null;
  text: string;
  tool_calls: number;
  turns: number;
  usage: WireUsage;
};

export type WireSessionInfo = {
  created_at: number;
  is_active: boolean;
  labels?: Record<string, unknown>;
  last_assistant_text?: string | null;
  message_count: number;
  model: string;
  provider: string;
  resolved_capabilities?: WireResolvedModelCapabilities | null;
  session_id: string;
  session_ref?: string | null;
  updated_at: number;
};

export type WireTurnInputOptions = {
  handling_mode?: WireHandlingMode | null;
  skill_references?: SkillKey[] | null;
  transient_turn_context?: TurnRequestContext | null;
};

export type WireTurnUsage = {
  accounting: ProviderTokenAccounting;
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens?: number | null;
  total_tokens: number;
};

export type WireUsage = {
  cache_creation_tokens?: number | null;
  cache_read_tokens?: number | null;
  input_tokens: number;
  output_tokens: number;
  reasoning_tokens?: number | null;
  total_tokens: number;
};

/** Canonical session/read projection. */
export type SessionState = WireSessionInfo;
