import type {
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
  TopologyEndpointRef,
} from "@console-core";

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
  progress?: IdentityStatusRow["progress"];
  response_phase?: ResponsePhase;
  wired_to?: string[];
  labels?: Record<string, string>;
  group?: string;
  subgroup?: string;
  addressable?: boolean;
  affordances?: ConsoleAgentAffordances;
  model_capabilities?: ConsoleModelCapabilities;
  unobserved?: IdentityStatusRow["unobserved"];
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
  session_id?: string;
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
  /** Same-origin path of a docked operator panel (trusted operator code). */
  panel?: string;
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
  unobserved?: IdentityStatusRow["unobserved"];
}

export interface ConsoleHealthSnapshot {
  loaded_modules?: unknown[];
  loaded_module_count?: number;
  /** `null` when nothing observed whether the mob runs; `unobserved` names it. */
  running?: boolean | null;
  identities?: IdentityStatusRow[];
  unobserved?: IdentityStatusRow["unobserved"];
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
  /** Native stores currently registered with a shared console. */
  panel_runtimes?: import("@console-core").ConsoleExperience["panel_runtimes"];
  /** Opaque server-owned runtime and principal scope for browser persistence. */
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
  access?: ConsoleAccessSection;
  voice?: { available?: boolean; readiness_method?: string };
  memory?: ConsoleMemorySection;
  workgraph?: ConsoleWorkGraphSection;
}

/// Per-caller WorkGraph standing, projected by `/console/experience` when a
/// WorkGraph service is wired on the runtime. `available && can_view` gates
/// the WorkGraph nav entry; `can_manage` gates operator actions (claim/close/
/// confirm/attention mutations) on the inline card and the panel.
export interface ConsoleWorkGraphSection {
  available?: boolean;
  can_view?: boolean;
  can_manage?: boolean;
}

/// Per-caller memory-panel standing, projected by `/console/experience` when a
/// memory panel is wired on the runtime. `can_read` gates the Memory nav entry;
/// `can_review_quarantine` gates the quarantine tab.
export interface ConsoleMemorySection {
  available?: boolean;
  can_read?: boolean;
  can_review_quarantine?: boolean;
}

/// Per-caller access standing, projected by `/console/experience` when an
/// access controller is wired on the runtime.
export interface ConsoleAccessSection {
  available?: boolean;
  enabled?: boolean;
  subject?: string | null;
  groups?: string[];
  can_administer?: boolean;
}

export interface ConsoleAccessStatus extends ConsoleAccessSection {
  conditional_mutations?: string;
  owner_instance?: string;
  revision?: number;
  is_admin?: boolean;
  actions?: string[];
}

export interface ConsoleAccessRule {
  id: string;
  description?: string;
  effect?: "allow" | "deny";
  subjects?: string[];
  groups?: string[];
  actions: string[];
  agents?: string[];
  roles?: string[];
  match_labels?: Record<string, string>;
}

export interface ConsoleAccessGroup {
  description?: string;
  members?: string[];
}

export interface ConsoleAccessConfig {
  enabled?: boolean;
  admins?: string[];
  groups?: Record<string, ConsoleAccessGroup>;
  rules?: ConsoleAccessRule[];
}

/** Captured with the protected config read, never rebased by a display refresh. */
export interface ConsoleAccessEditBase {
  owner_instance: string;
  revision: number;
  config: ConsoleAccessConfig;
}

export interface ConsoleAccessSaveFailure {
  kind: "revision_conflict" | "owner_changed" | "unavailable" | "invalid" | "failed";
}

export type ConsoleAccessSaveResult = void | boolean | ConsoleAccessSaveFailure;

// ── Memory panel (read-only) ──────────────────────────────────────────────
// Shapes mirror the server contract for the `meta/memory/panel/*`
// JSON-RPC methods. All fields are best-effort/optional on the client so the
// panel degrades gracefully across runtime versions.

export type MemoryRecordScope =
  | { scope: "identity"; realm: string; identity: string }
  | { scope: "mob"; realm: string; mob: string }
  | { scope: "operator"; realm: string; operator: string }
  | { scope: "realm"; realm: string };

export type MemoryRecordKind =
  | "preference"
  | "fact"
  | "gotcha"
  | "procedure"
  | "relationship"
  | "open_loop"
  | "reference";

export type MemoryTrust =
  | "untrusted"
  | "agent_observed"
  | "agent_verified"
  | "application"
  | "operator";

export type MemoryRecordStatus =
  | { status: "active" }
  | { status: "superseded"; by?: string }
  | { status: "quarantined"; reason?: string }
  | { status: "tombstoned" };

export interface MemoryEvidenceRef {
  session_id?: string;
  generation?: number;
  revision?: string;
  range?: [number, number];
}

export type MemoryAuthor =
  | { author: "agent"; identity?: string }
  | { author: "steward"; run_id?: string }
  | { author: "distiller"; run_id?: string }
  | { author: "operator" }
  | { author: "application" };

export interface MemoryProvenance {
  evidence?: MemoryEvidenceRef[];
  author?: MemoryAuthor;
  verification?: {
    checked?: string;
    evidence?: MemoryEvidenceRef[];
  };
}

export interface MemoryUsage {
  injected_count?: number;
  last_injected_at_ms?: number;
  explicit_recall_count?: number;
  last_recalled_at_ms?: number;
  judged_useful_count?: number;
  last_useful_at_ms?: number;
}

export interface MemoryPanelRecord {
  id: string;
  scope: MemoryRecordScope;
  kind: MemoryRecordKind;
  title: string;
  description?: string;
  tags?: string[];
  provenance?: MemoryProvenance;
  trust: MemoryTrust;
  status: MemoryRecordStatus;
  supersedes?: string;
  derived_from?: string[];
  working_set_rank?: number;
  created_at_ms?: number;
  updated_at_ms?: number;
  usage?: MemoryUsage;
  body_bytes?: number;
  /// The durable taint marker: the record landed quarantined or descends
  /// from one that did.
  ever_quarantined?: boolean;
}

export interface MemoryFullRecord extends Omit<MemoryPanelRecord, "body_bytes"> {
  body: string;
  /// Detail-only: the content hash a quarantine decision binds to.
  content_hash?: string;
}

export interface MemoryInjectionEntry {
  record_id: string;
  identity: string;
  session_key?: string;
  surface: "build" | "turn";
  at_ms: number;
}

export interface MemoryPendingPromotion {
  realm: string;
  pending_id: string;
  record_id: string;
  scope_kind: string;
  scope_key: string;
  rationale?: string;
  status?: string;
  created_at_ms?: number;
}

export interface MemoryDreamRun {
  realm: string;
  run_id: string;
  first_op_at_ms?: number;
  last_op_at_ms?: number;
  ops?: number;
  op_kinds?: Record<string, number>;
  quarantined_ops?: number;
  memory_ids?: string[];
  rationales?: string[];
}

export interface MemoryPanelRecordsResult {
  records: MemoryPanelRecord[];
  next_cursor: string | null;
  realms: string[];
}

export interface MemoryPanelRecordResult {
  realm: string;
  record: MemoryFullRecord;
  chain: MemoryPanelRecord[];
  injections: MemoryInjectionEntry[];
}

export type MemoryQuarantineVerdict = "release" | "tombstone";

/// One record as it stands after a quarantine decision, read back from the
/// store (meta/memory/quarantine/decide).
export interface MemoryReviewedRecordReceipt {
  memory_id: string;
  scope: MemoryRecordScope;
  kind: MemoryRecordKind;
  status: MemoryRecordStatus;
  trust: MemoryTrust;
  ever_quarantined: boolean;
  content_hash: string;
  supersedes?: string;
  derived_from: string[];
  created_at_ms: number;
  updated_at_ms: number;
}

export type MemoryQuarantineReviewer =
  | { kind: "operator"; principal: string | null }
  | { kind: "steward"; run_id: string };

/// The committed review a decision result reports; a replay returns the
/// original one.
export interface MemoryQuarantineDecision {
  audit_token: string;
  decided_at_ms: number;
  review: {
    verdict: MemoryQuarantineVerdict;
    reviewer: MemoryQuarantineReviewer;
    origin: string;
    successor?: string;
    expected_content_hash: string;
    origin_quarantine_reason: string;
    expired_promotions?: string[];
    invalidated_promotions?: string[];
    rationale?: string;
  };
}

export interface MemoryQuarantineDecideResult {
  outcome: "released" | "already_released" | "tombstoned" | "already_tombstoned";
  realm: string;
  origin: MemoryReviewedRecordReceipt;
  successor: MemoryReviewedRecordReceipt | null;
  decision: MemoryQuarantineDecision;
  superseded_prior?: string | null;
}

export interface MemoryPanelQuarantineResult {
  records: MemoryPanelRecord[];
  pending_promotions: MemoryPendingPromotion[];
  realms: string[];
}

export interface MemoryPanelDreamsResult {
  runs: MemoryDreamRun[];
  realms: string[];
}

// ── Memory panel Phase-2 read surfaces ────────────────────────────────────

/// One scope row from panel/overview: full store totals per scope, plus the
/// server-computed floor-pressure flag (active records or body bytes at the
/// per-scope floor). `scope_key` is "" for realm scope.
export interface MemoryScopeOverview {
  realm: string;
  scope_kind: "identity" | "mob" | "operator" | "realm" | string;
  scope_key: string;
  active?: number;
  quarantined?: number;
  superseded?: number;
  tombstoned?: number;
  body_bytes?: number;
  floor_pressure?: boolean;
}

export interface MemoryPanelOverviewResult {
  scopes: MemoryScopeOverview[];
  realms: string[];
  floors?: { records?: number; bytes?: number };
}

/// A pending proposal awaiting a dream verdict (titles only — bodies stay
/// behind panel/record once committed). `tainted` is the durable
/// propose-time taint capture.
export interface MemoryProposalEntry {
  realm: string;
  proposal_id: string;
  scope_kind: string;
  scope_key: string;
  title?: string;
  kind?: string;
  author?: string;
  status?: string;
  created_at_ms?: number;
  tainted?: boolean;
}

export interface MemoryPanelProposalsResult {
  proposals: MemoryProposalEntry[];
  realms: string[];
}

/// Realm-wide injection-ledger row. `session_key` is null for build-surface
/// assembly rows (the build composes before a session key exists).
export interface MemoryLedgerEntry {
  realm: string;
  record_id: string;
  identity: string;
  session_key?: string | null;
  surface: "build" | "turn" | string;
  at_ms: number;
}

export interface MemoryPanelInjectionsResult {
  injections: MemoryLedgerEntry[];
  realms: string[];
}

/// A retired identity awaiting its exit-interview harvest dream.
export interface MemoryHarvestEntry {
  realm: string;
  identity: string;
  session_key?: string | null;
  cause?: string;
  retired_at_ms?: number;
}

export interface MemoryPanelHarvestsResult {
  harvests: MemoryHarvestEntry[];
  realms: string[];
}

/// Persisted DreamRun detail (steward verdict sheet): executed phases in
/// order with an outcome note each, the verdict counters, and loud skips.
export interface MemoryDreamRunDetail {
  phases?: Array<[string, string]>;
  verdicts?: Record<string, number>;
  skips?: string[];
}

/// One durable dream_runs row from panel/dream_runs. `detail` degrades to
/// the raw string when the stored JSON does not parse.
export interface MemoryDreamRunSheet {
  realm: string;
  run_id: string;
  partition?: string;
  started_at_ms?: number;
  completed_at_ms?: number;
  ops_committed?: number;
  detail?: MemoryDreamRunDetail | string;
}

export interface MemoryPanelDreamRunsResult {
  runs: MemoryDreamRunSheet[];
  realms: string[];
}

/// An open usage-audit verdict: a dead-weight/correction call awaiting
/// operator review ("memories you might want to correct").
export interface MemoryAuditVerdictEntry {
  realm: string;
  run_id: string;
  record_id: string;
  verdict?: string;
  rationale?: string;
  created_at_ms?: number;
}

export interface MemoryPanelAuditVerdictsResult {
  verdicts: MemoryAuditVerdictEntry[];
  realms: string[];
}

// ── WorkGraph panel (read + gated operator actions) ───────────────────────
// Tolerant mirrors of meerkat-workgraph's typed results, serialized verbatim
// by the `meta/workgraph/*` JSON-RPC methods. All fields optional on the
// client so the panel degrades gracefully across runtime versions.

export interface WorkGraphWireOwnerKey {
  kind?: string;
  id?: string;
}

export interface WorkGraphWireOwner {
  key?: WorkGraphWireOwnerKey;
  display_name?: string;
}

export interface WorkGraphWireItem {
  id?: string;
  realm_id?: string;
  namespace?: string;
  title?: string;
  description?: string;
  status?: string;
  priority?: string;
  labels?: string[];
  owner?: WorkGraphWireOwner;
  claim?: { owner?: WorkGraphWireOwner; claimed_at?: string; lease_expires_at?: string };
  revision?: number;
  due_at?: string;
  created_at?: string;
  updated_at?: string;
  terminal_at?: string;
  evidence_refs?: Array<{ kind?: string; id?: string; label?: string; summary?: string }>;
}

export interface WorkGraphWireEdge {
  kind?: string;
  from_id?: string;
  to_id?: string;
}

export interface WorkGraphWireBinding {
  binding_id?: string;
  work_ref?: { realm_id?: string; namespace?: string; item_id?: string };
  target?: { kind?: string; session_id?: string; owner_key?: WorkGraphWireOwnerKey };
  mode?: string;
  status?: { state?: string; until?: string };
  machine_state?: { revision?: number };
  created_at?: string;
  updated_at?: string;
}

export interface WorkGraphWireEvent {
  seq?: number;
  item_id?: string;
  kind?: string;
  at?: string;
  payload?: unknown;
}

/// `meta/workgraph/snapshot` result (WorkGraphSnapshot verbatim).
export interface WorkGraphSnapshotResult {
  realm_id?: string;
  namespace?: string;
  all_namespaces?: boolean;
  captured_at?: string;
  event_high_water_mark?: number;
  items?: WorkGraphWireItem[];
  edges?: WorkGraphWireEdge[];
  attention?: WorkGraphWireBinding[];
  ready_item_ids?: string[];
}

export interface WorkGraphEventsResult {
  events?: WorkGraphWireEvent[];
}

export interface ConsoleModulesResponse {
  modules?: unknown[];
}

export interface ConsoleGatingActionPayload extends GatingActionRequest {}

export interface ConsoleGatingActionResponse extends GatingActionResult {}

export interface ConsoleReplayUnavailablePayload extends ReplayUnavailableError {}

export interface ConsoleToolAccumulatorSnapshot extends ToolCallAccumulatorState {}
