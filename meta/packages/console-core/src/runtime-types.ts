import type {
  MemberProgress,
  ActivityFilterPreset,
  ConsoleInteractionRejectedError,
  ExperienceSectionMeta,
  GatingActionRequest,
  GatingActionResult,
  IdentityStatusRow,
  IdentityInspectViewState,
  ReplayUnavailableError,
  ResponsePhase,
  RoutingSectionView,
  SidebarWatchFields,
  ToolCallAccumulatorState,
  UnobservedFacts,
} from "./control-plane";
import type { TopologyEndpointRef } from "./topology";

export interface ConsoleFrame {
  id: string;
  event: string;
  identity?: string;
  interactionId?: string;
  timestampMs?: number;
  cursor?: string;
  runtimeKey?: string;
  sessionId?: string;
  status?: string;
  sourceKind?: string;
  sourceCursor?: string;
  frameVersion?: number;
  updatedAtMs?: number;
  turnId?: string;
  runId?: string;
  data: unknown;
}

export interface ConsoleTimelineAccepted {
  interaction_id: string;
  identity: string;
  conversation_id?: string;
  session_id?: string;
  input_frame_id?: string;
  cursor?: string;
  status?: string;
}

export interface ConsoleTimelinePage {
  frames: ConsoleFrame[];
  nextCursor?: string;
  latestCursor?: string;
  exhausted?: boolean;
  available: boolean;
}

export type ConsoleGatewayInteractionRejectedError = ConsoleInteractionRejectedError;

export interface ConsoleAgentAffordances {
  addressable?: boolean;
  can_send_message?: boolean;
  can_retire?: boolean;
  can_respawn?: boolean;
  runtime_mode?: string;
}

export interface ConsoleModelCapabilities {
  image_input?: boolean;
}

export interface ConsoleAgent extends SidebarWatchFields {
  identity?: string;
  agent_id: string;
  member_id: string;
  session_id?: string;
  label: string;
  kind: string;
  role?: string;
  state?: string;
  addressability?: IdentityStatusRow["addressability"];
  generation?: number;
  progress?: MemberProgress;
  response_phase?: ResponsePhase;
  wired_to?: string[];
  labels?: Record<string, string>;
  group?: string;
  subgroup?: string;
  addressable?: boolean;
  affordances?: ConsoleAgentAffordances;
  model_capabilities?: ConsoleModelCapabilities;
  unobserved?: UnobservedFacts;
}

export interface ConsoleExperienceAgentSnapshotRow extends Partial<IdentityStatusRow>, SidebarWatchFields {
  identity?: string;
  agent_id?: string;
  member_id?: string;
  session_id?: string;
  label?: string;
  kind?: string;
  role?: string;
  state?: string;
  response_phase?: ResponsePhase;
  wired_to?: string[];
  labels?: Record<string, string>;
  group?: string;
  subgroup?: string;
  addressable?: boolean;
  affordances?: ConsoleAgentAffordances;
  model_capabilities?: ConsoleModelCapabilities;
}

export interface ConsoleSidebarButtonConfig {
  id: string;
  label: string;
  control?: string;
  href?: string;
  target?: string;
  icon_name?: string;
  iconName?: string;
}

export interface ConsoleSidebarUiConfig {
  visible_controls?: string[];
  hidden_controls?: string[];
  buttons?: ConsoleSidebarButtonConfig[];
}

export interface ConsoleAgentListConfig {
  group_by?: string[];
  subgroup_by?: string[];
  section_order?: string[];
  fallback_group?: string;
  fallback_subgroup?: string;
  collapse_single_subgroup?: boolean;
  default_pinned_agent_ids?: string[];
  badges?: ConsoleAgentBadgeConfig[];
  sections?: ConsoleAgentSectionConfig[];
}

export interface ConsoleAgentBadgeConfig {
  id: string;
  label: string;
  field: string;
  tone?: string;
}

export interface ConsoleAgentSectionConfig {
  name: string;
  collapsed?: boolean;
  empty_title?: string;
  empty_text?: string;
}

export interface ConsoleBrandingConfig {
  label?: string;
  logo_url?: string;
  logo_alt?: string;
}

export interface ConsoleAppearanceConfig {
  default_theme?: string;
  default_variant?: string;
}

export interface ConsoleEnvironmentConfig {
  label?: string;
}

export interface ConsoleLayoutConfig {
  initial_preset?: string;
  initial_control?: string;
  initial_agent?: string;
  sidebar_collapsed?: boolean;
}

export interface ConsoleRailFilterPresetConfig {
  id: string;
  label: string;
  watchedOnly?: boolean;
  alertLevels?: string[];
}

export interface ConsoleRailUiConfig {
  visible?: boolean;
  collapsed?: boolean;
  active_preset_id?: string;
  empty_text?: string;
  filter_presets?: ConsoleRailFilterPresetConfig[];
}

export interface ConsoleActionsUiConfig {
  inspect_label?: string;
  chat_label?: string;
  send_label?: string;
  respawn_label?: string;
  retire_label?: string;
  reset_label?: string;
  show_inspect?: boolean;
  show_chat?: boolean;
  show_respawn?: boolean;
  show_retire?: boolean;
  show_reset?: boolean;
}

export interface ConsoleUiConfig {
  /** Trusted same-origin ES modules exporting a ConsolePanel. */
  panel_modules?: string[];
  /** Isolated MCP Apps proxy origin. Omit to retain text-only tool results. */
  mcp_apps_sandbox_url?: string;
  title?: string;
  brand?: ConsoleBrandingConfig;
  appearance?: ConsoleAppearanceConfig;
  environment?: ConsoleEnvironmentConfig;
  layout?: ConsoleLayoutConfig;
  rail?: ConsoleRailUiConfig;
  sidebar?: ConsoleSidebarUiConfig;
  agent_list?: ConsoleAgentListConfig;
  actions?: ConsoleActionsUiConfig;
}

export interface ConsolePolicyConfig {
  fetch_timeout_ms?: number;
  read_only?: boolean;
}

export interface ConsoleTopologyNode {
  identity?: string;
  /** Optional canonical endpoint metadata for topology-aware hosts. */
  ref?: TopologyEndpointRef;
  label?: string;
  role?: string;
  state?: string;
  wired_to?: string[];
  labels?: Record<string, string>;
  group?: string;
  subgroup?: string;
  addressable?: boolean;
  unobserved?: UnobservedFacts;
}

export interface ConsoleHealthSnapshot {
  loaded_modules?: unknown[];
  loaded_module_count?: number;
  /** `null` when nothing observed whether the mob runs; `unobserved` names it. */
  running?: boolean | null;
  identities?: IdentityStatusRow[];
  unobserved?: UnobservedFacts;
}

type ConsoleExperienceSection<T> = T & Partial<ExperienceSectionMeta>;

export interface ProfileCapabilities {
  instance_count?: number;
  addressable?: boolean;
  has_wiring?: boolean;
}

export interface RuntimeCapabilities {
  can_spawn_members?: boolean;
  can_send_messages?: boolean;
  can_wire_members?: boolean;
  can_retire_members?: boolean;
  available_spawn_modes?: string[];
  profile_capabilities?: Record<string, ProfileCapabilities>;
}

export interface ConsoleExperience {
  /** Native store scopes exposed by a shared console, keyed by registration. */
  panel_runtimes?: Array<{key: string; label: string; namespace: string; memory: boolean; workgraph: boolean}>;
  /** Opaque server-owned runtime and authenticated principal scope for local drafts. */
  storage_scope?: string;
  /**
   * Whether the gateway's send dedupe records survive a restart, read from
   * the store it actually uses. Absent on older gateways. The console
   * resends an uncertain message (same idempotency key) only when true.
   */
  send_dedupe?: { durable?: boolean };
  contract_version?: string;
  runtime_id?: string;
  console_config?: ConsoleUiConfig;
  console_policy?: ConsolePolicyConfig;
  runtime_capabilities?: RuntimeCapabilities;
  agent_sidebar?: ConsoleExperienceSection<{
    title?: string;
    live_snapshot?: {
      agents?: ConsoleExperienceAgentSnapshotRow[];
    };
  }>;
  activity_feed?: ConsoleExperienceSection<{
    title?: string;
    filter_presets?: ActivityFilterPreset[];
    active_preset_id?: string;
  }>;
  identity_status?: ConsoleExperienceSection<{
    title?: string;
    rows?: IdentityStatusRow[];
  }>;
  chat_inspector?: ConsoleExperienceSection<{
    title?: string;
    inspect_identity_method?: string;
    live_snapshot?: IdentityInspectViewState | null;
  }>;
  flows?: ConsoleExperienceSection<{
    title?: string;
    evaluate_method?: string;
    dispatch_method?: string;
  }>;
  session_history?: ConsoleExperienceSection<{
    title?: string;
    source_method?: string;
  }>;
  routing?: ConsoleExperienceSection<RoutingSectionView & {
    title?: string;
  }>;
  gating?: ConsoleExperienceSection<{
    title?: string;
    decide_method?: string;
    pending?: unknown[];
    audit?: unknown[];
  }>;
  topology?: ConsoleExperienceSection<{
    title?: string;
    live_snapshot?: {
      nodes?: ConsoleTopologyNode[];
      node_count?: number;
    };
  }>;
  health_overview?: ConsoleExperienceSection<{
    title?: string;
    live_snapshot?: ConsoleHealthSnapshot;
  }>;
}

export interface ConsoleModulesResponse {
  modules?: unknown[];
}

export interface ConsoleGatingActionPayload extends GatingActionRequest {}

export interface ConsoleGatingActionResponse extends GatingActionResult {}

export interface ConsoleReplayUnavailablePayload extends ReplayUnavailableError {}

export interface ConsoleToolAccumulatorSnapshot extends ToolCallAccumulatorState {}
