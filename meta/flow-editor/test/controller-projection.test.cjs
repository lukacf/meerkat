const assert = require("node:assert/strict");

global.window = { __META_FLOW_CONTROLLER_TEST__: true };

// The bundle links @flow-editor-core (window.MetaFlowCore) with the
// controller.js residue exactly like the shipped artifact; build it with
// `node build.cjs --test-bundle`.
require("../.tmp/controller-under-test.cjs");

const controller = global.window.MetaFlowController;
assert.deepEqual(
  controller.authoringOperationFromIntent({
    intent: "settings.updateDeployField",
    field: "prompt",
    value: "Named field prompt.",
    selection: { kind: null, id: null },
  }),
  {
    type: "update_deploy_settings",
    field: "prompt",
    value: "Named field prompt.",
    selection: { kind: null, id: null },
  },
);
assert.deepEqual(
  controller.authoringOperationFromIntent({
    intent: "settings.updateMobField",
    field: "backendDefault",
    value: "external",
  }),
  {
    type: "update_mob_settings",
    field: "backendDefault",
    value: "external",
    selection: null,
  },
);
assert.deepEqual(
  controller.authoringOperationFromIntent({
    intent: "settings.editRoleWiring",
    action: "set_target",
    index: 0,
    value: "m_reviewer",
  }),
  {
    type: "update_role_wiring",
    action: "set_target",
    index: 0,
    field: undefined,
    value: "m_reviewer",
    selection: null,
  },
);
assert.deepEqual(controller.emptyAuthoringFlowState(), {
  name: "",
  steps: [],
});
const TEST_DEPLOY_VIEW_SCHEMA = {
  brand_label: "Meta · Flow Editor",
  flow_tab_label: "FLOW",
  agents_tab_label: "AGENTS",
  settings_tab_label: "SETTINGS",
  mob_status_title: "Active mob configuration",
  mob_file_label: "mob.toml",
  api_error_label: "api error",
  api_ready_label: "api ready",
  api_loading_label: "loading",
  deploy_prefix_label: "deploy:",
  flows_crumb_label: "mobs",
  crumb_separator: "/",
  switcher_view_all_label: "view all mobs",
  plan_trace_label: "PLAN TRACE",
  import_label: "IMPORT",
  validate_label: "VALIDATE",
  publish_label: "PUBLISH",
  deploy_plan_label: "DEPLOY PLAN",
  deploy_label: "DEPLOY",
  overflow_label: "MORE",
  theme_switch_prefix: "Switch to",
  theme_switch_suffix: "mode",
  dark_theme_label: "☾ dark",
  light_theme_label: "☀ light",
  basic_mode_title: "Basic Editor",
  basic_mode_label: "Basic",
  graph_mode_title: "Graph Editor",
  graph_mode_label: "Graph",
  validation_eyebrow: "VALIDATE · Meta",
  validation_passed_label: "passed",
  validation_warnings_label: "warnings",
  validation_blocking_label: "blocking",
  close_label: "×",
  plan_eyebrow: "DEPLOY PLAN",
  plan_unavailable_head: "DEPLOY TRACE UNAVAILABLE",
  plan_unavailable_body: "meta/mobpacks/deploy did not return plan_trace.",
  plan_first_label: "first",
  plan_step_label: "step",
  plan_previous_label: "‹",
  plan_next_label: "›",
};
const TEST_DEPLOY_VIEW = {
  brandLabel: "Meta · Flow Editor",
  flowTabLabel: "FLOW",
  agentsTabLabel: "AGENTS",
  settingsTabLabel: "SETTINGS",
  mobStatusTitle: "Active mob configuration",
  mobFileLabel: "mob.toml",
  apiErrorLabel: "api error",
  apiReadyLabel: "api ready",
  apiLoadingLabel: "loading",
  deployPrefixLabel: "deploy:",
  flowsCrumbLabel: "mobs",
  crumbSeparator: "/",
  switcherViewAllLabel: "view all mobs",
  planTraceLabel: "PLAN TRACE",
  importLabel: "IMPORT",
  validateLabel: "VALIDATE",
  publishLabel: "PUBLISH",
  deployPlanLabel: "DEPLOY PLAN",
  deployLabel: "DEPLOY",
  overflowLabel: "MORE",
  themeSwitchPrefix: "Switch to",
  themeSwitchSuffix: "mode",
  darkThemeLabel: "☾ dark",
  lightThemeLabel: "☀ light",
  basicModeTitle: "Basic Editor",
  basicModeLabel: "Basic",
  graphModeTitle: "Graph Editor",
  graphModeLabel: "Graph",
  validationEyebrow: "VALIDATE · Meta",
  validationPassedLabel: "passed",
  validationWarningsLabel: "warnings",
  validationBlockingLabel: "blocking",
  closeLabel: "×",
  planEyebrow: "DEPLOY PLAN",
  planUnavailableHead: "DEPLOY TRACE UNAVAILABLE",
  planUnavailableBody: "meta/mobpacks/deploy did not return plan_trace.",
  planFirstLabel: "first",
  planStepLabel: "step",
  planPreviousLabel: "‹",
  planNextLabel: "›",
};
const TEST_AGENT_ACCESS_VIEW_SCHEMA = {
  tool_invalid_error: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
  tool_empty_error: "Choose a tool first.",
  authoring_operation_unavailable_error: "Meta authoring operation API is unavailable",
  tool_title: "TOOL ACCESS",
  tool_hint: "Authority is calculated from this allowlist. Reviewed once here.",
  tool_missing_description: "—",
  tool_remove_label: "×",
  tool_add_select_placeholder: "+ add tool…",
  tool_source_label: "Configured tool source",
  tool_source_placeholder: "choose from Meta tool catalog",
  tool_add_button_label: "ADD",
  inline_skill_realm_id: "meta/editor-inline",
  inline_skill_realm_label: "This mobpack",
  inline_skill_realm_source: "meta/editor",
  inline_skill_source: "inline",
  inline_skill_default_description: "Inline Meta skill stored in this mobpack.",
  skill_default_description: "Meta skill",
  skill_selected_check_label: "✓",
  skill_remove_label: "×",
  skill_section_title: "SKILLS",
  skill_inline_cancel_label: "CANCEL",
  skill_inline_open_label: "+ INLINE",
  skill_hint: "Selected skills are baked into the mobpack. Browse a realm to add more.",
  skill_inline_label_placeholder: "mob.skill-name",
  skill_inline_content_rows: 4,
  skill_inline_content_placeholder: "Skill instructions stored as [skills.<id>] content",
  skill_inline_create_hint: "Creates an inline skill definition in this mobpack.",
  skill_inline_add_label: "ADD SKILL",
  skill_inline_error_fallback: "Could not create inline skill.",
  skill_inline_missing_label_error: "Inline skill id or label is required.",
  skill_inline_missing_content_error: "Inline skill content is required.",
  skill_inline_invalid_id_error: "Inline skill id or label must contain letters or numbers.",
  skill_no_realms_message: "Meta did not provide skill realms for this document.",
  skill_realm_label: "Realm",
  skill_default_realm_suffix: " · default",
  skill_unavailable_heading: "Unavailable in Meta skill realms:",
  skill_outside_realm_heading: "Selected from other realms:",
};
const TEST_AGENT_ACCESS_VIEW = {
  toolInvalidError: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
  toolEmptyError: "Choose a tool first.",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  toolTitle: "TOOL ACCESS",
  toolHint: "Authority is calculated from this allowlist. Reviewed once here.",
  toolMissingDescription: "—",
  toolRemoveLabel: "×",
  toolAddSelectPlaceholder: "+ add tool…",
  toolSourceLabel: "Configured tool source",
  toolSourcePlaceholder: "choose from Meta tool catalog",
  toolAddButtonLabel: "ADD",
  inlineSkillRealmId: "meta/editor-inline",
  inlineSkillRealmLabel: "This mobpack",
  inlineSkillRealmSource: "meta/editor",
  inlineSkillSource: "inline",
  inlineSkillDefaultDescription: "Inline Meta skill stored in this mobpack.",
  skillDefaultDescription: "Meta skill",
  skillSelectedCheckLabel: "✓",
  skillRemoveLabel: "×",
  skillSectionTitle: "SKILLS",
  skillInlineCancelLabel: "CANCEL",
  skillInlineOpenLabel: "+ INLINE",
  skillHint: "Selected skills are baked into the mobpack. Browse a realm to add more.",
  skillInlineLabelPlaceholder: "mob.skill-name",
  skillInlineContentRows: 4,
  skillInlineContentPlaceholder: "Skill instructions stored as [skills.<id>] content",
  skillInlineCreateHint: "Creates an inline skill definition in this mobpack.",
  skillInlineAddLabel: "ADD SKILL",
  skillInlineErrorFallback: "Could not create inline skill.",
  skillInlineMissingLabelError: "Inline skill id or label is required.",
  skillInlineMissingContentError: "Inline skill content is required.",
  skillInlineInvalidIdError: "Inline skill id or label must contain letters or numbers.",
  skillNoRealmsMessage: "Meta did not provide skill realms for this document.",
  skillRealmLabel: "Realm",
  skillDefaultRealmSuffix: " · default",
  skillUnavailableHeading: "Unavailable in Meta skill realms:",
  skillOutsideRealmHeading: "Selected from other realms:",
};
const TEST_SETTINGS_VIEW_SCHEMA = {
  option_separator: " · ",
  unsupported_label_separator: " — not in Meta ",
  unsupported_reason_prefix: "Unsupported by the Meta ",
  unsupported_reason_suffix: " contract.",
  canvas_title: "Canvas",
  edge_style_label: "Edges",
  edge_style_options: [{ value: "text", label: "Text" }, { value: "icons", label: "Icons" }, { value: "colored", label: "Color" }],
  density_label: "Density",
  density_options: [{ value: "compact", label: "Compact" }, { value: "comfortable", label: "Comfy" }],
  theme_title: "Theme",
  theme_mode_label: "Mode",
  theme_mode_options: [{ value: "light", label: "Light" }, { value: "dark", label: "Dark" }],
  mob_title: "Mob",
  orchestrator_label: "Orchestrator",
  profile_none_label: "none",
  auto_wire_label: "Auto wire",
  auto_wire_options: [{ value: "no", label: "No" }, { value: "yes", label: "Yes" }],
  role_wiring_label: "Role wiring",
  role_wiring_add_label: "+ rule",
  default_backend_label: "Default backend",
  external_base_label: "External base",
  external_base_placeholder: "http://127.0.0.1:9000",
  advanced_label: "Advanced",
  advanced_object_required_error: "object required",
  advanced_invalid_json_error: "invalid JSON",
  deploy_title: "Deploy",
  surface_label: "Surface",
  deploy_surface_contract_label: "deploy_settings.surfaces",
  deploy_surface_labels: { cli: "cli", rpc: "rpc" },
  trust_label: "Trust",
  trust_policy_contract_label: "deploy_settings.trust_policies",
  trust_policy_labels: { permissive: "permissive", strict: "strict" },
  model_label: "Model",
  model_default_label: "default",
  model_vendor_fallback: "provider",
  duration_label: "Duration",
  duration_placeholder: "30s",
  tool_calls_label: "Tool calls",
  tool_calls_min: 0,
  tool_calls_max: 999,
  tokens_label: "Tokens",
  tokens_min: 0,
  tokens_max: 200000,
  realm_label: "Realm",
  realm_options: [{ value: "isolated", label: "Isolated" }, { value: "shared", label: "Shared" }],
  realm_id_label: "Realm ID",
  realm_id_placeholder: "realm id",
  backend_label: "Backend",
  realm_backend_contract_label: "deploy_settings.realm_backends",
  realm_backend_labels: { jsonl: "jsonl", sqlite: "sqlite" },
  prompt_label: "Prompt",
  prompt_placeholder: "Deploy prompt",
  command_label: "Command",
  command_fallback: "--",
  inspector_title: "Inspector",
  inspector_layout_label: "Layout",
  inspector_layout_options: [{ value: "right", label: "Right" }, { value: "bottom", label: "Bottom" }, { value: "modal", label: "Modal" }],
};
const TEST_SETTINGS_VIEW = {
  optionSeparator: " · ",
  unsupportedLabelSeparator: " — not in Meta ",
  unsupportedReasonPrefix: "Unsupported by the Meta ",
  unsupportedReasonSuffix: " contract.",
  canvasTitle: "Canvas",
  edgeStyleLabel: "Edges",
  edgeStyleOptions: [{ value: "text", label: "Text" }, { value: "icons", label: "Icons" }, { value: "colored", label: "Color" }],
  densityLabel: "Density",
  densityOptions: [{ value: "compact", label: "Compact" }, { value: "comfortable", label: "Comfy" }],
  themeTitle: "Theme",
  themeModeLabel: "Mode",
  themeModeOptions: [{ value: "light", label: "Light" }, { value: "dark", label: "Dark" }],
  mobTitle: "Mob",
  orchestratorLabel: "Orchestrator",
  profileNoneLabel: "none",
  autoWireLabel: "Auto wire",
  autoWireOptions: [{ value: "no", label: "No" }, { value: "yes", label: "Yes" }],
  roleWiringLabel: "Role wiring",
  roleWiringAddLabel: "+ rule",
  defaultBackendLabel: "Default backend",
  externalBaseLabel: "External base",
  externalBasePlaceholder: "http://127.0.0.1:9000",
  advancedLabel: "Advanced",
  advancedObjectRequiredError: "object required",
  advancedInvalidJsonError: "invalid JSON",
  deployTitle: "Deploy",
  surfaceLabel: "Surface",
  deploySurfaceContractLabel: "deploy_settings.surfaces",
  deploySurfaceLabels: { cli: "cli", rpc: "rpc" },
  trustLabel: "Trust",
  trustPolicyContractLabel: "deploy_settings.trust_policies",
  trustPolicyLabels: { permissive: "permissive", strict: "strict" },
  modelLabel: "Model",
  modelDefaultLabel: "default",
  modelVendorFallback: "provider",
  durationLabel: "Duration",
  durationPlaceholder: "30s",
  toolCallsLabel: "Tool calls",
  toolCallsMin: 0,
  toolCallsMax: 999,
  tokensLabel: "Tokens",
  tokensMin: 0,
  tokensMax: 200000,
  realmLabel: "Realm",
  realmOptions: [{ value: "isolated", label: "Isolated" }, { value: "shared", label: "Shared" }],
  realmIdLabel: "Realm ID",
  realmIdPlaceholder: "realm id",
  backendLabel: "Backend",
  realmBackendContractLabel: "deploy_settings.realm_backends",
  realmBackendLabels: { jsonl: "jsonl", sqlite: "sqlite" },
  promptLabel: "Prompt",
  promptPlaceholder: "Deploy prompt",
  commandLabel: "Command",
  commandFallback: "--",
  inspectorTitle: "Inspector",
  inspectorLayoutLabel: "Layout",
  inspectorLayoutOptions: [{ value: "right", label: "Right" }, { value: "bottom", label: "Bottom" }, { value: "modal", label: "Modal" }],
};
const TEST_LAUNCH_VIEW_SCHEMA = {
  launch_title: "Launch mode",
  graph_launch_title: "LAUNCH MODE · this position",
  resume_session_label: "Bridge session",
  resume_session_placeholder: "session id",
  fork_source_label: "Fork from",
  fork_context_label: "Fork context",
  graph_fork_context_label: "Context",
  unsupported_label_separator: " — not in Meta ",
  unsupported_reason_prefix: "Unsupported by the Meta ",
  unsupported_reason_suffix: " contract.",
  launch_modes_contract_label: "launch_modes",
  fork_contexts_contract_label: "mob_definition.fork_contexts",
  launch_mode_labels: {
    Fresh: "Fresh — empty context",
    Resume: "Resume — existing bridge session",
    Fork: "Fork — copy context from another step",
  },
  fork_context_labels: {
    full_history: "full_history — entire transcript",
    last_messages: "last_messages — last N messages",
    FullHistory: "FullHistory — legacy alias for full_history",
  },
};
const TEST_LAUNCH_VIEW = {
  launchTitle: "Launch mode",
  graphLaunchTitle: "LAUNCH MODE · this position",
  resumeSessionLabel: "Bridge session",
  resumeSessionPlaceholder: "session id",
  forkSourceLabel: "Fork from",
  forkContextLabel: "Fork context",
  graphForkContextLabel: "Context",
  unsupportedLabelSeparator: " — not in Meta ",
  unsupportedReasonPrefix: "Unsupported by the Meta ",
  unsupportedReasonSuffix: " contract.",
  launchModesContractLabel: "launch_modes",
  forkContextsContractLabel: "mob_definition.fork_contexts",
  launchModeLabels: TEST_LAUNCH_VIEW_SCHEMA.launch_mode_labels,
  forkContextLabels: TEST_LAUNCH_VIEW_SCHEMA.fork_context_labels,
};
const TEST_CONDITION_VIEW_SCHEMA = {
  empty_value_label: "—",
  text_value_placeholder: "value",
};
const TEST_CONDITION_VIEW = {
  emptyValueLabel: "—",
  textValuePlaceholder: "value",
};
const TEST_ERROR_VIEW_SCHEMA = {
  critical_glyph: "!",
  generic_error_head: "Meta error",
  deploy_failed_head: "Deploy failed",
  deploy_plan_failed_head: "Deploy plan failed",
  deploy_error_meta: "meta/mobpacks/deploy",
  source_failed_head: "Source render failed",
  source_error_meta: "meta/mobpacks/source",
  validation_api_failed_head: "Meta API unavailable",
  rpc_error_meta: "/flow-editor/rpc",
  authoring_operation_failed_head: "Meta authoring operation failed",
  authoring_operation_meta: "Meta authoring",
  authoring_operation_fallback_heads: {
    graph_projection: "Meta graph projection failed",
    graph_sync: "Meta graph sync failed",
    graph_node_insert: "Meta graph node insert failed",
    draft_save: "Meta draft save failed",
  },
  authoring_operation_stale_error: "stale authoring operation",
  authoring_operation_missing_document_error: "Meta authoring operation did not return a document",
  export_failed_head: "Export failed",
  import_failed_head: "Import failed",
  boot_open_missing_head: "Mob not found",
  boot_open_missing_meta: "flow-editor?open",
  missing_editor_flow_head: "Imported mobpack is missing a Meta editor flow",
  missing_editor_flow_sub: "meta/mobpacks/import did not return document.flow.steps",
  missing_editor_flow_meta: "missing_editor_flow",
};
const TEST_ERROR_VIEW = {
  criticalGlyph: "!",
  genericErrorHead: "Meta error",
  deployFailedHead: "Deploy failed",
  deployPlanFailedHead: "Deploy plan failed",
  deployErrorMeta: "meta/mobpacks/deploy",
  sourceFailedHead: "Source render failed",
  sourceErrorMeta: "meta/mobpacks/source",
  validationApiFailedHead: "Meta API unavailable",
  rpcErrorMeta: "/flow-editor/rpc",
  authoringOperationFailedHead: "Meta authoring operation failed",
  authoringOperationMeta: "Meta authoring",
  authoringOperationFallbackHeads: {
    graph_projection: "Meta graph projection failed",
    graph_sync: "Meta graph sync failed",
    graph_node_insert: "Meta graph node insert failed",
    draft_save: "Meta draft save failed",
  },
  authoringOperationStaleError: "stale authoring operation",
  authoringOperationMissingDocumentError: "Meta authoring operation did not return a document",
  exportFailedHead: "Export failed",
  importFailedHead: "Import failed",
  bootOpenMissingHead: "Mob not found",
  bootOpenMissingMeta: "flow-editor?open",
  missingEditorFlowHead: "Imported mobpack is missing a Meta editor flow",
  missingEditorFlowSub: "meta/mobpacks/import did not return document.flow.steps",
  missingEditorFlowMeta: "missing_editor_flow",
};
const TEST_NEW_FLOW_VIEW_SCHEMA = {
  eyebrow_template: "NEW MOB",
  close_label: "close",
  name_label: "Mob name",
  name_placeholder: "review-mob",
  start_from_label: "Template",
  create_label: "CREATE MOB",
};
const TEST_NEW_FLOW_VIEW = {
  eyebrowTemplate: "NEW MOB",
  closeLabel: "close",
  nameLabel: "Mob name",
  namePlaceholder: "review-mob",
  startFromLabel: "Template",
  createLabel: "CREATE MOB",
};
const TEST_FLOW_REGISTRY_VIEW_SCHEMA = {
  eyebrow: "LIBRARY",
  title_singular_suffix: "mob",
  title_plural_suffix: "mobs",
  create_label: "+ NEW MOB",
  create_ready_title: "Create a Meta mob",
  create_unavailable_title: "Meta authoring contract unavailable",
  drafts_section_label: "Drafts",
  runtime_section_label: "Running",
  runtime_readonly_hint: "Runtime flows are read-only projections; open one to fork it into a draft.",
  empty_title: "No mobs yet",
  empty_text: "Create a mob to start authoring a deployable Meta flow.",
  updated_just_now_label: "just now",
  columns: [
    { key: "name", label: "NAME" },
    { key: "description", label: "DESCRIPTION" },
    { key: "updated", label: "UPDATED" },
    { key: "stage", label: "STAGE" },
  ],
};
const TEST_FLOW_REGISTRY_VIEW = {
  eyebrow: "LIBRARY",
  titleSingularSuffix: "mob",
  titlePluralSuffix: "mobs",
  createLabel: "+ NEW MOB",
  createReadyTitle: "Create a Meta mob",
  createUnavailableTitle: "Meta authoring contract unavailable",
  draftsSectionLabel: "Drafts",
  runtimeSectionLabel: "Running",
  runtimeReadonlyHint: "Runtime flows are read-only projections; open one to fork it into a draft.",
  emptyTitle: "No mobs yet",
  emptyText: "Create a mob to start authoring a deployable Meta flow.",
  updatedJustNowLabel: "just now",
  columns: [
    { key: "name", label: "NAME" },
    { key: "description", label: "DESCRIPTION" },
    { key: "updated", label: "UPDATED" },
    { key: "stage", label: "STAGE" },
  ],
};
const TEST_SCHEMA = {
  deploy_settings: {
    command: "rkat mob run",
    surfaces: ["cli"],
    defaults: {
      command: "rkat mob run",
      surface: "cli",
      trust_policy: "permissive",
      model: "",
      max_duration: "30s",
      max_tool_calls: 0,
      max_total_tokens: 64,
      isolated: true,
      realm: "",
      instance: "",
      realm_backend: "jsonl",
      context_root: "",
      state_root: "",
      user_config_root: "",
      prompt: "Reply with exactly OK.",
    },
  },
  mob_definition: {
    editor_deploy_view: TEST_DEPLOY_VIEW_SCHEMA,
    editor_agent_access_view: TEST_AGENT_ACCESS_VIEW_SCHEMA,
    editor_settings_view: TEST_SETTINGS_VIEW_SCHEMA,
    editor_launch_view: TEST_LAUNCH_VIEW_SCHEMA,
    editor_condition_view: TEST_CONDITION_VIEW_SCHEMA,
    editor_error_view: TEST_ERROR_VIEW_SCHEMA,
    editor_new_flow_view: TEST_NEW_FLOW_VIEW_SCHEMA,
    editor_flow_registry_view: TEST_FLOW_REGISTRY_VIEW_SCHEMA,
    mob_settings: {
      defaults: {
        orchestrator: "",
        autoWireOrchestrator: false,
        roleWiring: [],
        backendDefault: "session",
        externalAddressBase: "",
        advanced: {
          topology: null,
          supervisor: null,
          limits: null,
          spawnPolicy: null,
          eventRouter: null,
        },
      },
    },
  },
};
const testDeploySettings = () => controller.deployDefaultsFromSchema(TEST_SCHEMA);

assert.deepEqual(controller.deployDefaultsFromSchema(null), {
  command: "",
  surface: "",
  trustPolicy: "",
  model: "",
  maxDuration: "",
  maxToolCalls: null,
  maxTotalTokens: null,
  isolated: false,
  realm: "",
  instance: "",
  realmBackend: "",
  contextRoot: "",
  stateRoot: "",
  userConfigRoot: "",
  prompt: "",
});
assert.equal(controller.normalizeDeploySettings(controller.deployDefaultsFromSchema(null)).command, "");
assert.equal(controller.mobDefaultsFromSchema(null).backendDefault, "");
assert.deepEqual(controller.topRailState({
  contract: null,
  deploySettings: controller.deployDefaultsFromSchema(null),
  stage: "draft",
  view: "library",
  mobOpen: false,
  theme: "light",
  deployView: TEST_DEPLOY_VIEW,
}), {
  mobOpen: false,
  sectionTabs: [],
  brandLabel: "Meta · Flow Editor",
  mobStatusTitle: "Active mob configuration",
  mobFileLabel: "mob.toml",
  contractState: "loading",
  deployPrefixLabel: "deploy:",
  deployCommand: "",
  deploySurface: "",
  flowsCrumbLabel: "mobs",
  crumbSeparator: "/",
  planTraceLabel: "PLAN TRACE",
  importLabel: "IMPORT",
  validateLabel: "VALIDATE",
  publishLabel: "PUBLISH",
  deployPlanLabel: "DEPLOY PLAN",
  deployLabel: "DEPLOY",
  overflowLabel: "MORE",
  deployActionsDisabled: true,
  deployRunDisabled: true,
  themeToggleTitle: "Switch to dark mode",
  themeToggleLabel: "☀ light",
  basicModeTitle: "Basic Editor",
  basicModeLabel: "Basic",
  graphModeTitle: "Graph Editor",
  graphModeLabel: "Graph",
});
assert.deepEqual(controller.topRailState({
  contract: TEST_SCHEMA,
  deploySettings: testDeploySettings(),
  stage: "valid",
  view: "editor",
  mobOpen: true,
  theme: "dark",
  deployView: TEST_DEPLOY_VIEW,
}), {
  mobOpen: true,
  sectionTabs: [
    { target: "flow-tab", view: "editor", label: "FLOW", current: true },
    { target: "agents-tab", view: "agents", label: "AGENTS", current: false },
    { target: "settings-tab", view: "settings", label: "SETTINGS", current: false },
  ],
  brandLabel: "Meta · Flow Editor",
  mobStatusTitle: "Active mob configuration",
  mobFileLabel: "mob.toml",
  contractState: "api ready",
  deployPrefixLabel: "deploy:",
  deployCommand: "rkat mob run",
  deploySurface: "cli",
  flowsCrumbLabel: "mobs",
  crumbSeparator: "/",
  planTraceLabel: "PLAN TRACE",
  importLabel: "IMPORT",
  validateLabel: "VALIDATE",
  publishLabel: "PUBLISH",
  deployPlanLabel: "DEPLOY PLAN",
  deployLabel: "DEPLOY",
  overflowLabel: "MORE",
  deployActionsDisabled: false,
  deployRunDisabled: false,
  themeToggleTitle: "Switch to light mode",
  themeToggleLabel: "☾ dark",
  basicModeTitle: "Basic Editor",
  basicModeLabel: "Basic",
  graphModeTitle: "Graph Editor",
  graphModeLabel: "Graph",
});
assert.deepEqual(controller.topRailState({
  contract: TEST_SCHEMA,
  deploySettings: testDeploySettings(),
  stage: "draft",
  view: "settings",
  mobOpen: true,
  theme: "light",
  deployView: TEST_DEPLOY_VIEW,
}).sectionTabs.map((tab) => [tab.target, tab.view, tab.current]), [
  ["flow-tab", "editor", false],
  ["agents-tab", "agents", false],
  ["settings-tab", "settings", true],
]);
assert.equal(controller.topRailState({
  contract: { error: "schema unavailable", deploy_settings: { command: "rkat mob run", surfaces: ["cli"] } },
  deploySettings: { surface: "" },
  stage: "published",
  view: "editor",
  deployView: TEST_DEPLOY_VIEW,
}).contractState, "api error");
assert.equal(controller.topRailState({
  contract: TEST_SCHEMA,
  deploySettings: testDeploySettings(),
  stage: "valid",
  view: "editor",
  theme: "light",
  deployView: TEST_DEPLOY_VIEW,
  capabilities: { authoring_capabilities: { deploy_execute_allowed: false } },
}).deployRunDisabled, true);
// The rail is a real state machine over {library, editor (FLOW), agents,
// settings}: section targets resolve only while a mob is open, "library"
// always goes home, and there is no toggling.
assert.deepEqual(controller.topRailNavigationTransition("library", "flow-tab", { mobOpen: true }), { view: "editor" });
assert.deepEqual(controller.topRailNavigationTransition("editor", "agents-tab", { mobOpen: true }), { view: "agents" });
assert.deepEqual(controller.topRailNavigationTransition("agents", "settings-tab", { mobOpen: true }), { view: "settings" });
assert.deepEqual(controller.topRailNavigationTransition("settings", "flow-tab", { mobOpen: true }), { view: "editor" });
assert.deepEqual(controller.topRailNavigationTransition("editor", "flow-tab", { mobOpen: true }), { view: "editor" });
assert.equal(controller.topRailNavigationTransition("library", "flow-tab", { mobOpen: false }), null);
assert.equal(controller.topRailNavigationTransition("library", "agents-tab", { mobOpen: false }), null);
assert.equal(controller.topRailNavigationTransition("library", "settings-tab", { mobOpen: false }), null);
assert.deepEqual(controller.topRailNavigationTransition("editor", "library", { mobOpen: true }), { view: "library" });
assert.deepEqual(controller.topRailNavigationTransition("settings", "library", { mobOpen: true }), { view: "library" });
assert.equal(controller.topRailNavigationTransition("library", "library", { mobOpen: false }), null);
assert.equal(controller.topRailNavigationTransition("editor", "missing", { mobOpen: true }), null);
assert.equal(controller.topRailNavigationTransition("editor", "flows-tab", { mobOpen: true }), null);
assert.deepEqual(controller.editorModeTransition("basic"), { editorMode: "basic" });
assert.deepEqual(controller.editorModeTransition("advanced"), { editorMode: "advanced" });
assert.equal(controller.editorModeTransition("graph"), null);
assert.deepEqual(controller.themeToggleTransition("dark"), { field: "theme", value: "light" });
assert.deepEqual(controller.themeToggleTransition("light"), { field: "theme", value: "dark" });
assert.deepEqual(controller.themeToggleTransition(""), { field: "theme", value: "dark" });

// Deep-link boot intents parse once from location.search: ?open wins over
// ?intent, unknown intents land in the library, blank/whitespace open ids
// are no intent at all, and ?embedded=1 flags host-owned chrome everywhere.
assert.deepEqual(controller.bootIntentFromQuery("?open=f_review_mob"), { kind: "open", id: "f_review_mob", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery("?open=f_review_mob&embedded=1"), { kind: "open", id: "f_review_mob", embedded: true });
assert.deepEqual(controller.bootIntentFromQuery("?open=f_x&intent=new"), { kind: "open", id: "f_x", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery("?intent=new"), { kind: "new", template: "", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery("?intent=new&template=docs&embedded=1"), { kind: "new", template: "docs", embedded: true });
assert.deepEqual(controller.bootIntentFromQuery("?intent=edit"), { kind: "library", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery("?embedded=1"), { kind: "library", embedded: true });
assert.deepEqual(controller.bootIntentFromQuery("?embedded=true"), { kind: "library", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery("?open=%20%20"), { kind: "library", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery(""), { kind: "library", embedded: false });
assert.deepEqual(controller.bootIntentFromQuery(undefined), { kind: "library", embedded: false });

// Host integration payloads: protocol-typed, minimal ({ type, id, stage,
// ok }), and null for kinds outside the documented event set.
assert.deepEqual(controller.hostEventPayload("created", { id: "f_a", stage: "draft", ok: true }), { type: "meta-flow-editor:created", id: "f_a", stage: "draft", ok: true });
assert.deepEqual(controller.hostEventPayload("saved", { id: "f_a", stage: "valid", ok: true }), { type: "meta-flow-editor:saved", id: "f_a", stage: "valid", ok: true });
assert.deepEqual(controller.hostEventPayload("deployed", { id: "f_a", stage: "draft", ok: false }), { type: "meta-flow-editor:deployed", id: "f_a", stage: "draft", ok: false });
assert.deepEqual(controller.hostEventPayload("deployed", {}), { type: "meta-flow-editor:deployed", id: "", stage: "", ok: false });
assert.equal(controller.hostEventPayload("opened", { id: "f_a" }), null);
assert.equal(controller.hostEventPayload("", { id: "f_a" }), null);

assert.equal(controller.buildBlankDocument, undefined, "blank mobpack documents must come from Meta schema, not a local builder");

const schemaBlankMobpack = controller.blankMobpackFromCatalogs({
  blank_mobpack: {
    id: "blank",
    name: "Blank",
    source: "meta/blank-mobpack",
    trigger: "label · small-fix",
    version: "0.1.0",
    stage: "valid",
    document: {
      schema_version: "0.1.0",
      mob_id: "blank_mob",
      name: "blank-mob",
      flow: { name: "blank-mob", steps: [{ id: "work", type: "member", role: "m_worker" }] },
    },
    validation: { ok: true },
  },
});
assert.equal(schemaBlankMobpack.id, "blank");
assert.equal(schemaBlankMobpack.source, "meta/blank-mobpack");
assert.equal(controller.blankMobpackFromCatalogs({
  blank_mobpack: {
    id: "blank",
    name: "Blank",
    source: "meta/blank-mobpack",
    version: "0.1.0",
    stage: "valid",
    document: { mob_id: "blank_mob" },
    deployability: { deployable: true },
    provenance: { catalog: "meta/mobpacks/templates" },
  },
}).deployability.deployable, true);
assert.equal(controller.blankMobpackFromCatalogs({
  blank_mobpack: {
    source: "meta/blank-mobpack",
    document: { mob_id: "blank_mob" },
  },
}), null);
assert.equal(controller.blankMobpackFromCatalogs({
  blank_mobpack: {
    id: "blank",
    name: "Blank",
    source: "meta/blank-mobpack",
    document: { mob_id: "blank_mob" },
  },
}), null);

assert.deepEqual(controller.newFlowTemplateOptions([
  {
    id: "docs",
    name: "Docs",
    source: "meta://samples/docs",
    stage: "valid",
    validation: { ok: true },
    document: { flow: { description: "Reviews documentation changes." } },
  },
  { id: "needs_source", name: "Needs Source", source: "meta://samples/needs-source", stage: "draft", validation: { ok: false } },
  { id: "", name: "Missing Id", validation: { ok: true } },
  { id: "missing_name", name: "", validation: { ok: true } },
], { canCreateBlank: false, blankTemplate: schemaBlankMobpack }), [
  {
    id: "blank",
    label: "Blank",
    sub: "meta/blank-mobpack",
    tier: "valid",
    disabled: true,
  },
  {
    id: "docs",
    label: "Docs",
    sub: "Reviews documentation changes.",
    tier: "valid",
    disabled: false,
  },
  {
    id: "needs_source",
    label: "Needs Source",
    sub: "meta://samples/needs-source",
    tier: "draft",
    disabled: false,
  },
]);
assert.equal(controller.newFlowTemplateOptions([], { canCreateBlank: true })[0].disabled, true);
assert.equal(controller.newFlowTemplateOptions([
  { id: "validated_no_stage", name: "Validated no stage", trigger: "label · no-stage", validation: { ok: true } },
], { canCreateBlank: true, blankTemplate: {
  id: "blank",
  name: "Blank",
  document: { mob_id: "blank_mob" },
  validation: { ok: true },
} })[0].tier, "");
assert.equal(controller.newFlowTemplateOptions([
  { id: "validated_no_stage", name: "Validated no stage", trigger: "label · no-stage", validation: { ok: true } },
], { canCreateBlank: true, blankTemplate: schemaBlankMobpack })[1].tier, "");
assert.equal(controller.newFlowTemplateOptions([], { canCreateBlank: true, blankTemplate: schemaBlankMobpack })[0].disabled, false);
assert.equal(controller.newFlowTemplateOptions([], {
  canCreateBlank: true,
  blankTemplate: {
    id: "blank",
    name: "Blank",
    document: { mob_id: "blank_mob" },
    validation: { ok: true },
  },
})[0].sub, "");
assert.deepEqual(controller.newFlowInitialState({ blankTemplate: schemaBlankMobpack }), {
  name: "",
  template: "blank",
});
assert.deepEqual(controller.newFlowInitialState({ blankTemplate: { id: "missing-document" } }), {
  name: "",
  template: "",
});
// Deep-link template preselect (?intent=new&template=<id>): known ids win,
// unknown ids fall back to the blank template.
assert.deepEqual(controller.newFlowInitialState({
  blankTemplate: schemaBlankMobpack,
  template: "docs",
  templates: [{ id: "docs", name: "Docs" }],
}), { name: "", template: "docs" });
assert.deepEqual(controller.newFlowInitialState({
  blankTemplate: schemaBlankMobpack,
  template: "missing",
  templates: [{ id: "docs", name: "Docs" }],
}), { name: "", template: "blank" });
assert.deepEqual(controller.newFlowInitialState({
  blankTemplate: schemaBlankMobpack,
  template: "blank",
  templates: [],
}), { name: "", template: "blank" });
assert.deepEqual(controller.newFlowModalState({
  name: "New Mob",
  template: "docs",
}, controller.newFlowTemplateOptions([
  {
    id: "docs",
    name: "Docs",
    source: "meta://samples/docs",
    stage: "valid",
    validation: { ok: true },
    document: { flow: { description: "Reviews documentation changes." } },
  },
], { canCreateBlank: false, blankTemplate: schemaBlankMobpack }), TEST_NEW_FLOW_VIEW), {
  eyebrow: "NEW MOB",
  closeLabel: "close",
  nameLabel: "Mob name",
  namePlaceholder: "review-mob",
  startFromLabel: "Template",
  createLabel: "CREATE MOB",
  name: "New Mob",
  template: "docs",
  options: [
    {
      id: "blank",
      label: "Blank",
      sub: "meta/blank-mobpack",
      tier: "valid",
      disabled: true,
      className: "template-card",
    },
    {
      id: "docs",
      label: "Docs",
      sub: "Reviews documentation changes.",
      tier: "valid",
      disabled: false,
      className: "template-card is-selected",
    },
  ],
  createDisabled: false,
});
assert.equal(controller.newFlowModalState({ name: "   ", template: "blank" }, [
  { id: "blank", disabled: false },
], TEST_NEW_FLOW_VIEW).createDisabled, true);
assert.equal(controller.newFlowModalState({ name: "Draft", template: "blank" }, [
  { id: "blank", disabled: true },
]).createDisabled, true);
assert.equal(controller.newFlowModalState({ name: "Draft", template: "" }, [
  { id: "blank", disabled: false },
], TEST_NEW_FLOW_VIEW).createDisabled, true);
assert.equal(controller.newFlowModalState({ name: "Draft", template: "ghost" }, [
  { id: "blank", disabled: false },
], TEST_NEW_FLOW_VIEW).createDisabled, true);
assert.equal(controller.newFlowModalState({ name: "Draft", template: "blank" }, [
  { id: "blank", disabled: false },
], TEST_NEW_FLOW_VIEW).createDisabled, false);
assert.deepEqual(controller.newFlowModalPatch({
  name: "Draft",
  template: "docs",
}, { name: "Renamed" }), {
  name: "Renamed",
  template: "docs",
});
assert.deepEqual(controller.newFlowModalFieldPatch({
  name: "Draft",
  template: "blank",
}, "template", "docs"), {
  name: "Draft",
  template: "docs",
});
assert.deepEqual(controller.newFlowModalFieldPatch({
  name: "Draft",
  template: "blank",
}, "trigger", "label · review"), {
  name: "Draft",
  template: "blank",
});
assert.deepEqual(controller.newFlowModalFieldPatch({
  name: "Draft",
  template: "blank",
}, "unsupported", "ignored"), {
  name: "Draft",
  template: "blank",
});
assert.equal(controller.newFlowModalStepPatch, undefined, "single-step new-mob modal has no step patch");
assert.deepEqual(controller.newFlowModalCreateSpec({
  name: "Draft",
  template: "blank",
}), {
  name: "Draft",
  template: "blank",
});
const REGISTRY_NOW_UNIX_MS = 1750000000000;
assert.deepEqual(controller.flowRegistryViewState([
  {
    id: "f_existing",
    name: "Existing",
    stage: "valid",
    updated_at_unix_ms: REGISTRY_NOW_UNIX_MS - (5 * 60 * 1000),
    document: { flow: { description: "Document description wins." } },
    description: "Row description loses.",
  },
  { id: "f_draft", name: "Draft", stage: "" },
], "f_existing", { canCreate: false, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS }), {
  eyebrow: "LIBRARY",
  title: "2 mobs",
  createLabel: "+ NEW MOB",
  createDisabled: true,
  createTitle: "Meta authoring contract unavailable",
  columns: [
    { key: "name", label: "NAME" },
    { key: "description", label: "DESCRIPTION" },
    { key: "updated", label: "UPDATED" },
    { key: "stage", label: "STAGE" },
  ],
  empty: null,
  sections: [{
    key: "drafts",
    label: "Drafts",
    hint: "",
    rows: [
      {
        id: "f_existing",
        className: "flows-list__row is-current",
        name: "Existing",
        description: "Document description wins.",
        updated: "5m ago",
        stage: "valid",
      },
      {
        id: "f_draft",
        className: "flows-list__row",
        name: "Draft",
        description: "",
        updated: "",
        stage: "draft",
      },
    ],
  }],
});
assert.deepEqual(controller.flowRegistryViewState([
  { id: "only", name: "One", description: "Row description fallback.", updated_at_unix_ms: REGISTRY_NOW_UNIX_MS - 30000 },
], "", { canCreate: true, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS }), {
  eyebrow: "LIBRARY",
  title: "1 mob",
  createLabel: "+ NEW MOB",
  createDisabled: false,
  createTitle: "Create a Meta mob",
  columns: [
    { key: "name", label: "NAME" },
    { key: "description", label: "DESCRIPTION" },
    { key: "updated", label: "UPDATED" },
    { key: "stage", label: "STAGE" },
  ],
  empty: null,
  sections: [{
    key: "drafts",
    label: "Drafts",
    hint: "",
    rows: [{
      id: "only",
      className: "flows-list__row",
      name: "One",
      description: "Row description fallback.",
      updated: "just now",
      stage: "draft",
    }],
  }],
});
// Relative "updated" buckets: caller-supplied clock, deterministic labels.
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - 30000, REGISTRY_NOW_UNIX_MS, "just now"), "just now");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - 59999, REGISTRY_NOW_UNIX_MS, "just now"), "just now");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - 60000, REGISTRY_NOW_UNIX_MS, "just now"), "1m ago");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - (59 * 60 * 1000), REGISTRY_NOW_UNIX_MS, "just now"), "59m ago");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - (60 * 60 * 1000), REGISTRY_NOW_UNIX_MS, "just now"), "1h ago");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - (23 * 60 * 60 * 1000), REGISTRY_NOW_UNIX_MS, "just now"), "23h ago");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - (24 * 60 * 60 * 1000), REGISTRY_NOW_UNIX_MS, "just now"), "1d ago");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - (9 * 24 * 60 * 60 * 1000), REGISTRY_NOW_UNIX_MS, "just now"), "9d ago");
assert.equal(controller.flowRegistryUpdatedLabel(0, REGISTRY_NOW_UNIX_MS, "just now"), "");
assert.equal(controller.flowRegistryUpdatedLabel(undefined, REGISTRY_NOW_UNIX_MS, "just now"), "");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS - 30000, 0, "just now"), "");
assert.equal(controller.flowRegistryUpdatedLabel(REGISTRY_NOW_UNIX_MS + 30000, REGISTRY_NOW_UNIX_MS, "just now"), "");
// Sections split: runtime_projection rows land in the read-only runtime section.
const splitRegistryView = controller.flowRegistryViewState([
  { id: "f_draft", name: "Draft", stage: "draft", updated_at_unix_ms: REGISTRY_NOW_UNIX_MS - (2 * 60 * 60 * 1000) },
  {
    id: "runtime_main",
    name: "Runtime Main",
    stage: "valid",
    runtime_projection: true,
    document: { flow: { description: "Runtime flow projection." } },
  },
], "runtime_main", { canCreate: true, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS });
assert.equal(splitRegistryView.title, "2 mobs");
assert.equal(splitRegistryView.empty, null);
assert.deepEqual(splitRegistryView.sections, [
  {
    key: "drafts",
    label: "Drafts",
    hint: "",
    rows: [{
      id: "f_draft",
      className: "flows-list__row",
      name: "Draft",
      description: "",
      updated: "2h ago",
      stage: "draft",
    }],
  },
  {
    key: "runtime",
    label: "Running",
    hint: "Runtime flows are read-only projections; open one to fork it into a draft.",
    rows: [{
      id: "runtime_main",
      className: "flows-list__row is-current",
      name: "Runtime Main",
      description: "Runtime flow projection.",
      updated: "",
      stage: "valid",
    }],
  },
]);
// Runtime-only registries omit the empty drafts section.
assert.deepEqual(controller.flowRegistryViewState([
  { id: "runtime_only", name: "Runtime Only", stage: "valid", runtime_projection: true },
], "", { canCreate: true, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS }).sections, [{
  key: "runtime",
  label: "Running",
  hint: "Runtime flows are read-only projections; open one to fork it into a draft.",
  rows: [{
    id: "runtime_only",
    className: "flows-list__row",
    name: "Runtime Only",
    description: "",
    updated: "",
    stage: "valid",
  }],
}]);
// Empty registry surfaces the empty state alone — no section chrome.
const emptyRegistryView = controller.flowRegistryViewState([], "", { canCreate: true, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS });
assert.equal(emptyRegistryView.title, "0 mobs");
assert.deepEqual(emptyRegistryView.empty, {
  title: "No mobs yet",
  text: "Create a mob to start authoring a deployable Meta flow.",
});
assert.deepEqual(emptyRegistryView.sections, []);
// Description derivation: document.flow.description, then row.description, then "".
const descriptionRows = controller.flowRegistryViewState([
  { id: "f_doc", name: "Doc", description: "Row fallback.", document: { flow: { description: "  Document first.  " } } },
  { id: "f_row", name: "Row", description: "  Row fallback.  ", document: { flow: { description: "   " } } },
  { id: "f_none", name: "None", document: { flow: {} } },
], "", { canCreate: true, flowRegistryView: TEST_FLOW_REGISTRY_VIEW, nowUnixMs: REGISTRY_NOW_UNIX_MS }).sections[0].rows;
assert.deepEqual(descriptionRows.map((row) => row.description), ["Document first.", "Row fallback.", ""]);

assert.equal(controller.flowDraftIdFromSpec({ name: "Blank Created" }, [{ id: "f_blank_created" }]), "f_blank_created_2");
assert.deepEqual(controller.authoringRpcMethodsFromSchema({
  commands: {
    schema: "meta/mobpacks/schema",
    catalogs: "meta/editor/catalogs",
    validate: "meta/editor/validate",
    source: "meta/editor/source",
    export: "meta/editor/export",
    import: "meta/editor/import",
    deploy_command: "meta/editor/deploy_command",
    deploy_rpc: "meta/editor/deploy",
  },
}), {
  schema: "meta/mobpacks/schema",
  catalogs: "meta/editor/catalogs",
  validate: "meta/editor/validate",
  source: "meta/editor/source",
  export: "meta/editor/export",
  import: "meta/editor/import",
  deployCommand: "meta/editor/deploy_command",
  deploy: "meta/editor/deploy",
});
assert.deepEqual(controller.authoringRpcMethodsFromSchema({ commands: { catalogs: "" } }), {});
const authoringOperations = controller.authoringOperationsFromSchema({
  operations: [
    {
      type: "rename_schema_field",
      plane: "agent",
      authority: "meta",
      requires: ["schema_id", "field_id", "new_name"],
      mutates: ["document.schemas", "document.flow", "document.edges"],
      projection_document_supported: true,
    },
    { type: "", plane: "ignored" },
    null,
  ],
});
assert.equal(authoringOperations.rename_schema_field.type, "rename_schema_field");
assert.equal(authoringOperations.rename_schema_field.plane, "agent");
assert.equal(authoringOperations.rename_schema_field.authority, "meta");
assert.equal(authoringOperations.rename_schema_field.projectionDocumentSupported, true);
assert.deepEqual(authoringOperations.rename_schema_field.requires, ["schema_id", "field_id", "new_name"]);
assert.deepEqual(authoringOperations.rename_schema_field.mutates, ["document.schemas", "document.flow", "document.edges"]);
assert.equal(controller.authoringOperationAvailability(authoringOperations, "rename_schema_field").supported, true);
assert.equal(controller.authoringOperationAvailability(authoringOperations, "missing_operation").supported, false);
assert.match(controller.authoringOperationAvailability(authoringOperations, "missing_operation").error, /missing_operation/);

assert.deepEqual(controller.modelCatalogFromCatalogs({
  models: [
    { id: "missing-label", vendor: "openai" },
    { id: "missing-vendor", label: "Missing Vendor" },
    { id: "openai/gpt-5.5", label: "GPT-5.5", vendor: "openai", profile: { temperature: 0 } },
  ],
}), [{
  id: "openai/gpt-5.5",
  label: "GPT-5.5",
  vendor: "openai",
  profile: { temperature: 0 },
}]);
assert.deepEqual(controller.modelCatalogFromCatalogs({
  models: [
    {
      id: "openai/gpt-5.5",
      label: "GPT-5.5",
      vendor: "openai",
      deployability: { deployable: true },
      provenance: { catalog: "meta/mobpacks/catalogs" },
    },
  ],
})[0].provenance, { catalog: "meta/mobpacks/catalogs" });

assert.deepEqual(controller.toolCatalogFromCatalogs({
  tool_config: [{
    id: "compat-only",
    label: "Compatibility Only",
    desc: "Must not hydrate from compatibility aliases.",
    kind: "runtime",
    source: "meerkat_mob::ToolConfig",
  }],
}), []);

assert.deepEqual(controller.toolCatalogFromCatalogs({
  tool_catalog: [
    { id: "missing-desc", label: "Missing Desc", kind: "runtime", source: "meerkat_mob::ToolConfig" },
    { id: "missing-source", label: "Missing Source", desc: "No source", kind: "runtime" },
    { id: "builtins", label: "builtins", desc: "Built-ins", kind: "runtime", source: "meerkat_mob::ToolConfig", tag_class: "is-runtime" },
  ],
}), [{
  id: "builtins",
  label: "builtins",
  desc: "Built-ins",
  kind: "runtime",
  source: "meerkat_mob::ToolConfig",
  tagClass: "is-runtime",
  raw: { id: "builtins", label: "builtins", desc: "Built-ins", kind: "runtime", source: "meerkat_mob::ToolConfig", tag_class: "is-runtime" },
}]);

const catalogBoot = { grid: { cellW: 10 }, cellXY: () => ({ x: 0, y: 0 }), template: { col: 1 } };
const emptyCatalogs = controller.emptyMetaCatalogs(catalogBoot);
assert.equal(emptyCatalogs.contractMeta.loaded, false);
assert.equal(emptyCatalogs.grid, catalogBoot.grid);
assert.equal(emptyCatalogs.cellXY, catalogBoot.cellXY);
assert.equal(emptyCatalogs.template, null);
assert.equal(emptyCatalogs.conditionView, null);
assert.equal(emptyCatalogs.errorView, null);
assert.deepEqual(controller.skillRealmsFromCatalogs({ skill_realms: "starter" }), []);
const schemaOnlyCatalogLeakState = controller.metaCatalogsFromSchema({
  schema_version: "mobpack/v1",
  media_type: "application/vnd.meta.mobpack+json",
  validation_source: "meta/mobpacks/schema",
  deploy_settings: {
    defaults: { command: "rkat mob run", surface: "cli" },
  },
  mob_definition: {
    mob_settings: { defaults: { backendDefault: "session", advanced: { topology: null } } },
  },
  models: [{ id: "schema-leak-model", label: "Schema Leak Model", vendor: "openai" }],
  tool_catalog: [{ id: "schema-leak-tool", label: "Schema Leak Tool", desc: "leaked", kind: "runtime", source: "schema" }],
  skill_realms: [{ id: "schema/leak", skills: [{ id: "mob.schema.leak" }] }],
  agent_definitions: [{
    id: "schema_leak_agent",
    name: "Schema Leak Agent",
    role: "schema_leak_agent",
    definitionType: "meta/profile-member",
    source: "schema",
    model: "gpt-5.5",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
  blank_mobpack: {
    id: "schema-leak-blank",
    name: "Schema Leak Blank",
    source: "schema",
    document: { flow: { name: "leak", steps: [] }, members: [] },
  },
}, catalogBoot);
assert.deepEqual(schemaOnlyCatalogLeakState.models, []);
assert.deepEqual(schemaOnlyCatalogLeakState.toolCatalog, []);
assert.deepEqual(schemaOnlyCatalogLeakState.skillRealms, []);
assert.deepEqual(schemaOnlyCatalogLeakState.agentDefinitions, []);
assert.equal(schemaOnlyCatalogLeakState.blankMobpack, null);
assert.equal(schemaOnlyCatalogLeakState.template, null);
const separateCatalogPayloadState = controller.metaCatalogsFromSchema({
  schema_version: "mobpack/v1",
  media_type: "application/vnd.meta.mobpack+json",
  validation_source: "meta/mobpacks/schema",
  deploy_settings: {
    defaults: { command: "rkat mob run", surface: "cli" },
  },
  mob_definition: {
    mob_settings: { defaults: { backendDefault: "session", advanced: { topology: null } } },
  },
}, catalogBoot, {
  models: [{ id: "openai/gpt-5.5", label: "GPT-5.5", vendor: "openai" }],
  tool_catalog: [{ id: "shell", label: "shell", desc: "Shell", kind: "runtime", source: "meerkat_mob::ToolConfig" }],
  skill_realms: [{ id: "meta/sample-mobpacks", source: "meta/sample-mobpack", skills: [{ id: "mob.review" }] }],
  agent_definitions: [{
    id: "sample_reviewer",
    name: "Reviewer",
    role: "reviewer",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_review_pr",
    sourceOrigin: "meta/sample-mobpack",
    model: "gpt-5.5",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
  blank_mobpack: {
    id: "blank",
    name: "Blank",
    version: "mobpack/v1",
    stage: "valid",
    trigger: "label · small-fix",
    source: "meta/blank-mobpack",
    document: {
      name: "Blank",
      mob_id: "blank",
      flow: { name: "Blank", steps: [{ id: "work", type: "member", role: "m_worker" }] },
      members: [{ id: "m_worker", role: "worker" }],
    },
  },
  runtime_flows: [{
    id: "runtime_loaded_main",
    name: "Runtime Loaded Main",
    source: "meta/runtime/flow_projection",
    stage: "valid",
    trigger: "runtime · main",
    document: {
      mob_id: "runtime_loaded",
      name: "Runtime Loaded Main",
      flow: { name: "main", steps: [] },
      members: [],
    },
    validation: { ok: true },
  }],
});
assert.equal(separateCatalogPayloadState.contractMeta.schemaVersion, "mobpack/v1");
assert.deepEqual(separateCatalogPayloadState.models.map((model) => model.id), ["openai/gpt-5.5"]);
assert.deepEqual(separateCatalogPayloadState.toolCatalog.map((tool) => tool.id), ["shell"]);
assert.deepEqual(separateCatalogPayloadState.skillRealms.map((realm) => realm.id), ["meta/sample-mobpacks"]);
assert.deepEqual(separateCatalogPayloadState.agentDefinitions.map((definition) => definition.id), ["sample_reviewer"]);
assert.deepEqual(separateCatalogPayloadState.runtimeFlows.map((row) => [row.id, row.source]), [
  ["runtime_loaded_main", "meta/runtime/flow_projection"],
]);
assert.equal(separateCatalogPayloadState.template.name, "Blank");

const hydratedContractAndCatalogFixture = {
  schema_version: "mobpack/v1",
  media_type: "application/vnd.meta.mobpack+json",
  validation_source: "meta/mobpacks/schema",
  deploy_settings: {
    defaults: {
      command: "rkat mob run",
      surface: "cli",
      trust_policy: "permissive",
      isolated: true,
    },
  },
  mob_definition: {
    runtime_modes: ["turn_driven"],
    editor_source_view: {
      drawer_eyebrow: "SOURCE · mob.toml",
      inline_title: "mob.toml",
      primary_source_path: "meta/mob.toml",
      loading_text: "rendering mob.toml from meta/mobpacks/source...",
      copy_label: "copy",
      close_label: "×",
    },
    editor_condition_view: TEST_CONDITION_VIEW_SCHEMA,
    editor_error_view: TEST_ERROR_VIEW_SCHEMA,
    editor_new_flow_view: TEST_NEW_FLOW_VIEW_SCHEMA,
    editor_flow_registry_view: TEST_FLOW_REGISTRY_VIEW_SCHEMA,
    editor_agent_view: {
      agents_heading: "AGENTS",
      schemas_heading: "SCHEMAS",
      add_schema_label: "+ new schema",
      add_agent_title: "Create an agent from a Meta profile-member definition.",
      add_agent_unavailable_title: "Meta schema contract has not provided agent definitions yet.",
      add_agent_unavailable_label: "agents unavailable",
      add_agent_placeholder_label: "+ new agent...",
      add_agent_error_prefix: "Agent definition unavailable: ",
      authoring_operation_unavailable_error: "Meta authoring operation API is unavailable",
      member_update_fallback_error: "Meta member update failed",
      tool_update_fallback_error: "Meta tool update failed",
      schema_assignment_fallback_error: "Meta schema assignment failed",
      schema_add_fallback_error: "add_schema failed",
      definition_catalog_title: "DEFINITION CATALOG",
      definition_catalog_empty: "No Meta profile-member definitions available.",
      definition_catalog_source_label: "source",
      definition_catalog_tools_label: "tools",
      definition_catalog_skills_label: "skills",
      member_sub_label_template: "{role} · {model}",
      member_placed_empty_label: "unplaced",
      member_placed_count_template: "×{count}",
      schema_field_singular_template: "{count} field",
      schema_field_plural_template: "{count} fields",
      schema_usage_label_template: "used by {count}",
      sidebar_sub_label_separator: " · ",
      empty_title: "AGENT LIBRARY",
      empty_lines: [
        "Select an agent or schema on the left.",
        "Agents are reusable across topologies. Edit one here and every placement updates.",
      ],
      missing_schema_label: "Schema not found.",
      missing_agent_label: "Agent not found.",
    },
    editor_agent_detail_view: {
      used_in_label: "used in",
      instance_singular: "instance",
      instance_plural: "instances",
      delete_label: "DELETE",
      delete_confirm_intro: "Delete agent",
      delete_confirm_placed_prefix: "It is placed in",
      delete_cancel_label: "CANCEL",
      cell_singular: "cell",
      cell_plural: "cells",
      delete_confirm_cells_suffix: "those nodes will be removed.",
      usage_title_prefix: "USED IN",
      empty_usage_hint: "Not yet placed in any cell. Switch to Graph to add.",
      agent_eyebrow_prefix: "AGENT",
      identity_title: "IDENTITY",
      profile_binding_label: "Profile binding",
      missing_profile_binding_label: "missing profile binding",
      realm_profile_label: "Realm profile",
      realm_profile_placeholder: "realm profile id",
      realm_profile_import_hint_fallback: "Realm profile refs are import-only for this editor. Mobpack archives must use inline profiles before validation/export.",
      realm_profile_title: "REALM PROFILE",
      realm_profile_reference_hint_before: "This imported member references",
      realm_profile_reference_hint_after_fallback: "from a target realm. Convert it to an inline profile before validating or exporting a deployable mobpack.",
      model_label: "Model",
      runtime_mode_label: "Runtime mode",
      runtime_section_title: "RUNTIME",
      missing_runtime_mode_label: "missing runtime mode",
      backend_label: "Backend",
      backend_definition_default_label: "definition default",
      inline_peer_notifications_label: "Inline peer notifications",
      inline_peer_notifications_placeholder: "runtime default",
      provider_params_label: "Provider params",
      provider_params_placeholder: '{"thinking_budget":4096}',
      provider_params_rows: 4,
      provider_params_invalid_json_label: "invalid JSON",
      provider_params_object_required_error: "provider_params must be a JSON object",
      system_prompt_title: "PEER DESCRIPTION",
      apply_skeleton_label: "APPLY SKELETON",
      apply_skeleton_title: "Insert the bundled starter template: a second-person mandate with operating rules ('You are <name>, a member of a Meerkat mob...'). It lands in this peer-facing description, not in the member's prompt, so rewrite it as a description of this member before exporting.",
      system_prompt_placeholder: "Describe this member's role for its peers. Exported as the profile peer_description and shown to other members through the peers tool and peer-added notifications; it is not this member's system prompt, which comes from the profile skills.",
      output_schema_title: "OUTPUT SCHEMA",
      schema_none_label: "— none —",
      schema_required_label: "req",
      edit_schema_label: "Edit schema →",
      empty_schema_hint: "No structured output. Agent returns free-form text.",
      source_title: "SOURCE",
      source_empty_hint: "Created in this editor.",
      source_definition_label: "Definition",
      source_mobpack_label: "Mobpack",
      source_origin_label: "Origin",
      source_document_path_label: "Member path",
      source_schema_path_label: "Schema path",
      source_tools_label: "Tool refs",
      source_skills_label: "Skill refs",
    },
    editor_agent_access_view: TEST_AGENT_ACCESS_VIEW_SCHEMA,
    editor_deploy_view: TEST_DEPLOY_VIEW_SCHEMA,
    editor_settings_view: TEST_SETTINGS_VIEW_SCHEMA,
    editor_launch_view: TEST_LAUNCH_VIEW_SCHEMA,
    editor_schema_view: {
      eyebrow: "OUTPUT SCHEMA",
      description_title: "DESCRIPTION",
      description_placeholder: "What is this artifact and when is it emitted?",
      fields_title_prefix: "FIELDS",
      fields_title_template: "{prefix} · {count}",
      add_field_label: "+ field",
      authoring_operation_unavailable_error: "Meta authoring operation API is unavailable",
      schema_operation_fallback_error: "Meta schema operation failed",
      field_add_fallback_error: "add_schema_field failed",
      header_labels: {
        name: "NAME",
        type: "TYPE",
        required: "REQ",
        description: "DESCRIPTION",
        action: "",
      },
      empty_fields_hint: "No fields yet. Click + field to start.",
      used_by_prefix: "USED BY",
      used_by_title_template: "{prefix} · {count}",
      usage_singular_template: "used by {count} agent",
      usage_plural_template: "used by {count} agents",
      empty_used_by_hint: "Not yet referenced by any agent.",
      delete_label: "DELETE",
      delete_blocked_title: "Unassign from agents first",
      field_name_placeholder: "field_name",
      field_description_placeholder: "—",
      field_remove_title: "Remove field",
      field_enum_label: "VALUES",
      field_enum_add_label: "+ value",
      field_enum_add_value: "value",
    },
    editor_basic_view: {
      start_label: "START",
      loop_badge: "LOOP",
      tips_title: "Tips",
      empty_panel_title: "Build your mob flow",
      empty_panel_subtitle_parts: [
        { kind: "text", text: "Pick a node to configure, or press " },
        { kind: "strong", text: "+" },
        { kind: "text", text: " to add a member turn or flow primitive. The result is a " },
        { kind: "code", text: "mob.toml" },
        { kind: "text", text: " flow." },
      ],
      source_toggle_label: "{ } mob.toml",
      authoring_operation_unavailable_error: "Meta authoring operation API is unavailable",
      authoring_operation_fallback_error: "Meta authoring operation failed",
      member_step_panel_title_fallback: "Member step",
      member_step_panel_sub_fallback: "Assign a member to run this step",
      member_step_member_label: "Member (profile)",
      member_step_member_placeholder: "— select member —",
      member_step_runtime_default_label: "runtime default",
      member_step_instruction_label: "message — instruction for this turn",
      member_step_instruction_placeholder: "e.g. Run the focused tests and report failures.",
      member_step_dispatch_label: "Dispatch mode",
      member_step_collection_label: "Collection policy",
      member_step_quorum_label: "Quorum",
      member_step_quorum_placeholder: "required",
      member_step_timeout_label: "Timeout (ms)",
      member_step_dependency_label: "depends_on mode",
      member_step_output_format_label: "Output format",
      member_step_allowed_tools_label: "Allowed tools",
      member_step_allowed_tools_empty_label: "Runtime profile default",
      member_step_blocked_tools_label: "Blocked tools",
      member_step_blocked_tools_empty_label: "No step-level blocks",
      member_step_schema_hint_prefix: "Emits ",
      member_step_schema_hint_tools_prefix: " · tools: ",
      member_step_schema_hint_empty_tools_label: "—",
      tool_scope_not_in_catalog_reason: "not in Meta tool catalog",
      tool_scope_not_enabled_reason: "not enabled on profile",
      tool_scope_tool_description_fallback: "Meta tool",
      tool_scope_remove_label: "×",
      tool_scope_select_member_placeholder: "select a member first",
      tool_scope_block_catalog_placeholder: "+ block Meta tool...",
      tool_scope_add_profile_placeholder: "+ add profile tool...",
      input_panel_icon: "▤",
      input_panel_title: "Input",
      input_panel_sub: "The task this mob is run with — its ingress",
      input_task_label: "Task",
      input_task_placeholder: "e.g. Fix the issue described below.",
      input_params_title_prefix: "INPUT PARAMS",
      input_add_param_label: "+ param",
      input_param_source_label: "Input params",
      input_param_header_labels: {
        name: "NAME",
        type: "TYPE",
        required: "REQ",
        description: "DESCRIPTION",
        action: "",
      },
      input_param_name_placeholder: "param_name",
      input_param_description_placeholder: "—",
      input_param_remove_title: "Remove param",
      input_param_enum_label: "VALUES",
      input_param_enum_add_label: "+ value",
      input_param_enum_add_value: "value",
      input_empty_params_parts: [
        { key: "prefix", text: "No params yet. Add one before branching on " },
        { key: "ref", text: "params.*", kind: "code" },
        { key: "suffix", text: "." },
      ],
      input_tips: [
        "Run with: rkat mob run <pack> --prompt \"<task>\" — or run_flow(input).",
        "Typed fields become the input schema the run is validated against.",
        "Event sources & schedules live outside the mobpack (e.g. fugue).",
      ],
      branch_panel_title: "Branch",
      branch_panel_sub: "Choose one downstream path by condition",
      parallel_panel_title: "Parallel",
      parallel_panel_sub: "fan_out to members, then fan_in and collect",
      branch_route_member_label: "Route member",
      parallel_join_member_label: "Join member",
      branch_controller_placeholder_label: "— direct Meta lanes —",
      branch_empty_controller_hint: "Without a selected profile, Meta conditions/parallel lanes attach directly to the first real member in each lane.",
      branch_condition_title: "Branch conditions",
      branch_condition_intro: "Read in order; the first match wins. Conditions read a member's structured output.",
      branch_condition_row_title_prefix: "Branch",
      branch_condition_empty_hint: "Add an upstream member with an output schema before configuring this branch.",
      branch_condition_source_placeholder: "— source —",
      branch_condition_field_placeholder: "— field —",
      branch_condition_no_schema_label: "(no schema)",
      branch_condition_preview_prefix: "when",
      branch_condition_preview_fallback: "…",
      branch_fallback_title: "Fallback",
      branch_fallback_hint: "If none match, the flow follows the fallback path; else it stops.",
      add_branch_label: "+ Add branch",
      add_parallel_branch_label: "+ Add parallel branch",
      parallel_dispatch_label: "Dispatch mode",
      parallel_collection_label: "Collection policy (fan_in)",
      parallel_quorum_label: "Quorum (N)",
      parallel_quorum_placeholder: "required",
      branch_dependency_label: "depends_on mode",
      repeat_panel_title: "Repeat until",
      repeat_panel_sub: "Loop the body, then evaluate the condition after each iteration",
      repeat_loop_id_label: "loop_id",
      repeat_loop_id_placeholder: "quality_loop",
      repeat_condition_title: "Until condition",
      repeat_condition_intro: "Evaluated on a body member's structured output after each pass. The loop exits when it holds.",
      repeat_empty_body_hint: "Add a member step inside the loop first — the condition reads its output schema.",
      repeat_member_placeholder_label: "— member —",
      repeat_condition_field_placeholder: "— field —",
      repeat_condition_no_schema_label: "(no schema)",
      repeat_preview_label: "until",
      repeat_preview_fallback: "…",
      repeat_iteration_input_label: "Iteration input — what each pass receives",
      repeat_max_iterations_label: "max_iterations",
      repeat_max_iterations_placeholder: "required",
      repeat_tips: [
        "The body is its own FrameSpec — add member steps inside the loop.",
        "The condition reads a member's typed output (e.g. reviewer.verdict == green).",
        "max_iterations bounds the loop so it always terminates.",
      ],
      repeat_canvas_while_label: "while",
      repeat_canvas_not_label: "not",
      repeat_canvas_missing_max_iterations_label: "missing max_iterations",
      repeat_canvas_max_iterations_prefix: "max ",
      repeat_canvas_loop_back_prefix: "↑ loop back · ",
      repeat_canvas_exit_prefix: "↓ exit when ",
      repeat_canvas_exit_fallback: "condition met",
      repeat_iteration_runtime_default_label: "runtime default",
      repeat_iteration_carry_label: "carries last output",
      repeat_iteration_reuse_unsupported_label: "unsupported: re-use input task",
      repeat_iteration_feeds_unsupported_prefix: "unsupported: feeds ",
      repeat_iteration_unsupported_prefix: "unsupported: ",
      add_step_title: "Add step",
      input_step_card_title: "Input",
      input_step_card_desc_fallback: "the task this mob is run with",
      branch_step_card_title: "Branch",
      branch_step_card_desc: "Mob picks the first matching path",
      parallel_step_card_title: "Parallel",
      parallel_step_card_desc_prefix: "fan-out → join · ",
      parallel_step_card_collection_fallback: "—",
      repeat_step_card_title: "Repeat until",
      repeat_step_card_desc_prefix: "until ",
      repeat_step_card_desc_fallback: "loop body until condition",
      member_step_card_title_fallback: "Select member",
      picker_kickoff_title: "Input",
      picker_kickoff_sub: "Every mob run starts from a single task input",
      picker_kickoff_hint: "This node is the mob's ingress — the task it's deployed/run with. Select it on the canvas to edit the task and any typed input fields.",
      picker_title: "Add step",
      picker_sub: "A flow node — a member turn or a flow primitive",
      picker_search_icon: "⌕",
      picker_search_placeholder: "Search members & primitives…",
      picker_members_label: "Mob members",
      picker_flow_label: "Flow",
      picker_empty_members_hint: "No members yet — define some in the Agents tab.",
      picker_new_badge_label: "NEW",
      adaptive_defaults: {
        type: "adaptive",
        flowmasterId: "",
        prompt: "",
        objectiveClass: "",
        resultSchema: "",
        profileTemplateIds: [],
        targetSurfaces: ["cli", "rpc"],
        allowInlineProfiles: false,
        limits: {
          maxDepth: 4,
          maxTotalDecisions: 12,
          maxRepairAttempts: 2,
          maxLayerFailures: 2,
          maxAttemptsPerLayer: 2,
          maxMembersPerLayer: 20,
          maxTotalSpawnedMembers: 60,
          maxActiveMembers: 20,
          maxRetainedLayerMobs: 4,
          maxWallClockMs: 1800000,
          maxAggregateTokens: 2000000,
          maxAggregateToolCalls: 500,
        },
      },
      adaptive_limit_rows: [
        { key: "maxDepth", toml_key: "max_depth", label: "Max depth", primary: true },
        { key: "maxTotalDecisions", toml_key: "max_total_decisions", label: "Max total decisions", primary: false },
        { key: "maxRepairAttempts", toml_key: "max_repair_attempts", label: "Max repair attempts", primary: false },
        { key: "maxLayerFailures", toml_key: "max_layer_failures", label: "Max layer failures", primary: false },
        { key: "maxAttemptsPerLayer", toml_key: "max_attempts_per_layer", label: "Max attempts / layer", primary: false },
        { key: "maxMembersPerLayer", toml_key: "max_members_per_layer", label: "Max members / layer", primary: true },
        { key: "maxTotalSpawnedMembers", toml_key: "max_total_spawned_members", label: "Max total members", primary: true },
        { key: "maxActiveMembers", toml_key: "max_active_members", label: "Max active members", primary: false },
        { key: "maxRetainedLayerMobs", toml_key: "max_retained_layer_mobs", label: "Max retained layer mobs", primary: false },
        { key: "maxWallClockMs", toml_key: "max_wall_clock_ms", label: "Max wall clock (ms)", primary: false },
        { key: "maxAggregateTokens", toml_key: "max_aggregate_tokens", label: "Max aggregate tokens", primary: false },
        { key: "maxAggregateToolCalls", toml_key: "max_aggregate_tool_calls", label: "Max aggregate tool calls", primary: false },
      ],
      adaptive_target_surfaces: ["cli", "rpc"],
      adaptive_panel_title: "Adaptive layer",
      adaptive_panel_sub: "A FlowMaster designs the next mob layer at runtime from prior results — dynamic width & depth. Each plan is validated, then run as a normal mob flow.",
      adaptive_flowmaster_label: "FlowMaster (planning agent)",
      adaptive_flowmaster_placeholder_label: "— select member —",
      adaptive_prompt_label: "Planning prompt",
      adaptive_prompt_placeholder: "How should the FlowMaster decide each layer?",
      adaptive_objective_label: "Objective class",
      adaptive_objective_placeholder: "code-review",
      adaptive_result_schema_label: "LayerResult schema",
      adaptive_result_schema_placeholder_label: "— LayerResult (untyped) —",
      adaptive_profiles_title: "Profile templates the layer may spawn",
      adaptive_profiles_hint: "No members are baked in — the FlowMaster picks counts & roles per layer from these shapes.",
      adaptive_profiles_agents_hint: "Edit member shapes in the Agents tab.",
      adaptive_profiles_empty_hint: "No members yet — define some in the Agents tab.",
      adaptive_surfaces_label: "Target surfaces",
      adaptive_inline_profiles_label: "Allow inline profiles",
      adaptive_limits_title: "Limits",
      adaptive_limits_advanced_label: "Advanced limits",
      adaptive_tips: [
        "Dynamic width: the layer is generated with the right number of members.",
        "Dynamic depth: the loop repeats until the FlowMaster returns Finish or limits hit.",
        "Every layer runs through MobMachine and emits a validated LayerResult.",
      ],
      adaptive_step_card_title: "Adaptive layer",
      adaptive_block_head_prefix: "ADAPTIVE LAYER · synthesized at runtime · max depth ",
      adaptive_block_flowmaster_kicker: "FLOWMASTER · plan",
      adaptive_block_flowmaster_title_fallback: "—",
      adaptive_block_flowmaster_desc: "Designs the next LayerPlan from prior results",
      adaptive_block_plan_connector: "LayerPlan ✓",
      adaptive_block_layer_kicker_prefix: "LAYERMOB · ≤ ",
      adaptive_block_layer_kicker_suffix: " members",
      adaptive_block_chip_suffix: "×N",
      adaptive_block_empty_profiles_label: "no profiles yet",
      adaptive_block_collect_connector_prefix: "collector → ",
      adaptive_block_collect_fallback: "LayerResult",
      adaptive_block_loop_back_prefix: "↑ loop until Finish · or depth ",
      adaptive_block_loop_back_suffix: " / budget",
      flow_primitive_rows: [
        {
          id: "repeat",
          glyph: "↻",
          tint: "member",
          label: "Repeat until",
          sub: "Loop a body of steps until a condition holds (max_iterations)",
        },
        {
          id: "branch",
          glyph: "⑂",
          tint: "member",
          label: "Branch",
          sub: "Pick one downstream path by condition (first match wins)",
        },
        {
          id: "parallel",
          glyph: "‖",
          tint: "member",
          label: "Parallel",
          sub: "fan_out to several members, then fan_in with a collection policy",
        },
        {
          id: "adaptive",
          glyph: "❖",
          tint: "member",
          label: "Adaptive layer",
          sub: "A FlowMaster designs & runs the next mob layer at runtime — dynamic width & depth",
          is_new: true,
        },
        {
          id: "subagent",
          glyph: "⬡",
          tint: "link",
          label: "Sub-mob",
          sub: "delegate to a nested mob, tested and packed on its own",
          disabled: true,
          disabled_reason: "Current Meta mob.toml flow semantics do not yet define deployable sub-mob nodes",
        },
        {
          id: "wait",
          glyph: "▮▮",
          tint: "link",
          label: "Wait for input",
          sub: "Pause the flow for required input before continuing",
          disabled: true,
          disabled_reason: "Current Meta mob.toml flow semantics do not yet define deployable wait-for-input nodes",
        },
      ],
    },
    editor_graph_view: {
      zoom_out_title: "Zoom out",
      fit_title: "Fit to view",
      zoom_in_title: "Zoom in",
      port_drag_title: "Drag to a member to connect",
      add_node_search_icon: "⌕",
      add_node_search_placeholder: "Add a node…",
      add_node_close_label: "✕",
      add_node_close_title: "Close",
      add_node_agents_label: "Agents",
      add_node_controls_label: "Control flow",
      add_node_terminals_label: "Terminals",
      add_node_empty_prefix: "No matches for “",
      add_node_empty_suffix: "”",
      add_node_jump_label: "+ New agent in Agents →",
      authoring_operation_unavailable_error: "Meta authoring operation API is unavailable",
      authoring_operation_fallback_error: "Meta graph operation failed",
      gate_palette_rows: [
        { id: "branch", glyph: "⑂", label: "Branch gate", meta: "conditional split" },
        { id: "fork", glyph: "‖", label: "Parallel fork", meta: "fan_out lanes" },
        { id: "join", glyph: "⋈", label: "Join gate", meta: "fan_in barrier" },
      ],
      terminal_palette_rows: [
        { id: "success", glyph: "■", label: "Success", meta: "visual-only; not deployable" },
        { id: "failed", glyph: "■", label: "Failed", meta: "visual-only; not deployable" },
        { id: "human", glyph: "□", label: "Needs human", meta: "visual-only; not deployable" },
      ],
      graph_gate_kind_labels: {
        branch: "branch — conditional split",
        fork: "fork — fan out in parallel",
        join: "join — wait for branches",
      },
      graph_terminal_kind_labels: {
        success: "success — done",
        failed: "failed — blocked",
        human: "human — needs human",
      },
      graph_frame_kind_labels: {
        Branch: "Branch — conditional flow frame",
        Parallel: "Parallel — concurrent flow frame",
        RepeatUntil: "RepeatUntil — bounded loop frame",
      },
      graph_edge_kind_labels: {
        next: "next — sequential handoff",
        fanout: "fanout — parallel sibling",
        cond: "cond — guarded branch",
      },
      inspector_delete_label: "DELETE",
      inspector_label_title: "LABEL",
      inspector_kind_title: "KIND",
      inspector_runtime_default_label: "runtime default",
      instance_eyebrow: "INSTANCE",
      instance_id_line_template: "{id} · cell ({col},{row})",
      instance_member_role_template: "MEMBER · {role}",
      instance_edit_member_label: "EDIT MEMBER →",
      instance_model_label: "model",
      instance_schema_label: "schema",
      instance_tools_label: "tools",
      instance_member_hint: "Editing the member updates every instance that uses it.",
      instance_position_title: "POSITION",
      instance_position_stage_label: "stage (col)",
      instance_position_slot_label: "slot (row)",
      instance_output_title_template: "MEMBER OUTPUT · {schema}",
      instance_output_required_label: "req",
      instance_output_hint: "Defined on the member.",
      instance_output_open_member_label: "Open member →",
      gate_eyebrow_template: "GATE · {kind}",
      gate_id_line_template: "{id} · cell ({col},{row})",
      gate_quorum_incoming_template: "of {count} incoming",
      gate_member_option_template: "{name} · {role}",
      terminal_eyebrow_template: "TERMINAL · {kind}",
      terminal_id_line_template: "{id} · cell ({col},{row})",
      terminal_authoring_locked_title: "Visual terminal node",
      terminal_authoring_locked_hint: "Visual terminal nodes do not compile into deployable Meta mob.toml. Delete this node or replace it with a real member, branch, join, or loop flow shape.",
      adaptive_authoring_locked_title: "Adaptive layer node",
      adaptive_authoring_locked_hint: "Adaptive layers are authored in Basic mode — FlowMaster, prompt, profile templates, and limits are edited there. Deleting this node deletes the adaptive step from the flow.",
      edge_eyebrow_template: "EDGE · {kind}",
      edge_title_template: "{from} → {to}",
      edge_id_line_template: "{id}",
      edge_field_placeholder: "— field —",
      edge_field_no_schema_placeholder: "(no schema)",
      gate_collection_title: "COLLECTION POLICY",
      gate_join_member_label: "Join member",
      gate_join_member_placeholder: "— select member —",
      gate_join_member_hint: "Meta uses this real profile to resolve non-all fan-in.",
      gate_dispatch_title: "DISPATCH MODE",
      gate_dispatch_hint: "Exports as the Meta parallel flow dispatch mode.",
      gate_conditions_title: "CONDITIONS",
      gate_empty_branch_hint: "add outgoing edges, then configure each as a typed condition or fallback",
      gate_wiring_title: "WIRING",
      gate_incoming_label: "incoming",
      gate_outgoing_label: "outgoing",
      branch_condition_mode_condition_label: "condition",
      branch_condition_mode_fallback_label: "fallback",
      branch_condition_target_prefix: "→",
      graph_condition_target_missing_label: "?",
      graph_condition_owner_option_template: "{name}",
      graph_condition_field_option_template: "{name} · {type}",
      branch_input_param_source_label: "Input params",
      source_file_label: "mob.toml",
      source_file_aria_label: "Open mob.toml read-only source editor",
      source_file_glyph: "{ }",
      source_file_role_label: "source file",
      source_file_node_id: "source_mob_toml",
      source_file_node_kind: "source",
      source_file_node_col_offset: 0,
      source_file_node_row_offset: -1,
      source_file_activation_hash: "#meta-graph-source",
      source_file_activation_selector: ".node--source-file",
      branch_condition_field_placeholder: "— field —",
      branch_condition_no_options_hint: "add input params or an upstream schema field for this condition",
      edge_condition_title: "CONDITION",
      edge_no_condition_options_hint: "Add an upstream agent with an output schema before configuring this edge.",
      edge_owner_placeholder: "— member —",
      edge_from_title: "FROM",
      edge_to_title: "TO",
      edge_row_instance_label: "instance",
      edge_row_member_label: "member",
      edge_row_schema_label: "schema",
      edge_row_missing_value: "—",
      edge_terminal_member_value: "(terminal)",
    },
    editor_graph_template_view: {
      template_eyebrow: "TEMPLATE",
      summary_title: "SUMMARY",
      triggers_title: "TRIGGERS",
      trigger_labels_label: "labels",
      trigger_default_label: "default",
      default_yes_label: "yes",
      default_no_label: "no",
      summary_members_label: "members",
      summary_instances_label: "instances",
      summary_terminals_label: "terminals",
      summary_edges_label: "edges",
      summary_frames_label: "frames",
      summary_members_value_template: "{placed} placed / {total} in library",
      quick_start_title: "QUICK START",
      quick_start_rows: [
        [
          { kind: "text", text: "Click a " },
          { kind: "strong", text: "library member" },
          { kind: "text", text: " on the left to edit it." },
        ],
        [
          { kind: "text", text: "Click an " },
          { kind: "strong", text: "empty grid cell" },
          { kind: "text", text: " to place a member." },
        ],
        [
          { kind: "text", text: "Drag a node's " },
          { kind: "strong", text: "right port" },
          { kind: "text", text: " to wire it to another." },
        ],
        [{ kind: "text", text: "⌫ deletes the selected instance or edge." }],
      ],
    },
  },
  models: [{ id: "openai/gpt-5.5", label: "GPT-5.5", vendor: "openai" }],
  tool_catalog: [{ id: "builtins", label: "builtins", desc: "Built-ins", kind: "runtime", source: "meerkat_mob::ToolConfig", tag_class: "" }],
  skill_realms: [{ id: "meta/sample-mobpacks", source: "meta/sample-mobpack", skills: [{ id: "mob.workpad" }] }],
  agent_definitions: [{
    id: "sample_reviewer",
    name: "Reviewer",
    role: "reviewer",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_review_pr",
    sourceOrigin: "meta/sample-mobpack",
    model: "gpt-5.5",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
  blank_mobpack: {
    id: "blank",
    name: "Blank",
    version: "mobpack/v1",
    stage: "valid",
    trigger: "label · small-fix",
    source: "meta/blank-mobpack",
    document: {
      name: "Blank",
      mob_id: "blank",
      flow: { name: "Blank", steps: [{ id: "work", type: "member", role: "m_worker" }] },
      members: [{ id: "m_worker", role: "worker" }],
    },
  },
};
const hydratedCatalogs = controller.metaCatalogsFromSchema(hydratedContractAndCatalogFixture, catalogBoot, hydratedContractAndCatalogFixture);
assert.equal(hydratedCatalogs.contractMeta.loaded, true);
assert.equal(hydratedCatalogs.contractMeta.schemaVersion, "mobpack/v1");
assert.equal(hydratedCatalogs.deployDefaults.command, "rkat mob run");
assert.equal(hydratedCatalogs.mobDefinition.runtime_modes[0], "turn_driven");
assert.deepEqual(hydratedCatalogs.models.map((model) => model.id), ["openai/gpt-5.5"]);
assert.deepEqual(hydratedCatalogs.toolCatalog.map((tool) => tool.id), ["builtins"]);
assert.deepEqual(hydratedCatalogs.skillRealms.map((realm) => realm.id), ["meta/sample-mobpacks"]);
assert.deepEqual(hydratedCatalogs.agentDefinitions.map((definition) => definition.id), ["sample_reviewer"]);
assert.deepEqual(hydratedCatalogs.newFlowView, TEST_NEW_FLOW_VIEW);
assert.deepEqual(hydratedCatalogs.flowRegistryView, TEST_FLOW_REGISTRY_VIEW);
assert.deepEqual(hydratedCatalogs.sourceView, {
  drawerEyebrow: "SOURCE · mob.toml",
  inlineTitle: "mob.toml",
  primarySourcePath: "meta/mob.toml",
  loadingText: "rendering mob.toml from meta/mobpacks/source...",
  copyLabel: "copy",
  closeLabel: "×",
});
assert.deepEqual(hydratedCatalogs.conditionView, TEST_CONDITION_VIEW);
assert.deepEqual(hydratedCatalogs.errorView, TEST_ERROR_VIEW);
assert.deepEqual(hydratedCatalogs.agentView, {
  agentsHeading: "AGENTS",
  schemasHeading: "SCHEMAS",
  addSchemaLabel: "+ new schema",
  addAgentTitle: "Create an agent from a Meta profile-member definition.",
  addAgentUnavailableTitle: "Meta schema contract has not provided agent definitions yet.",
  addAgentUnavailableLabel: "agents unavailable",
  addAgentPlaceholderLabel: "+ new agent...",
  addAgentErrorPrefix: "Agent definition unavailable:",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  memberUpdateFallbackError: "Meta member update failed",
  toolUpdateFallbackError: "Meta tool update failed",
  schemaAssignmentFallbackError: "Meta schema assignment failed",
  schemaAddFallbackError: "add_schema failed",
  definitionCatalogTitle: "DEFINITION CATALOG",
  definitionCatalogEmpty: "No Meta profile-member definitions available.",
  definitionCatalogSourceLabel: "source",
  definitionCatalogToolsLabel: "tools",
  definitionCatalogSkillsLabel: "skills",
  memberSubLabelTemplate: "{role} · {model}",
  memberPlacedEmptyLabel: "unplaced",
  memberPlacedCountTemplate: "×{count}",
  schemaFieldSingularTemplate: "{count} field",
  schemaFieldPluralTemplate: "{count} fields",
  schemaUsageLabelTemplate: "used by {count}",
  sidebarSubLabelSeparator: " · ",
  emptyTitle: "AGENT LIBRARY",
  emptyLines: [
    "Select an agent or schema on the left.",
    "Agents are reusable across topologies. Edit one here and every placement updates.",
  ],
  missingSchemaLabel: "Schema not found.",
  missingAgentLabel: "Agent not found.",
});
assert.deepEqual(hydratedCatalogs.agentDetailView, {
  usedInLabel: "used in",
  instanceSingular: "instance",
  instancePlural: "instances",
  deleteLabel: "DELETE",
  deleteConfirmIntro: "Delete agent",
  deleteConfirmPlacedPrefix: "It is placed in",
  deleteCancelLabel: "CANCEL",
  cellSingular: "cell",
  cellPlural: "cells",
  deleteConfirmCellsSuffix: "those nodes will be removed.",
  usageTitlePrefix: "USED IN",
  emptyUsageHint: "Not yet placed in any cell. Switch to Graph to add.",
  agentEyebrowPrefix: "AGENT",
  identityTitle: "IDENTITY",
  profileBindingLabel: "Profile binding",
  missingProfileBindingLabel: "missing profile binding",
  realmProfileLabel: "Realm profile",
  realmProfilePlaceholder: "realm profile id",
  realmProfileImportHintFallback: "Realm profile refs are import-only for this editor. Mobpack archives must use inline profiles before validation/export.",
  realmProfileTitle: "REALM PROFILE",
  realmProfileReferenceHintBefore: "This imported member references",
  realmProfileReferenceHintAfterFallback: "from a target realm. Convert it to an inline profile before validating or exporting a deployable mobpack.",
  modelLabel: "Model",
  runtimeModeLabel: "Runtime mode",
  runtimeSectionTitle: "RUNTIME",
  missingRuntimeModeLabel: "missing runtime mode",
  backendLabel: "Backend",
  backendDefinitionDefaultLabel: "definition default",
  inlinePeerNotificationsLabel: "Inline peer notifications",
  inlinePeerNotificationsPlaceholder: "runtime default",
  providerParamsLabel: "Provider params",
  providerParamsPlaceholder: '{"thinking_budget":4096}',
  providerParamsRows: 4,
  providerParamsInvalidJsonLabel: "invalid JSON",
  providerParamsObjectRequiredError: "provider_params must be a JSON object",
  systemPromptTitle: "PEER DESCRIPTION",
  applySkeletonLabel: "APPLY SKELETON",
  applySkeletonTitle: "Insert the bundled starter template: a second-person mandate with operating rules ('You are <name>, a member of a Meerkat mob...'). It lands in this peer-facing description, not in the member's prompt, so rewrite it as a description of this member before exporting.",
  systemPromptPlaceholder: "Describe this member's role for its peers. Exported as the profile peer_description and shown to other members through the peers tool and peer-added notifications; it is not this member's system prompt, which comes from the profile skills.",
  outputSchemaTitle: "OUTPUT SCHEMA",
  schemaNoneLabel: "— none —",
  schemaRequiredLabel: "req",
  editSchemaLabel: "Edit schema →",
  emptySchemaHint: "No structured output. Agent returns free-form text.",
  sourceTitle: "SOURCE",
  sourceEmptyHint: "Created in this editor.",
  sourceDefinitionLabel: "Definition",
  sourceMobpackLabel: "Mobpack",
  sourceOriginLabel: "Origin",
  sourceDocumentPathLabel: "Member path",
  sourceSchemaPathLabel: "Schema path",
  sourceToolsLabel: "Tool refs",
  sourceSkillsLabel: "Skill refs",
});
assert.deepEqual(hydratedCatalogs.agentAccessView, TEST_AGENT_ACCESS_VIEW);
assert.deepEqual(hydratedCatalogs.deployView, TEST_DEPLOY_VIEW);
assert.deepEqual(hydratedCatalogs.settingsView, TEST_SETTINGS_VIEW);
assert.deepEqual(hydratedCatalogs.schemaView, {
  eyebrow: "OUTPUT SCHEMA",
  descriptionTitle: "DESCRIPTION",
  descriptionPlaceholder: "What is this artifact and when is it emitted?",
  fieldsTitlePrefix: "FIELDS",
  fieldsTitleTemplate: "{prefix} · {count}",
  addFieldLabel: "+ field",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  schemaOperationFallbackError: "Meta schema operation failed",
  fieldAddFallbackError: "add_schema_field failed",
  headerLabels: {
    name: "NAME",
    type: "TYPE",
    required: "REQ",
    description: "DESCRIPTION",
    action: "",
  },
  emptyFieldsHint: "No fields yet. Click + field to start.",
  usedByPrefix: "USED BY",
  usedByTitleTemplate: "{prefix} · {count}",
  usageSingularTemplate: "used by {count} agent",
  usagePluralTemplate: "used by {count} agents",
  emptyUsedByHint: "Not yet referenced by any agent.",
  deleteLabel: "DELETE",
  deleteBlockedTitle: "Unassign from agents first",
  fieldNamePlaceholder: "field_name",
  fieldDescriptionPlaceholder: "—",
  fieldRemoveTitle: "Remove field",
  fieldEnumLabel: "VALUES",
  fieldEnumAddLabel: "+ value",
  fieldEnumAddValue: "value",
});
assert.deepEqual(hydratedCatalogs.basicView, {
  startLabel: "START",
  loopBadge: "LOOP",
  tipsTitle: "Tips",
  emptyPanelTitle: "Build your mob flow",
  emptyPanelSubtitleParts: [
    { key: "text-0", kind: "text", text: "Pick a node to configure, or press " },
    { key: "strong-1", kind: "strong", text: "+" },
    { key: "text-2", kind: "text", text: " to add a member turn or flow primitive. The result is a " },
    { key: "code-3", kind: "code", text: "mob.toml" },
    { key: "text-4", kind: "text", text: " flow." },
  ],
  sourceToggleLabel: "{ } mob.toml",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  authoringOperationFallbackError: "Meta authoring operation failed",
  memberStepPanelTitleFallback: "Member step",
  memberStepPanelSubFallback: "Assign a member to run this step",
  memberStepMemberLabel: "Member (profile)",
  memberStepMemberPlaceholder: "— select member —",
  memberStepRuntimeDefaultLabel: "runtime default",
  memberStepInstructionLabel: "message — instruction for this turn",
  memberStepInstructionPlaceholder: "e.g. Run the focused tests and report failures.",
  memberStepDispatchLabel: "Dispatch mode",
  memberStepCollectionLabel: "Collection policy",
  memberStepQuorumLabel: "Quorum",
  memberStepQuorumPlaceholder: "required",
  memberStepTimeoutLabel: "Timeout (ms)",
  memberStepDependencyLabel: "depends_on mode",
  memberStepOutputFormatLabel: "Output format",
  memberStepAllowedToolsLabel: "Allowed tools",
  memberStepAllowedToolsEmptyLabel: "Runtime profile default",
  memberStepBlockedToolsLabel: "Blocked tools",
  memberStepBlockedToolsEmptyLabel: "No step-level blocks",
  memberStepSchemaHintPrefix: "Emits ",
  memberStepSchemaHintToolsPrefix: " · tools: ",
  memberStepSchemaHintEmptyToolsLabel: "—",
  toolScopeNotInCatalogReason: "not in Meta tool catalog",
  toolScopeNotEnabledReason: "not enabled on profile",
  toolScopeToolDescriptionFallback: "Meta tool",
  toolScopeRemoveLabel: "×",
  toolScopeSelectMemberPlaceholder: "select a member first",
  toolScopeBlockCatalogPlaceholder: "+ block Meta tool...",
  toolScopeAddProfilePlaceholder: "+ add profile tool...",
  inputPanelIcon: "▤",
  inputPanelTitle: "Input",
  inputPanelSub: "The task this mob is run with — its ingress",
  inputTaskLabel: "Task",
  inputTaskPlaceholder: "e.g. Fix the issue described below.",
  inputParamsTitlePrefix: "INPUT PARAMS",
  inputAddParamLabel: "+ param",
  inputParamSourceLabel: "Input params",
  inputParamHeaderLabels: {
    name: "NAME",
    type: "TYPE",
    required: "REQ",
    description: "DESCRIPTION",
    action: "",
  },
  inputParamNamePlaceholder: "param_name",
  inputParamDescriptionPlaceholder: "—",
  inputParamRemoveTitle: "Remove param",
  inputParamEnumLabel: "VALUES",
  inputParamEnumAddLabel: "+ value",
  inputParamEnumAddValue: "value",
  inputEmptyParamsParts: [
    { key: "prefix", kind: "text", text: "No params yet. Add one before branching on " },
    { key: "ref", kind: "code", text: "params.*" },
    { key: "suffix", kind: "text", text: "." },
  ],
  inputTips: [
    "Run with: rkat mob run <pack> --prompt \"<task>\" — or run_flow(input).",
    "Typed fields become the input schema the run is validated against.",
    "Event sources & schedules live outside the mobpack (e.g. fugue).",
  ],
  branchPanelTitle: "Branch",
  branchPanelSub: "Choose one downstream path by condition",
  parallelPanelTitle: "Parallel",
  parallelPanelSub: "fan_out to members, then fan_in and collect",
  branchRouteMemberLabel: "Route member",
  parallelJoinMemberLabel: "Join member",
  branchControllerPlaceholderLabel: "— direct Meta lanes —",
  branchEmptyControllerHint: "Without a selected profile, Meta conditions/parallel lanes attach directly to the first real member in each lane.",
  branchConditionTitle: "Branch conditions",
  branchConditionIntro: "Read in order; the first match wins. Conditions read a member's structured output.",
  branchConditionRowTitlePrefix: "Branch",
  branchConditionEmptyHint: "Add an upstream member with an output schema before configuring this branch.",
  branchConditionSourcePlaceholder: "— source —",
  branchConditionFieldPlaceholder: "— field —",
  branchConditionNoSchemaLabel: "(no schema)",
  branchConditionPreviewPrefix: "when",
  branchConditionPreviewFallback: "…",
  branchFallbackTitle: "Fallback",
  branchFallbackHint: "If none match, the flow follows the fallback path; else it stops.",
  addBranchLabel: "+ Add branch",
  addParallelBranchLabel: "+ Add parallel branch",
  parallelDispatchLabel: "Dispatch mode",
  parallelCollectionLabel: "Collection policy (fan_in)",
  parallelQuorumLabel: "Quorum (N)",
  parallelQuorumPlaceholder: "required",
  branchDependencyLabel: "depends_on mode",
  repeatPanelTitle: "Repeat until",
  repeatPanelSub: "Loop the body, then evaluate the condition after each iteration",
  repeatLoopIdLabel: "loop_id",
  repeatLoopIdPlaceholder: "quality_loop",
  repeatConditionTitle: "Until condition",
  repeatConditionIntro: "Evaluated on a body member's structured output after each pass. The loop exits when it holds.",
  repeatEmptyBodyHint: "Add a member step inside the loop first — the condition reads its output schema.",
  repeatMemberPlaceholderLabel: "— member —",
  repeatConditionFieldPlaceholder: "— field —",
  repeatConditionNoSchemaLabel: "(no schema)",
  repeatPreviewLabel: "until",
  repeatPreviewFallback: "…",
  repeatIterationInputLabel: "Iteration input — what each pass receives",
  repeatMaxIterationsLabel: "max_iterations",
  repeatMaxIterationsPlaceholder: "required",
  repeatTips: [
    "The body is its own FrameSpec — add member steps inside the loop.",
    "The condition reads a member's typed output (e.g. reviewer.verdict == green).",
    "max_iterations bounds the loop so it always terminates.",
  ],
  repeatCanvasWhileLabel: "while",
  repeatCanvasNotLabel: "not",
  repeatCanvasMissingMaxIterationsLabel: "missing max_iterations",
  repeatCanvasMaxIterationsPrefix: "max ",
  repeatCanvasLoopBackPrefix: "↑ loop back · ",
  repeatCanvasExitPrefix: "↓ exit when ",
  repeatCanvasExitFallback: "condition met",
  repeatIterationRuntimeDefaultLabel: "runtime default",
  repeatIterationCarryLabel: "carries last output",
  repeatIterationReuseUnsupportedLabel: "unsupported: re-use input task",
  repeatIterationFeedsUnsupportedPrefix: "unsupported: feeds ",
  repeatIterationUnsupportedPrefix: "unsupported: ",
  addStepTitle: "Add step",
  inputStepCardTitle: "Input",
  inputStepCardDescFallback: "the task this mob is run with",
  branchStepCardTitle: "Branch",
  branchStepCardDesc: "Mob picks the first matching path",
  parallelStepCardTitle: "Parallel",
  parallelStepCardDescPrefix: "fan-out → join · ",
  parallelStepCardCollectionFallback: "—",
  repeatStepCardTitle: "Repeat until",
  repeatStepCardDescPrefix: "until ",
  repeatStepCardDescFallback: "loop body until condition",
  memberStepCardTitleFallback: "Select member",
  pickerKickoffTitle: "Input",
  pickerKickoffSub: "Every mob run starts from a single task input",
  pickerKickoffHint: "This node is the mob's ingress — the task it's deployed/run with. Select it on the canvas to edit the task and any typed input fields.",
  pickerTitle: "Add step",
  pickerSub: "A flow node — a member turn or a flow primitive",
  pickerSearchIcon: "⌕",
  pickerSearchPlaceholder: "Search members & primitives…",
  pickerMembersLabel: "Mob members",
  pickerFlowLabel: "Flow",
  pickerEmptyMembersHint: "No members yet — define some in the Agents tab.",
  pickerNewBadgeLabel: "NEW",
  adaptiveDefaults: {
    type: "adaptive",
    flowmasterId: "",
    prompt: "",
    objectiveClass: "",
    resultSchema: "",
    profileTemplateIds: [],
    targetSurfaces: ["cli", "rpc"],
    allowInlineProfiles: false,
    limits: {
      maxDepth: 4,
      maxTotalDecisions: 12,
      maxRepairAttempts: 2,
      maxLayerFailures: 2,
      maxAttemptsPerLayer: 2,
      maxMembersPerLayer: 20,
      maxTotalSpawnedMembers: 60,
      maxActiveMembers: 20,
      maxRetainedLayerMobs: 4,
      maxWallClockMs: 1800000,
      maxAggregateTokens: 2000000,
      maxAggregateToolCalls: 500,
    },
  },
  adaptiveLimitRows: [
    { key: "maxDepth", tomlKey: "max_depth", label: "Max depth", primary: true },
    { key: "maxTotalDecisions", tomlKey: "max_total_decisions", label: "Max total decisions", primary: false },
    { key: "maxRepairAttempts", tomlKey: "max_repair_attempts", label: "Max repair attempts", primary: false },
    { key: "maxLayerFailures", tomlKey: "max_layer_failures", label: "Max layer failures", primary: false },
    { key: "maxAttemptsPerLayer", tomlKey: "max_attempts_per_layer", label: "Max attempts / layer", primary: false },
    { key: "maxMembersPerLayer", tomlKey: "max_members_per_layer", label: "Max members / layer", primary: true },
    { key: "maxTotalSpawnedMembers", tomlKey: "max_total_spawned_members", label: "Max total members", primary: true },
    { key: "maxActiveMembers", tomlKey: "max_active_members", label: "Max active members", primary: false },
    { key: "maxRetainedLayerMobs", tomlKey: "max_retained_layer_mobs", label: "Max retained layer mobs", primary: false },
    { key: "maxWallClockMs", tomlKey: "max_wall_clock_ms", label: "Max wall clock (ms)", primary: false },
    { key: "maxAggregateTokens", tomlKey: "max_aggregate_tokens", label: "Max aggregate tokens", primary: false },
    { key: "maxAggregateToolCalls", tomlKey: "max_aggregate_tool_calls", label: "Max aggregate tool calls", primary: false },
  ],
  adaptiveTargetSurfaces: ["cli", "rpc"],
  adaptivePanelTitle: "Adaptive layer",
  adaptivePanelSub: "A FlowMaster designs the next mob layer at runtime from prior results — dynamic width & depth. Each plan is validated, then run as a normal mob flow.",
  adaptiveFlowmasterLabel: "FlowMaster (planning agent)",
  adaptiveFlowmasterPlaceholderLabel: "— select member —",
  adaptivePromptLabel: "Planning prompt",
  adaptivePromptPlaceholder: "How should the FlowMaster decide each layer?",
  adaptiveObjectiveLabel: "Objective class",
  adaptiveObjectivePlaceholder: "code-review",
  adaptiveResultSchemaLabel: "LayerResult schema",
  adaptiveResultSchemaPlaceholderLabel: "— LayerResult (untyped) —",
  adaptiveProfilesTitle: "Profile templates the layer may spawn",
  adaptiveProfilesHint: "No members are baked in — the FlowMaster picks counts & roles per layer from these shapes.",
  adaptiveProfilesAgentsHint: "Edit member shapes in the Agents tab.",
  adaptiveProfilesEmptyHint: "No members yet — define some in the Agents tab.",
  adaptiveSurfacesLabel: "Target surfaces",
  adaptiveInlineProfilesLabel: "Allow inline profiles",
  adaptiveLimitsTitle: "Limits",
  adaptiveLimitsAdvancedLabel: "Advanced limits",
  adaptiveTips: [
    "Dynamic width: the layer is generated with the right number of members.",
    "Dynamic depth: the loop repeats until the FlowMaster returns Finish or limits hit.",
    "Every layer runs through MobMachine and emits a validated LayerResult.",
  ],
  adaptiveStepCardTitle: "Adaptive layer",
  adaptiveBlockHeadPrefix: "ADAPTIVE LAYER · synthesized at runtime · max depth ",
  adaptiveBlockFlowmasterKicker: "FLOWMASTER · plan",
  adaptiveBlockFlowmasterTitleFallback: "—",
  adaptiveBlockFlowmasterDesc: "Designs the next LayerPlan from prior results",
  adaptiveBlockPlanConnector: "LayerPlan ✓",
  adaptiveBlockLayerKickerPrefix: "LAYERMOB · ≤ ",
  adaptiveBlockLayerKickerSuffix: " members",
  adaptiveBlockChipSuffix: "×N",
  adaptiveBlockEmptyProfilesLabel: "no profiles yet",
  adaptiveBlockCollectConnectorPrefix: "collector → ",
  adaptiveBlockCollectFallback: "LayerResult",
  adaptiveBlockLoopBackPrefix: "↑ loop until Finish · or depth ",
  adaptiveBlockLoopBackSuffix: " / budget",
  flowPrimitiveRows: [
    {
      id: "repeat",
      glyph: "↻",
      tint: "member",
      label: "Repeat until",
      sub: "Loop a body of steps until a condition holds (max_iterations)",
      isNew: false,
      disabled: false,
      disabledReason: "",
    },
    {
      id: "branch",
      glyph: "⑂",
      tint: "member",
      label: "Branch",
      sub: "Pick one downstream path by condition (first match wins)",
      isNew: false,
      disabled: false,
      disabledReason: "",
    },
    {
      id: "parallel",
      glyph: "‖",
      tint: "member",
      label: "Parallel",
      sub: "fan_out to several members, then fan_in with a collection policy",
      isNew: false,
      disabled: false,
      disabledReason: "",
    },
    {
      id: "adaptive",
      glyph: "❖",
      tint: "member",
      label: "Adaptive layer",
      sub: "A FlowMaster designs & runs the next mob layer at runtime — dynamic width & depth",
      isNew: true,
      disabled: false,
      disabledReason: "",
    },
    {
      id: "subagent",
      glyph: "⬡",
      tint: "link",
      label: "Sub-mob",
      sub: "delegate to a nested mob, tested and packed on its own",
      isNew: false,
      disabled: true,
      disabledReason: "Current Meta mob.toml flow semantics do not yet define deployable sub-mob nodes",
    },
    {
      id: "wait",
      glyph: "▮▮",
      tint: "link",
      label: "Wait for input",
      sub: "Pause the flow for required input before continuing",
      isNew: false,
      disabled: true,
      disabledReason: "Current Meta mob.toml flow semantics do not yet define deployable wait-for-input nodes",
    },
  ],
});
assert.deepEqual(controller.basicEditorViewState(hydratedCatalogs.basicView), hydratedCatalogs.basicView);
assert.deepEqual(controller.basicEditorViewState(null), {
  startLabel: "",
  loopBadge: "",
  tipsTitle: "",
  emptyPanelTitle: "",
  emptyPanelSubtitleParts: [],
  sourceToggleLabel: "",
  authoringOperationUnavailableError: "",
  authoringOperationFallbackError: "",
  memberStepPanelTitleFallback: "",
  memberStepPanelSubFallback: "",
  memberStepMemberLabel: "",
  memberStepMemberPlaceholder: "",
  memberStepRuntimeDefaultLabel: "",
  memberStepInstructionLabel: "",
  memberStepInstructionPlaceholder: "",
  memberStepDispatchLabel: "",
  memberStepCollectionLabel: "",
  memberStepQuorumLabel: "",
  memberStepQuorumPlaceholder: "",
  memberStepTimeoutLabel: "",
  memberStepDependencyLabel: "",
  memberStepOutputFormatLabel: "",
  memberStepAllowedToolsLabel: "",
  memberStepAllowedToolsEmptyLabel: "",
  memberStepBlockedToolsLabel: "",
  memberStepBlockedToolsEmptyLabel: "",
  memberStepSchemaHintPrefix: "",
  memberStepSchemaHintToolsPrefix: "",
  memberStepSchemaHintEmptyToolsLabel: "",
  toolScopeNotInCatalogReason: "",
  toolScopeNotEnabledReason: "",
  toolScopeToolDescriptionFallback: "",
  toolScopeRemoveLabel: "",
  toolScopeSelectMemberPlaceholder: "",
  toolScopeBlockCatalogPlaceholder: "",
  toolScopeAddProfilePlaceholder: "",
  inputPanelIcon: "",
  inputPanelTitle: "",
  inputPanelSub: "",
  inputTaskLabel: "",
  inputTaskPlaceholder: "",
  inputParamsTitlePrefix: "",
  inputAddParamLabel: "",
  inputParamSourceLabel: "",
  inputParamHeaderLabels: {
    name: "",
    type: "",
    required: "",
    description: "",
    action: "",
  },
  inputParamNamePlaceholder: "",
  inputParamDescriptionPlaceholder: "",
  inputParamRemoveTitle: "",
  inputParamEnumLabel: "",
  inputParamEnumAddLabel: "",
  inputParamEnumAddValue: "",
  inputEmptyParamsParts: [],
  inputTips: [],
  branchPanelTitle: "",
  branchPanelSub: "",
  parallelPanelTitle: "",
  parallelPanelSub: "",
  branchRouteMemberLabel: "",
  parallelJoinMemberLabel: "",
  branchControllerPlaceholderLabel: "",
  branchEmptyControllerHint: "",
  branchConditionTitle: "",
  branchConditionIntro: "",
  branchConditionRowTitlePrefix: "",
  branchConditionEmptyHint: "",
  branchConditionSourcePlaceholder: "",
  branchConditionFieldPlaceholder: "",
  branchConditionNoSchemaLabel: "",
  branchConditionPreviewPrefix: "",
  branchConditionPreviewFallback: "",
  branchFallbackTitle: "",
  branchFallbackHint: "",
  addBranchLabel: "",
  addParallelBranchLabel: "",
  parallelDispatchLabel: "",
  parallelCollectionLabel: "",
  parallelQuorumLabel: "",
  parallelQuorumPlaceholder: "",
  branchDependencyLabel: "",
  repeatPanelTitle: "",
  repeatPanelSub: "",
  repeatLoopIdLabel: "",
  repeatLoopIdPlaceholder: "",
  repeatConditionTitle: "",
  repeatConditionIntro: "",
  repeatEmptyBodyHint: "",
  repeatMemberPlaceholderLabel: "",
  repeatConditionFieldPlaceholder: "",
  repeatConditionNoSchemaLabel: "",
  repeatPreviewLabel: "",
  repeatPreviewFallback: "",
  repeatIterationInputLabel: "",
  repeatMaxIterationsLabel: "",
  repeatMaxIterationsPlaceholder: "",
  repeatTips: [],
  repeatCanvasWhileLabel: "",
  repeatCanvasNotLabel: "",
  repeatCanvasMissingMaxIterationsLabel: "",
  repeatCanvasMaxIterationsPrefix: "",
  repeatCanvasLoopBackPrefix: "",
  repeatCanvasExitPrefix: "",
  repeatCanvasExitFallback: "",
  repeatIterationRuntimeDefaultLabel: "",
  repeatIterationCarryLabel: "",
  repeatIterationReuseUnsupportedLabel: "",
  repeatIterationFeedsUnsupportedPrefix: "",
  repeatIterationUnsupportedPrefix: "",
  addStepTitle: "",
  inputStepCardTitle: "",
  inputStepCardDescFallback: "",
  branchStepCardTitle: "",
  branchStepCardDesc: "",
  parallelStepCardTitle: "",
  parallelStepCardDescPrefix: "",
  parallelStepCardCollectionFallback: "",
  repeatStepCardTitle: "",
  repeatStepCardDescPrefix: "",
  repeatStepCardDescFallback: "",
  memberStepCardTitleFallback: "",
  pickerKickoffTitle: "",
  pickerKickoffSub: "",
  pickerKickoffHint: "",
  pickerTitle: "",
  pickerSub: "",
  pickerSearchIcon: "",
  pickerSearchPlaceholder: "",
  pickerMembersLabel: "",
  pickerFlowLabel: "",
  pickerEmptyMembersHint: "",
  pickerNewBadgeLabel: "",
  adaptiveDefaults: null,
  adaptiveLimitRows: [],
  adaptiveTargetSurfaces: [],
  adaptivePanelTitle: "",
  adaptivePanelSub: "",
  adaptiveFlowmasterLabel: "",
  adaptiveFlowmasterPlaceholderLabel: "",
  adaptivePromptLabel: "",
  adaptivePromptPlaceholder: "",
  adaptiveObjectiveLabel: "",
  adaptiveObjectivePlaceholder: "",
  adaptiveResultSchemaLabel: "",
  adaptiveResultSchemaPlaceholderLabel: "",
  adaptiveProfilesTitle: "",
  adaptiveProfilesHint: "",
  adaptiveProfilesAgentsHint: "",
  adaptiveProfilesEmptyHint: "",
  adaptiveSurfacesLabel: "",
  adaptiveInlineProfilesLabel: "",
  adaptiveLimitsTitle: "",
  adaptiveLimitsAdvancedLabel: "",
  adaptiveTips: [],
  adaptiveStepCardTitle: "",
  adaptiveBlockHeadPrefix: "",
  adaptiveBlockFlowmasterKicker: "",
  adaptiveBlockFlowmasterTitleFallback: "",
  adaptiveBlockFlowmasterDesc: "",
  adaptiveBlockPlanConnector: "",
  adaptiveBlockLayerKickerPrefix: "",
  adaptiveBlockLayerKickerSuffix: "",
  adaptiveBlockChipSuffix: "",
  adaptiveBlockEmptyProfilesLabel: "",
  adaptiveBlockCollectConnectorPrefix: "",
  adaptiveBlockCollectFallback: "",
  adaptiveBlockLoopBackPrefix: "",
  adaptiveBlockLoopBackSuffix: "",
  flowPrimitiveRows: [],
});
assert.deepEqual(hydratedCatalogs.launchView, TEST_LAUNCH_VIEW);
assert.deepEqual(hydratedCatalogs.graphView, {
  zoomOutTitle: "Zoom out",
  fitTitle: "Fit to view",
  zoomInTitle: "Zoom in",
  portDragTitle: "Drag to a member to connect",
  addNodeSearchIcon: "⌕",
  addNodeSearchPlaceholder: "Add a node…",
  addNodeCloseLabel: "✕",
  addNodeCloseTitle: "Close",
  addNodeAgentsLabel: "Agents",
  addNodeControlsLabel: "Control flow",
  addNodeTerminalsLabel: "Terminals",
  addNodeEmptyPrefix: "No matches for “",
  addNodeEmptySuffix: "”",
  addNodeJumpLabel: "+ New agent in Agents →",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  authoringOperationFallbackError: "Meta graph operation failed",
  gatePaletteRows: [
    { id: "branch", glyph: "⑂", label: "Branch gate", meta: "conditional split" },
    { id: "fork", glyph: "‖", label: "Parallel fork", meta: "fan_out lanes" },
    { id: "join", glyph: "⋈", label: "Join gate", meta: "fan_in barrier" },
  ],
  terminalPaletteRows: [
    { id: "success", glyph: "■", label: "Success", meta: "visual-only; not deployable" },
    { id: "failed", glyph: "■", label: "Failed", meta: "visual-only; not deployable" },
    { id: "human", glyph: "□", label: "Needs human", meta: "visual-only; not deployable" },
  ],
  gateKindLabels: {
    branch: "branch — conditional split",
    fork: "fork — fan out in parallel",
    join: "join — wait for branches",
  },
  terminalKindLabels: {
    success: "success — done",
    failed: "failed — blocked",
    human: "human — needs human",
  },
  frameKindLabels: {
    Branch: "Branch — conditional flow frame",
    Parallel: "Parallel — concurrent flow frame",
    RepeatUntil: "RepeatUntil — bounded loop frame",
  },
  edgeKindLabels: {
    next: "next — sequential handoff",
    fanout: "fanout — parallel sibling",
    cond: "cond — guarded branch",
  },
  inspectorDeleteLabel: "DELETE",
  inspectorLabelTitle: "LABEL",
  inspectorKindTitle: "KIND",
  inspectorRuntimeDefaultLabel: "runtime default",
  instanceEyebrow: "INSTANCE",
  instanceIdLineTemplate: "{id} · cell ({col},{row})",
  instanceMemberRoleTemplate: "MEMBER · {role}",
  instanceEditMemberLabel: "EDIT MEMBER →",
  instanceModelLabel: "model",
  instanceSchemaLabel: "schema",
  instanceToolsLabel: "tools",
  instanceMemberHint: "Editing the member updates every instance that uses it.",
  instancePositionTitle: "POSITION",
  instancePositionStageLabel: "stage (col)",
  instancePositionSlotLabel: "slot (row)",
  instanceOutputTitleTemplate: "MEMBER OUTPUT · {schema}",
  instanceOutputRequiredLabel: "req",
  instanceOutputHint: "Defined on the member.",
  instanceOutputOpenMemberLabel: "Open member →",
  gateEyebrowTemplate: "GATE · {kind}",
  gateIdLineTemplate: "{id} · cell ({col},{row})",
  gateQuorumIncomingTemplate: "of {count} incoming",
  gateMemberOptionTemplate: "{name} · {role}",
  terminalEyebrowTemplate: "TERMINAL · {kind}",
  terminalIdLineTemplate: "{id} · cell ({col},{row})",
  terminalAuthoringLockedTitle: "Visual terminal node",
  terminalAuthoringLockedHint: "Visual terminal nodes do not compile into deployable Meta mob.toml. Delete this node or replace it with a real member, branch, join, or loop flow shape.",
  adaptiveAuthoringLockedTitle: "Adaptive layer node",
  adaptiveAuthoringLockedHint: "Adaptive layers are authored in Basic mode — FlowMaster, prompt, profile templates, and limits are edited there. Deleting this node deletes the adaptive step from the flow.",
  edgeEyebrowTemplate: "EDGE · {kind}",
  edgeTitleTemplate: "{from} → {to}",
  edgeIdLineTemplate: "{id}",
  edgeFieldPlaceholder: "— field —",
  edgeFieldNoSchemaPlaceholder: "(no schema)",
  gateCollectionTitle: "COLLECTION POLICY",
  gateJoinMemberLabel: "Join member",
  gateJoinMemberPlaceholder: "— select member —",
  gateJoinMemberHint: "Meta uses this real profile to resolve non-all fan-in.",
  gateDispatchTitle: "DISPATCH MODE",
  gateDispatchHint: "Exports as the Meta parallel flow dispatch mode.",
  gateConditionsTitle: "CONDITIONS",
  gateEmptyBranchHint: "add outgoing edges, then configure each as a typed condition or fallback",
  gateWiringTitle: "WIRING",
  gateIncomingLabel: "incoming",
  gateOutgoingLabel: "outgoing",
  branchConditionModeConditionLabel: "condition",
  branchConditionModeFallbackLabel: "fallback",
  branchConditionTargetPrefix: "→",
  graphConditionTargetMissingLabel: "?",
  graphConditionOwnerOptionTemplate: "{name}",
  graphConditionFieldOptionTemplate: "{name} · {type}",
  graphInputParamSourceLabel: "Input params",
  sourceFileLabel: "mob.toml",
  sourceFileAriaLabel: "Open mob.toml read-only source editor",
  sourceFileGlyph: "{ }",
  sourceFileRoleLabel: "source file",
  sourceFileNodeId: "source_mob_toml",
  sourceFileNodeKind: "source",
  sourceFileNodeColOffset: 0,
  sourceFileNodeRowOffset: -1,
  sourceFileActivationHash: "#meta-graph-source",
  sourceFileActivationSelector: ".node--source-file",
  branchConditionFieldPlaceholder: "— field —",
  branchConditionNoOptionsHint: "add input params or an upstream schema field for this condition",
  edgeConditionTitle: "CONDITION",
  edgeNoConditionOptionsHint: "Add an upstream agent with an output schema before configuring this edge.",
  edgeOwnerPlaceholder: "— member —",
  edgeFromTitle: "FROM",
  edgeToTitle: "TO",
  edgeRowInstanceLabel: "instance",
  edgeRowMemberLabel: "member",
  edgeRowSchemaLabel: "schema",
  edgeRowMissingValue: "—",
  edgeTerminalMemberValue: "(terminal)",
});
assert.deepEqual(controller.graphCanvasViewState(hydratedCatalogs.graphView), hydratedCatalogs.graphView);
assert.deepEqual(controller.graphCanvasViewState(null), {
  zoomOutTitle: "",
  fitTitle: "",
  zoomInTitle: "",
  portDragTitle: "",
  addNodeSearchIcon: "",
  addNodeSearchPlaceholder: "",
  addNodeCloseLabel: "",
  addNodeCloseTitle: "",
  addNodeAgentsLabel: "",
  addNodeControlsLabel: "",
  addNodeTerminalsLabel: "",
  addNodeEmptyPrefix: "",
  addNodeEmptySuffix: "",
  addNodeJumpLabel: "",
  authoringOperationUnavailableError: "",
  authoringOperationFallbackError: "",
  gatePaletteRows: [],
  terminalPaletteRows: [],
  gateKindLabels: {},
  terminalKindLabels: {},
  frameKindLabels: {},
  edgeKindLabels: {},
  inspectorDeleteLabel: "",
  inspectorLabelTitle: "",
  inspectorKindTitle: "",
  inspectorRuntimeDefaultLabel: "",
  instanceEyebrow: "",
  instanceIdLineTemplate: "",
  instanceMemberRoleTemplate: "",
  instanceEditMemberLabel: "",
  instanceModelLabel: "",
  instanceSchemaLabel: "",
  instanceToolsLabel: "",
  instanceMemberHint: "",
  instancePositionTitle: "",
  instancePositionStageLabel: "",
  instancePositionSlotLabel: "",
  instanceOutputTitleTemplate: "",
  instanceOutputRequiredLabel: "",
  instanceOutputHint: "",
  instanceOutputOpenMemberLabel: "",
  gateEyebrowTemplate: "",
  gateIdLineTemplate: "",
  gateQuorumIncomingTemplate: "",
  gateMemberOptionTemplate: "",
  terminalEyebrowTemplate: "",
  terminalIdLineTemplate: "",
  terminalAuthoringLockedTitle: "",
  terminalAuthoringLockedHint: "",
  adaptiveAuthoringLockedTitle: "",
  adaptiveAuthoringLockedHint: "",
  edgeEyebrowTemplate: "",
  edgeTitleTemplate: "",
  edgeIdLineTemplate: "",
  edgeFieldPlaceholder: "",
  edgeFieldNoSchemaPlaceholder: "",
  gateCollectionTitle: "",
  gateJoinMemberLabel: "",
  gateJoinMemberPlaceholder: "",
  gateJoinMemberHint: "",
  gateDispatchTitle: "",
  gateDispatchHint: "",
  gateConditionsTitle: "",
  gateEmptyBranchHint: "",
  gateWiringTitle: "",
  gateIncomingLabel: "",
  gateOutgoingLabel: "",
  branchConditionModeConditionLabel: "",
  branchConditionModeFallbackLabel: "",
  branchConditionTargetPrefix: "",
  graphConditionTargetMissingLabel: "",
  graphConditionOwnerOptionTemplate: "",
  graphConditionFieldOptionTemplate: "",
  graphInputParamSourceLabel: "",
  sourceFileLabel: "",
  sourceFileAriaLabel: "",
  sourceFileGlyph: "",
  sourceFileRoleLabel: "",
  sourceFileNodeId: "",
  sourceFileNodeKind: "",
  sourceFileNodeColOffset: 0,
  sourceFileNodeRowOffset: 0,
  sourceFileActivationHash: "",
  sourceFileActivationSelector: "",
  branchConditionFieldPlaceholder: "",
  branchConditionNoOptionsHint: "",
  edgeConditionTitle: "",
  edgeNoConditionOptionsHint: "",
  edgeOwnerPlaceholder: "",
  edgeFromTitle: "",
  edgeToTitle: "",
  edgeRowInstanceLabel: "",
  edgeRowMemberLabel: "",
  edgeRowSchemaLabel: "",
  edgeRowMissingValue: "",
  edgeTerminalMemberValue: "",
});
assert.deepEqual(hydratedCatalogs.graphTemplateView, {
  templateEyebrow: "TEMPLATE",
  summaryTitle: "SUMMARY",
  triggersTitle: "TRIGGERS",
  triggerLabelsLabel: "labels",
  triggerDefaultLabel: "default",
  defaultYesLabel: "yes",
  defaultNoLabel: "no",
  summaryMembersLabel: "members",
  summaryInstancesLabel: "instances",
  summaryTerminalsLabel: "terminals",
  summaryEdgesLabel: "edges",
  summaryFramesLabel: "frames",
  summaryMembersValueTemplate: "{placed} placed / {total} in library",
  quickStartTitle: "QUICK START",
  quickStartRows: [
    {
      key: "quick-start-0",
      parts: [
        { key: "text-0", kind: "text", text: "Click a " },
        { key: "strong-1", kind: "strong", text: "library member" },
        { key: "text-2", kind: "text", text: " on the left to edit it." },
      ],
    },
    {
      key: "quick-start-1",
      parts: [
        { key: "text-0", kind: "text", text: "Click an " },
        { key: "strong-1", kind: "strong", text: "empty grid cell" },
        { key: "text-2", kind: "text", text: " to place a member." },
      ],
    },
    {
      key: "quick-start-2",
      parts: [
        { key: "text-0", kind: "text", text: "Drag a node's " },
        { key: "strong-1", kind: "strong", text: "right port" },
        { key: "text-2", kind: "text", text: " to wire it to another." },
      ],
    },
    {
      key: "quick-start-3",
      parts: [{ key: "text-0", kind: "text", text: "⌫ deletes the selected instance or edge." }],
    },
  ],
});
assert.equal(hydratedCatalogs.grid, catalogBoot.grid);
assert.deepEqual(hydratedCatalogs.template, {
  name: "Blank",
  repo: "meta/blank-mobpack",
  version: "mobpack/v1",
  triggers: { labels: ["label · small-fix"], default: false },
});

assert.deepEqual(controller.mergeSkillRealms([
  { id: "document", default: true, skills: [{ id: "mob.workpad", source: "inline" }, { id: "mob.review", source: "inline" }] },
], [
  { id: "document", default: true, skills: [{ id: "mob.workpad", source: "catalog" }, { id: "mob.tests", source: "path" }] },
  { id: "contract", default: true, skills: [{ id: "mob.review", source: "catalog" }, { id: "mob.docs", source: "inline" }] },
]), [
  { id: "document", default: true, skills: [{ id: "mob.workpad", source: "inline" }, { id: "mob.review", source: "inline" }, { id: "mob.tests", source: "path" }] },
  { id: "contract", default: false, skills: [{ id: "mob.docs", source: "inline" }] },
]);

assert.deepEqual(controller.memberToolAccessPatch(
  { tools: ["builtins"] },
  "shell",
  hydratedCatalogs.toolCatalog,
  hydratedCatalogs.agentAccessView,
), {
  ok: false,
  id: "",
  error: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
  patch: null,
});
assert.deepEqual(controller.memberToolAccessPatch(
  { tools: ["builtins"] },
  "builtins",
  hydratedCatalogs.toolCatalog,
  hydratedCatalogs.agentAccessView,
), { ok: true, id: "builtins", alreadySelected: true, patch: null });
assert.deepEqual(controller.memberToolAccessPatch(
  { tools: [] },
  "builtins",
  hydratedCatalogs.toolCatalog,
  hydratedCatalogs.agentAccessView,
), { ok: true, id: "builtins", alreadySelected: false, patch: { tools: ["builtins"] } });
assert.deepEqual(controller.memberToolAccessState(
  { tools: ["builtins", "missing"] },
  [
    { id: "builtins", label: "Built-ins", desc: "Built-in runtime tools" },
    { id: "shell", label: "Shell", desc: "Shell tool" },
  ],
  hydratedCatalogs.agentAccessView,
), {
  selectedTools: ["builtins", "missing"],
  title: "TOOL ACCESS",
  hint: "Authority is calculated from this allowlist. Reviewed once here.",
  rows: [
    {
      id: "builtins",
      name: "builtins",
      description: "Built-in runtime tools",
      unavailable: false,
      reason: "",
      runtimeAvailability: null,
      meta: { id: "builtins", label: "Built-ins", desc: "Built-in runtime tools" },
      className: "tool-row",
      removeLabel: "×",
    },
    {
      id: "missing",
      name: "missing",
      description: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
      unavailable: true,
      reason: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
      runtimeAvailability: null,
      meta: null,
      className: "tool-row tool-row--invalid",
      removeLabel: "×",
    },
  ],
  addableRows: [{
    id: "shell",
    value: "shell",
    label: "Shell",
    description: "Shell tool",
    optionLabel: "Shell — Shell tool",
    disabled: false,
    meta: { id: "shell", label: "Shell", desc: "Shell tool" },
  }],
  addSelectValue: "",
  addSelectPlaceholder: "+ add tool…",
  sourceLabel: "Configured tool source",
  sourcePlaceholder: "choose from Meta tool catalog",
  addButtonLabel: "ADD",
  emptyToolError: "Choose a tool first.",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
});
const runtimeUnavailableTools = [
  { id: "builtins", label: "Built-ins", desc: "Built-in runtime tools" },
  {
    id: "mob",
    label: "Mob",
    desc: "Mob management tools",
    runtimeAvailability: {
      available: false,
      state: "unavailable",
      reason: "current UnifiedRuntime does not expose the required Meta cross-mob control-plane methods",
    },
  },
];
assert.deepEqual(controller.memberToolAccessPatch(
  { tools: [] },
  "mob",
  runtimeUnavailableTools,
  hydratedCatalogs.agentAccessView,
), {
  ok: false,
  id: "",
  error: "Use a Meta-listed runtime tool or configured MCP/Rust source.",
  patch: null,
});
const runtimeUnavailableState = controller.memberToolAccessState(
  { tools: ["mob"] },
  runtimeUnavailableTools,
  hydratedCatalogs.agentAccessView,
);
assert.equal(runtimeUnavailableState.rows[0].unavailable, true);
assert.equal(
  runtimeUnavailableState.rows[0].reason,
  "current UnifiedRuntime does not expose the required Meta cross-mob control-plane methods",
);
assert.equal(runtimeUnavailableState.addableRows.find(row => row.id === "builtins").disabled, false);
assert.deepEqual(controller.memberToolRemovePatch(
  { tools: ["builtins", "shell", "shell"] },
  "shell",
), { ok: true, id: "shell", patch: { tools: ["builtins"] } });
assert.deepEqual(controller.memberSkillAccessState({
  member: { skills: ["mob.review", "mob.tests", "mob.missing"] },
  realmId: "docs",
  inlineOpen: true,
  skillRealms: [
    {
      id: "main",
      label: "Main",
      default: true,
      skills: [
        { id: "mob.review", desc: "Review skill" },
        { id: "mob.plan", source: "inline" },
      ],
    },
    {
      id: "docs",
      label: "Docs",
      skills: [
        { id: "mob.tests", path: "skills/tests.md" },
        { id: "mob.docs", source: "path" },
      ],
    },
  ],
  accessView: hydratedCatalogs.agentAccessView,
}), {
  sectionTitle: "SKILLS",
  inlineToggleLabel: "CANCEL",
  hint: "Selected skills are baked into the mobpack. Browse a realm to add more.",
  inlineLabelPlaceholder: "mob.skill-name",
  inlineContentRows: 4,
  inlineContentPlaceholder: "Skill instructions stored as [skills.<id>] content",
  inlineCreateHint: "Creates an inline skill definition in this mobpack.",
  inlineAddLabel: "ADD SKILL",
  inlineErrorFallback: "Could not create inline skill.",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  noRealmsMessage: "Meta did not provide skill realms for this document.",
  realmLabel: "Realm",
  hasRealms: true,
  realmId: "docs",
  realmOptions: [
    { id: "main", label: "Main · default" },
    { id: "docs", label: "Docs" },
  ],
  skillRows: [
    {
      id: "mob.tests",
      selected: true,
      className: "skill-row is-on",
      checkLabel: "✓",
      name: "mob.tests",
      desc: "skills/tests.md",
      skill: { id: "mob.tests", path: "skills/tests.md" },
    },
    {
      id: "mob.docs",
      selected: false,
      className: "skill-row",
      checkLabel: "",
      name: "mob.docs",
      desc: "path",
      skill: { id: "mob.docs", source: "path" },
    },
  ],
  selectedOutsideRealm: [{
    id: "mob.review",
    realmId: "main",
    realmLabel: "Main",
    className: "skill-chip",
    title: "Main",
    label: "mob.review",
    detail: "Main",
    removeLabel: "×",
  }],
  unavailableSelected: [{
    id: "mob.missing",
    className: "skill-chip is-invalid",
    label: "mob.missing",
    removeLabel: "×",
  }],
  unavailableHeading: "Unavailable in Meta skill realms:",
  outsideRealmHeading: "Selected from other realms:",
});
assert.deepEqual(controller.memberSkillAccessState({
  member: { skills: ["mob.review"] },
  realmId: "gone",
  skillRealms: [{ id: "main", label: "Main", skills: [{ id: "mob.review" }] }],
}).realmId, "main");
assert.deepEqual(controller.stepToolScopeState({
  member: { tools: ["builtins", "shell", "unknown"] },
  selected: ["builtins", "missing", "builtins"],
  mode: "member",
  toolCatalog: [
    { id: "builtins", label: "Built-ins", desc: "Built-in runtime tools" },
    { id: "shell", label: "Shell", desc: "Shell tool" },
    { id: "git", label: "Git", desc: "Git tool" },
  ],
  basicView: hydratedCatalogs.basicView,
}), {
  selectedTools: ["builtins", "missing"],
  addable: ["shell"],
  addableRows: [{
    id: "shell",
    value: "shell",
    label: "Shell",
    description: "Shell tool",
    optionLabel: "Shell — Shell tool",
    meta: { id: "shell", label: "Shell", desc: "Shell tool" },
  }],
  rows: [
    {
      id: "builtins",
      name: "builtins",
      meta: { id: "builtins", label: "Built-ins", desc: "Built-in runtime tools" },
      unavailable: false,
      reason: "",
      className: "tool-row",
      description: "Built-in runtime tools",
      removeLabel: "×",
    },
    {
      id: "missing",
      name: "missing",
      meta: null,
      unavailable: true,
      reason: "not enabled on profile",
      className: "tool-row tool-row--invalid",
      description: "not enabled on profile",
      removeLabel: "×",
    },
  ],
  addSelectValue: "",
  addSelectPlaceholder: "+ add profile tool...",
  disabled: false,
});
assert.deepEqual(controller.stepToolScopeAddPatch(
  ["builtins"],
  "shell",
  {
    member: { tools: ["builtins", "shell"] },
    mode: "member",
    field: "allowedTools",
    toolCatalog: [{ id: "builtins" }, { id: "shell" }, { id: "git" }],
  },
), { ok: true, id: "shell", patch: { allowedTools: ["builtins", "shell"] } });
assert.deepEqual(controller.stepToolScopeAddPatch(
  ["builtins"],
  "git",
  {
    member: { tools: ["builtins", "shell"] },
    mode: "member",
    field: "allowedTools",
    toolCatalog: [{ id: "builtins" }, { id: "shell" }, { id: "git" }],
  },
), { ok: false, id: "git", patch: null });
assert.deepEqual(controller.stepToolScopeAddPatch(
  [],
  "git",
  {
    member: { tools: ["builtins"] },
    mode: "catalog",
    field: "blockedTools",
    toolCatalog: [{ id: "builtins" }, { id: "shell" }, { id: "git" }],
  },
), { ok: true, id: "git", patch: { blockedTools: ["git"] } });
assert.deepEqual(controller.stepToolScopeRemovePatch(
  ["builtins", "shell", "shell"],
  "shell",
  { field: "allowedTools" },
), { ok: true, id: "shell", patch: { allowedTools: ["builtins"] } });

assert.deepEqual(controller.memberSkillTogglePatch(
  { skills: ["mob.workpad"] },
  "mob.review",
  [{ id: "main", skills: [{ id: "mob.review" }] }],
), { ok: true, id: "mob.review", selected: true, patch: { skills: ["mob.workpad", "mob.review"] } });
assert.deepEqual(controller.memberSkillTogglePatch(
  { skills: ["mob.workpad", "mob.review"] },
  "mob.review",
  [{ id: "main", skills: [{ id: "mob.review" }] }],
), { ok: true, id: "mob.review", selected: false, patch: { skills: ["mob.workpad"] } });
assert.deepEqual(controller.memberSkillTogglePatch(
  { skills: ["mob.workpad"] },
  "mob.fake",
  [{ id: "main", skills: [{ id: "mob.review" }] }],
), { ok: false, id: "mob.fake", patch: null });
assert.deepEqual(controller.memberSkillTogglePatch(
  { skills: ["mob.workpad", "mob.stale"] },
  "mob.stale",
  [{ id: "main", skills: [{ id: "mob.review" }] }],
), { ok: true, id: "mob.stale", selected: false, patch: { skills: ["mob.workpad"] } });
assert.deepEqual(controller.memberSkillRemovePatch(
  { skills: ["mob.workpad", "mob.review", "mob.review"] },
  "mob.review",
), { ok: true, id: "mob.review", patch: { skills: ["mob.workpad"] } });
const inlinePatch = controller.memberInlineSkillPatch(
  { skills: ["mob.workpad"] },
  [{ id: "meta/sample-mobpacks", skills: [{ id: "mob.workpad" }] }],
  { label: "Quality Gate", content: "Review and emit the QualityVerdict schema." },
  hydratedCatalogs.agentAccessView,
);
assert.equal(inlinePatch.id, "mob.quality.gate");
assert.equal(inlinePatch.realmId, "meta/editor-inline");
assert.deepEqual(inlinePatch.patch, { skills: ["mob.workpad", "mob.quality.gate"] });
assert.equal(inlinePatch.skillRealms[0].id, "meta/editor-inline");
assert.equal(inlinePatch.skillRealms[0].source, "meta/editor");
assert.equal(inlinePatch.skillRealms[0].skills[0].source, "inline");
assert.equal(inlinePatch.skillRealms[0].skills[0].content, "Review and emit the QualityVerdict schema.");
assert.equal(inlinePatch.skillRealms[0].skills[0].desc, "Inline Meta skill stored in this mobpack.");
assert.throws(
  () => controller.memberInlineSkillPatch(
    { skills: [] },
    [],
    { content: "Do the work." },
    hydratedCatalogs.agentAccessView,
  ),
  new RegExp(hydratedCatalogs.agentAccessView.skillInlineMissingLabelError.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
);
assert.throws(
  () => controller.memberInlineSkillPatch(
    { skills: [] },
    [],
    { label: "mob.empty" },
    hydratedCatalogs.agentAccessView,
  ),
  new RegExp(hydratedCatalogs.agentAccessView.skillInlineMissingContentError.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
);
assert.throws(
  () => controller.memberInlineSkillPatch(
    { skills: [] },
    [],
    { label: "!!!", content: "Do the work." },
    hydratedCatalogs.agentAccessView,
  ),
  new RegExp(hydratedCatalogs.agentAccessView.skillInlineInvalidIdError.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
);

const sampleRows = controller.sampleFlowsFromCatalogs({
  sample_mobpacks: [
    {
      id: "missing_source",
      name: "Missing source",
      document: { mob_id: "missing_source" },
    },
    {
      source: "meta/sample-mobpack",
      document: { mob_id: "" },
    },
    {
      source: "meta/sample-mobpack",
      document: { mob_id: "document_id_only", name: "Document Id Only" },
    },
    {
      id: "document_name_only",
      source: "meta/sample-mobpack",
      document: { mob_id: "document_name_only", name: "Document Name Only" },
    },
    {
      id: "schema_source",
      name: "Schema source",
      source: "meta/sample-mobpack",
      stage: "valid",
      document: { mob_id: "schema_source", schema_version: "0.1" },
      validation: { ok: true },
    },
    {
      id: "schema_source_with_trigger",
      name: "Schema source with trigger",
      source: "meta/imported",
      trigger: "imported/mob.toml",
      stage: "draft",
      document: { mob_id: "schema_source_with_trigger" },
    },
  ],
});
assert.deepEqual(sampleRows.map((row) => ({
  id: row.id,
  source: row.source,
  trigger: row.trigger,
  stage: row.stage,
})), [
  {
    id: "schema_source",
    source: "meta/sample-mobpack",
    trigger: "meta/sample-mobpack",
    stage: "valid",
  },
  {
    id: "schema_source_with_trigger",
    source: "meta/imported",
    trigger: "imported/mob.toml",
    stage: "draft",
  },
]);

const bootstrapProjection = controller.flowCatalogBootstrapState({
  blank_mobpack: {
    id: "blank",
    name: "Blank Meta mobpack",
    source: "meta/blank-mobpack",
    trigger: "blank · authoring",
    stage: "valid",
    document: { mob_id: "blank", schema_version: "0.1" },
    validation: { ok: true },
  },
  sample_mobpacks: [
    {
      id: "starter",
      name: "Starter",
      source: "meta/sample-mobpack",
      trigger: "label · starter",
      stage: "valid",
      document: { mob_id: "starter", schema_version: "0.1" },
      validation: { ok: true },
    },
    {
      id: "second",
      name: "Second",
      source: "meta/sample-mobpack",
      stage: "draft",
      document: { mob_id: "second", schema_version: "0.1" },
      validation: { ok: false },
    },
  ],
});
assert.deepEqual(bootstrapProjection.templates.map((row) => row.id), ["starter", "second"]);
assert.deepEqual(bootstrapProjection.flows, []);
assert.equal("initialHydration" in bootstrapProjection, false);

const savedRegistryBootstrap = controller.flowCatalogBootstrapState({
  blank_mobpack: {
    id: "blank",
    name: "Blank Meta mobpack",
    source: "meta/blank-mobpack",
    trigger: "blank · authoring",
    stage: "valid",
    document: { mob_id: "blank", schema_version: "0.1" },
    validation: { ok: true },
  },
  sample_mobpacks: [],
}, {
  registryResult: {
    rows: [{
      id: "saved_flow",
      name: "Saved Flow",
      source: "meta/mobpacks/save",
      trigger: "saved",
      stage: "draft",
      document: { mob_id: "saved_flow", name: "Saved Flow", schema_version: "0.1" },
      validation: null,
    }],
  },
});
assert.deepEqual(savedRegistryBootstrap.flows.map((row) => row.id), ["saved_flow"]);
// The library is home: bootstrap projects rows only, never a startup hydration.
assert.equal("initialHydration" in savedRegistryBootstrap, false);
assert.equal(savedRegistryBootstrap.flows[0].source, "meta/mobpacks/save");
assert.equal(savedRegistryBootstrap.flows[0].document.mob_id, "saved_flow");

const runtimeRegistryBootstrap = controller.flowCatalogBootstrapState({
  sample_mobpacks: [],
  runtime_flows: [
    {
      id: "runtime_live_main",
      name: "live-mob · main",
      source: "meta/runtime/flow_projection",
      trigger: "runtime · main",
      stage: "valid",
      document: {
        mob_id: "live-mob",
        name: "live-mob · main",
        schema_version: "0.1",
        flow: { name: "main", steps: [{ id: "input_1", type: "input", task: "Run.", inputParams: [] }] },
        members: [],
      },
      validation: { ok: true },
    },
  ],
}, {
  registryResult: { rows: [] },
});
assert.deepEqual(runtimeRegistryBootstrap.flows.map((row) => row.id), ["runtime_live_main"]);
assert.equal(runtimeRegistryBootstrap.flows[0].source, "meta/runtime/flow_projection");
assert.equal("initialHydration" in runtimeRegistryBootstrap, false);
assert.equal(runtimeRegistryBootstrap.flows[0].document.mob_id, "live-mob");
assert.equal(runtimeRegistryBootstrap.flows[0].trigger, "runtime · main");

const runtimeDedupedBootstrap = controller.flowCatalogBootstrapState({
  sample_mobpacks: [],
  runtime_flows: [{
    id: "saved_flow",
    name: "Runtime duplicate",
    source: "meta/runtime/flow_projection",
    registry_source: "meta/runtime/flow_projection",
    document_kind: "runtime_projection",
    runtime_projection: true,
    runtime_mob_id: "runtime_duplicate",
    runtime_flow_id: "main",
    deployability: { ready: true },
    provenance: { catalog: "meta/runtime/flow_projection" },
    document: { mob_id: "runtime_duplicate", flow: { name: "main", steps: [] } },
  }],
}, {
  registryResult: {
    rows: [{
      id: "saved_flow",
      name: "Saved Flow",
      source: "meta/mobpacks/save",
      trigger: "saved",
      stage: "draft",
      document: { mob_id: "saved_flow", name: "Saved Flow", schema_version: "0.1" },
      validation: null,
    }],
  },
});
assert.deepEqual(runtimeDedupedBootstrap.flows.map((row) => [row.id, row.source]), [
  ["saved_flow", "meta/runtime/flow_projection"],
]);
assert.equal(runtimeDedupedBootstrap.flows[0].runtime_projection, true);
assert.equal(runtimeDedupedBootstrap.flows[0].document_kind, "runtime_projection");
assert.equal(runtimeDedupedBootstrap.flows[0].runtime_mob_id, "runtime_duplicate");
assert.equal(runtimeDedupedBootstrap.flows[0].runtime_flow_id, "main");
assert.deepEqual(runtimeDedupedBootstrap.flows[0].deployability, { ready: true });
assert.deepEqual(runtimeDedupedBootstrap.flows[0].provenance, { catalog: "meta/runtime/flow_projection" });

assert.deepEqual(controller.flowCatalogBootstrapState({ sample_mobpacks: [] }), {
  templates: [],
  flows: [],
});

global.window.treeToGraph = () => {
  throw new Error("controller exports must not call the UI graph renderer");
};

const members = [{
  id: "m_reviewer",
  name: "Reviewer",
  role: "reviewer",
  model: "openai/gpt-5",
  tools: ["git", "shell"],
  skills: ["mob.review"],
  schema: "ReviewArtifact",
}];

const previousFlow = {
  name: "main",
  steps: [{
    id: "input_1",
    type: "input",
    task: "Review the change.",
    inputParams: [],
  }],
};

const flow = controller.graphToFlow({
  previousFlow,
  members,
  instances: [{
    id: "review_step",
    memberId: "m_reviewer",
    col: 0,
    row: 0,
    launchMode: {
      kind: "Resume",
      sessionId: "session-123",
    },
    dispatchMode: "one_to_one",
    collection: "quorum",
    quorum: 1,
    timeoutMs: 120000,
    allowedTools: ["git"],
    blockedTools: ["shell"],
    outputFormat: "text",
    dependsMode: "any",
  }],
  edges: [],
});

const graphSemanticBaseInstances = [{
  id: "review_step",
  memberId: "m_reviewer",
  col: 0,
  row: 0,
  launchMode: { kind: "Fresh" },
  collection: "all",
}];
const graphSemanticMovedInstances = [{
  ...graphSemanticBaseInstances[0],
  col: 9,
  row: 7,
}];
const graphSemanticChangedInstances = [{
  ...graphSemanticBaseInstances[0],
  launchMode: { kind: "Resume", sessionId: "session-456" },
}];
const graphSemanticBaseEdges = [{
  id: "route",
  from: "review_step",
  to: "review_step",
  kind: "cond",
  cond: { path: "steps.review_step.verdict", op: "==", value: "green" },
}];
const graphSemanticChangedEdges = [{
  ...graphSemanticBaseEdges[0],
  cond: { path: "steps.review_step.verdict", op: "!=", value: "red" },
}];
assert.notEqual(
  controller.graphSignature(graphSemanticBaseInstances, graphSemanticBaseEdges),
  controller.graphSignature(graphSemanticMovedInstances, graphSemanticBaseEdges),
);
assert.notEqual(
  controller.graphStructureSignature(graphSemanticBaseInstances, graphSemanticBaseEdges),
  controller.graphStructureSignature(graphSemanticMovedInstances, graphSemanticBaseEdges),
);
assert.notEqual(
  controller.graphStructureSignature(graphSemanticBaseInstances, graphSemanticBaseEdges),
  controller.graphStructureSignature(graphSemanticChangedInstances, graphSemanticBaseEdges),
);
assert.notEqual(
  controller.graphStructureSignature(graphSemanticBaseInstances, graphSemanticBaseEdges),
  controller.graphStructureSignature(graphSemanticBaseInstances, graphSemanticChangedEdges),
);
assert.notEqual(
  controller.graphStructureSignature(graphSemanticBaseInstances, [{ ...graphSemanticBaseEdges[0], kind: "" }]),
  controller.graphStructureSignature(graphSemanticBaseInstances, [{ ...graphSemanticBaseEdges[0], kind: "next" }]),
  "Graph signatures must not normalize missing edge kind to a local next default",
);

const graphOrderMembers = [{ id: "m_reviewer" }];
const graphOrderPreviousFlow = { steps: [{ id: "input_1", type: "input", task: "", inputParams: [] }] };
const graphOrderBase = controller.graphToFlow({
  previousFlow: graphOrderPreviousFlow,
  members: graphOrderMembers,
  instances: [
    { id: "draft", memberId: "m_reviewer", col: 0, row: 0 },
    { id: "review", memberId: "m_reviewer", col: 1, row: 0 },
  ],
  edges: [],
});
const graphOrderMoved = controller.graphToFlow({
  previousFlow: graphOrderPreviousFlow,
  members: graphOrderMembers,
  instances: [
    { id: "draft", memberId: "m_reviewer", col: 2, row: 0 },
    { id: "review", memberId: "m_reviewer", col: 1, row: 0 },
  ],
  edges: [],
});
assert.deepEqual(graphOrderBase.steps.map((step) => step.id), ["input_1", "draft", "review"]);
assert.deepEqual(graphOrderMoved.steps.map((step) => step.id), ["input_1", "review", "draft"]);

const graphMemberLabelInstances = [
  { id: "left", memberId: "m_left", col: 0, row: 0 },
  { id: "right", memberId: "m_right", col: 0, row: 1 },
];
const graphMemberLabelEdges = [
  { id: "route_left", from: "start", to: "left", kind: "cond", cond: { path: "params.kind", op: "==", value: "docs" } },
  { id: "route_right", from: "start", to: "right", kind: "cond", cond: { path: "params.kind", op: "==", value: "code" } },
];
const graphMemberLabelMembers = [{ id: "m_left", name: "Docs" }, { id: "m_right", name: "Code" }];
const graphMemberRenamedMembers = [{ id: "m_left", name: "Docs" }, { id: "m_right", name: "Implement" }];
assert.notEqual(
  controller.graphStructureSignature(graphMemberLabelInstances, graphMemberLabelEdges, { members: graphMemberLabelMembers }),
  controller.graphStructureSignature(graphMemberLabelInstances, graphMemberLabelEdges, { members: graphMemberRenamedMembers }),
);
const graphMemberLabelFlow = controller.graphToFlow({
  previousFlow: graphOrderPreviousFlow,
  members: graphMemberRenamedMembers,
  instances: graphMemberLabelInstances,
  edges: graphMemberLabelEdges,
});
assert.deepEqual(graphMemberLabelFlow.steps[1].branches.map((branch) => branch.label), ["Docs", "Implement"]);

assert.equal(flow.steps.length, 2);
assert.deepEqual(flow.steps[1], {
  id: "review_step",
  type: "member",
  role: "m_reviewer",
  instruction: "",
  launchMode: {
    kind: "Resume",
    sessionId: "session-123",
  },
  dispatchMode: "one_to_one",
  collection: "quorum",
  quorum: 1,
  timeoutMs: 120000,
  allowedTools: ["git"],
  blockedTools: ["shell"],
  outputFormat: "text",
  dependsMode: "any",
});

const flowWithPriorInstruction = controller.graphToFlow({
  previousFlow: {
    name: "main",
    steps: [
      previousFlow.steps[0],
      {
        id: "review_step",
        type: "member",
        role: "m_reviewer",
        instruction: "Run Reviewer.",
      },
    ],
  },
  members,
  instances: [{
    id: "review_step",
    memberId: "m_reviewer",
    col: 0,
    row: 0,
  }],
  edges: [],
});
assert.equal(flowWithPriorInstruction.steps[1].instruction, "Run Reviewer.");
const flowWithoutPriorInput = controller.graphToFlow({
  previousFlow: {
    name: "no-input",
    steps: [{ id: "input_1", type: "member", role: "m_reviewer" }],
  },
  members,
  instances: [],
  edges: [],
});
assert.deepEqual(flowWithoutPriorInput.steps, [{
  id: "input_2",
  type: "input",
  task: "",
  fields: "",
  inputParams: [],
}]);

const document = controller.buildDocument({
  flow,
  studio: {
    members,
    schemas: [],
    instances: [],
    edges: [
      { id: "stale_wrong_kind", from: "g_branch_route", to: "left", kind: "next", label: "stale" },
      { id: "stale_extra", from: "left", to: "g_parallel_deleted", kind: "next", label: "stale" },
    ],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "projection-proof" },
  deploySettings: testDeploySettings(),
});

const graphModeDocumentProjection = controller.authoringDocumentFromState({
  editorMode: "advanced",
  flow: { name: "graph-mode-doc", steps: [] },
  studio: {
    members,
    schemas: [],
    instances: [{ id: "review_step", memberId: "m_reviewer", col: 0, row: 0 }],
    edges: [{ id: "stale_edge", from: "review_step", to: "ghost", kind: "next", label: "stale" }],
    frames: [{ id: "stale_frame", kind: "Branch", label: "stale", colStart: 99, colEnd: 99 }],
    skillRealms: [],
  },
  currentFlow: { name: "graph-mode-doc" },
  deploySettings: testDeploySettings(),
  mobSettings: { backendDefault: "session" },
});
assert.deepEqual(graphModeDocumentProjection.flow.steps, []);
assert.deepEqual(graphModeDocumentProjection.document.flow.steps, []);
assert.deepEqual(graphModeDocumentProjection.instances, graphModeDocumentProjection.document.instances);
assert.deepEqual(graphModeDocumentProjection.edges, graphModeDocumentProjection.document.edges);
assert.deepEqual(graphModeDocumentProjection.frames, graphModeDocumentProjection.document.frames);
assert.deepEqual(graphModeDocumentProjection.instances, []);
assert.deepEqual(graphModeDocumentProjection.edges, []);
assert.deepEqual(graphModeDocumentProjection.frames, []);
assert.equal(graphModeDocumentProjection.document.mob_settings.backendDefault, "session");

const graphModeBranchMembers = [
  { id: "m_left", name: "Left", role: "left", model: "gpt-5.5", tools: ["builtins"], skills: [] },
  { id: "m_right", name: "Right", role: "right", model: "gpt-5.5", tools: ["builtins"], skills: [] },
  { id: "m_review", name: "Review", role: "review", model: "gpt-5.5", tools: ["builtins"], skills: [] },
];
const graphModeBranchProjection = controller.authoringDocumentFromState({
  editorMode: "advanced",
  flow: { name: "graph-mode-branch-doc", steps: [] },
  studio: {
    members: graphModeBranchMembers,
    schemas: [],
    instances: [
      { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
      { id: "left", memberId: "m_left", col: 1, row: 0 },
      { id: "right", memberId: "m_right", col: 1, row: 1 },
      { id: "j_branch_route", isGate: true, gateKind: "join", collection: "any", controllerRole: "m_review", col: 2, row: 0 },
    ],
    edges: [
      { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "docs", cond: { source: "params.kind", op: "==", value: "docs" } },
      { id: "e2", from: "g_branch_route", to: "right", kind: "next", label: "fallback" },
      { id: "stale_extra", from: "right", to: "ghost", kind: "next", label: "stale" },
    ],
    frames: [{ id: "stale_frame", kind: "Parallel", label: "stale", colStart: 7, colEnd: 8 }],
    skillRealms: [],
  },
  currentFlow: { name: "graph-mode-branch-doc" },
  deploySettings: testDeploySettings(),
  mobSettings: { backendDefault: "session" },
});
assert.deepEqual(graphModeBranchProjection.instances, graphModeBranchProjection.document.instances);
assert.deepEqual(graphModeBranchProjection.edges, graphModeBranchProjection.document.edges);
assert.deepEqual(graphModeBranchProjection.frames, graphModeBranchProjection.document.frames);
assert.deepEqual(graphModeBranchProjection.instances, []);
assert.deepEqual(graphModeBranchProjection.edges, []);
assert.deepEqual(graphModeBranchProjection.frames, []);
assert.deepEqual(graphModeBranchProjection.flow.steps, []);

const reconciledAuthoringDocumentProjection = controller.authoringDocumentFromState({
  editorMode: "basic",
  flow: {
    name: "export-reconcile-doc",
    steps: [{
      id: "work",
      type: "member",
      role: "m_worker",
      schema: "MissingArtifact",
      expectedSchemaRef: "schemas/MissingArtifact.json",
      allowedTools: ["shell", "git"],
      blockedTools: ["git"],
    }],
  },
  studio: {
    members: [{
      id: "m_worker",
      name: "Worker",
      role: "worker",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      backend: "session",
      model: "bad-model",
      tools: ["shell", "git"],
      skills: ["mob.review", "missing.skill"],
      schema: "MissingArtifact",
    }],
    schemas: [{ id: "ReviewArtifact", fields: [] }],
    instances: [{ id: "work", memberId: "m_worker", allowedTools: ["shell", "git"], blockedTools: ["git"] }],
    edges: [],
    frames: [],
    skillRealms: [{ id: "main", skills: [{ id: "mob.review", source: "inline", content: "Review." }] }],
  },
  currentFlow: { name: "export-reconcile-doc" },
  deploySettings: { ...testDeploySettings(), model: "bad-model" },
  mobSettings: { backendDefault: "session" },
  contract: {
    deploy_settings: { command: "rkat mob run" },
    mob_definition: {
      runtime_modes: ["turn_driven"],
      profile_binding: ["inline"],
      profile_backends: ["session"],
    },
  },
  modelCatalog: [{ id: "gpt-5.5" }],
  toolCatalog: [{ id: "shell" }],
  contractLoaded: true,
});
assert.equal(reconciledAuthoringDocumentProjection.document.deploy.model, "");
assert.deepEqual(reconciledAuthoringDocumentProjection.document.members[0].tools, ["shell"]);
assert.deepEqual(reconciledAuthoringDocumentProjection.document.members[0].skills, ["mob.review"]);
assert.equal(reconciledAuthoringDocumentProjection.document.members[0].schema, "");
assert(!("schema" in reconciledAuthoringDocumentProjection.document.flow.steps[0]));
assert(!("expectedSchemaRef" in reconciledAuthoringDocumentProjection.document.flow.steps[0]));
assert.deepEqual(reconciledAuthoringDocumentProjection.document.flow.steps[0].allowedTools, ["shell"]);
assert.deepEqual(reconciledAuthoringDocumentProjection.document.flow.steps[0].blockedTools, []);
assert.deepEqual(reconciledAuthoringDocumentProjection.document.skill_realms[0].skills, [{ id: "mob.review", source: "inline", content: "Review." }]);

const prunedSkillDocument = controller.buildDocument({
  flow,
  studio: {
    members,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
    skillRealms: [{
      id: "meta/sample-mobpacks",
      label: "meta/sample-mobpacks",
      source: "meta/sample-mobpack",
      skills: [
        { id: "mob.review", source: "inline", content: "Review the current work." },
        { id: "mob.unused", source: "inline", content: "Unused catalog skill." },
      ],
    }, {
      id: "filesystem",
      label: "filesystem",
      source: "filesystem",
      skills: [{ id: "mob.filesystem.unused", source: "path", path: "/tmp/unused.md" }],
    }],
  },
  currentFlow: { name: "skill-prune-proof" },
  deploySettings: testDeploySettings(),
});
assert.deepEqual(prunedSkillDocument.skill_realms, [{
  id: "meta/sample-mobpacks",
  label: "meta/sample-mobpacks",
  source: "meta/sample-mobpack",
  skills: [
    { id: "mob.review", source: "inline", content: "Review the current work." },
  ],
  default: false,
}]);
assert.deepEqual(controller.skillRealmsForDocument([{ id: "m", skills: [] }], prunedSkillDocument.skill_realms), []);

assert.deepEqual(document.launch_modes, [{
  step_id: "review_step",
  member_id: "m_reviewer",
  profile: "reviewer",
  launch_mode: {
    kind: "Resume",
    sessionId: "session-123",
  },
}]);

const missingLaunchFlow = controller.graphToFlow({
  previousFlow,
  members,
  instances: [{
    id: "missing_launch_step",
    memberId: "m_reviewer",
    col: 0,
    row: 0,
  }],
  edges: [],
});
assert.equal(missingLaunchFlow.steps[1].launchMode, null);
assert(!("dispatchMode" in missingLaunchFlow.steps[1]));
assert(!("collection" in missingLaunchFlow.steps[1]));
assert(!("dependsMode" in missingLaunchFlow.steps[1]));
assert(!("outputFormat" in missingLaunchFlow.steps[1]));

const missingLaunchDocument = controller.buildDocument({
  flow: missingLaunchFlow,
  studio: {
    members,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "missing-launch-proof" },
  deploySettings: testDeploySettings(),
});
assert.deepEqual(missingLaunchDocument.launch_modes, [{
  step_id: "missing_launch_step",
  member_id: "m_reviewer",
  profile: "reviewer",
  launch_mode: null,
}]);

const blankOptionalMetadataDocument = controller.buildDocument({
  flow: {
    name: "blank-optional-metadata",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Review",
      fields: "",
      inputParams: [],
    }, {
      id: "review_blank_output",
      type: "member",
      role: "m_reviewer",
      instruction: "Review.",
      launchMode: { kind: "Fresh" },
      dispatchMode: "",
      collection: "",
      dependsMode: "",
      outputFormat: "",
    }, {
      id: "loop_blank_iteration",
      type: "repeat",
      loopId: "review_loop",
      maxIterations: 2,
      iterationInput: "",
      cond: { stepId: "review_loop_step", field: "verdict", op: "==", val: "green" },
      steps: [{
        id: "review_loop_step",
        type: "member",
        role: "m_reviewer",
        instruction: "Review loop.",
        launchMode: { kind: "Fresh" },
        dispatchMode: null,
        collection: "",
        dependsMode: "",
        outputFormat: null,
      }],
    }],
  },
  studio: {
    members,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "blank-optional-metadata" },
  deploySettings: testDeploySettings(),
});
const blankSteps = blankOptionalMetadataDocument.flow.steps;
assert(!("dispatchMode" in blankSteps[1]));
assert(!("collection" in blankSteps[1]));
assert(!("dependsMode" in blankSteps[1]));
assert(!("outputFormat" in blankSteps[1]));
assert(!("iterationInput" in blankSteps[2]));
assert(!("dispatchMode" in blankSteps[2].steps[0]));
assert(!("collection" in blankSteps[2].steps[0]));
assert(!("dependsMode" in blankSteps[2].steps[0]));
assert(!("outputFormat" in blankSteps[2].steps[0]));
const blankInstance = blankOptionalMetadataDocument.instances.find((inst) => inst.id === "review_blank_output");
assert(!("dispatchMode" in blankInstance));
assert(!("collection" in blankInstance));
assert(!("dependsMode" in blankInstance));
assert(!("outputFormat" in blankInstance));

const explicitFreshNoBudgetFlow = controller.graphToFlow({
  previousFlow,
  members,
  instances: [{
    id: "fresh_no_budget_step",
    memberId: "m_reviewer",
    col: 0,
    row: 0,
    launchMode: { kind: "Fresh" },
  }],
  edges: [],
});
assert.deepEqual(explicitFreshNoBudgetFlow.steps[1].launchMode, { kind: "Fresh" });
const explicitFreshNoBudgetDocument = controller.buildDocument({
  flow: explicitFreshNoBudgetFlow,
  studio: {
    members,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "fresh-no-budget-proof" },
  deploySettings: testDeploySettings(),
});
assert.deepEqual(explicitFreshNoBudgetDocument.launch_modes[0], {
  step_id: "fresh_no_budget_step",
  member_id: "m_reviewer",
  profile: "reviewer",
  launch_mode: { kind: "Fresh" },
});

const schemaSyncedFlow = controller.reconcileFlowMemberSchemas({
  name: "main",
  steps: [{
    id: "review_step",
    type: "member",
    role: "m_reviewer",
    schema: "ReviewArtifact",
    expectedSchemaRef: "schemas/ReviewArtifact.json",
  }],
}, [{ ...members[0], schema: "RenamedVerdict" }]);
assert.deepEqual(schemaSyncedFlow.steps[0], {
  id: "review_step",
  type: "member",
  role: "m_reviewer",
  schema: "RenamedVerdict",
  expectedSchemaRef: "schemas/RenamedVerdict.json",
});

const customSchemaPathFlow = controller.reconcileFlowMemberSchemas({
  name: "main",
  steps: [{
    id: "review_step",
    type: "member",
    role: "m_reviewer",
    schema: "ReviewArtifact",
    expectedSchemaRef: "schemas/custom/reviewer.json",
  }],
}, [{ ...members[0], schema: "RenamedVerdict" }]);
assert.equal(customSchemaPathFlow.steps[0].schema, "RenamedVerdict");
assert.equal(customSchemaPathFlow.steps[0].expectedSchemaRef, "schemas/custom/reviewer.json");

const schemaRemovedFlow = controller.reconcileFlowMemberSchemas({
  name: "main",
  steps: [{
    id: "review_step",
    type: "member",
    role: "m_reviewer",
    schema: "ReviewArtifact",
    expectedSchemaRef: "schemas/ReviewArtifact.json",
    expected_schema_ref: "schemas/ReviewArtifact.json",
  }],
}, [{ ...members[0], schema: "" }]);
assert(!("schema" in schemaRemovedFlow.steps[0]));
assert(!("expectedSchemaRef" in schemaRemovedFlow.steps[0]));
assert(!("expected_schema_ref" in schemaRemovedFlow.steps[0]));

const schemaRename = controller.renameSchemaDefinition({
  schemas: [
    { id: "ReviewArtifact", fields: [] },
    { id: "PlanArtifact", fields: [] },
  ],
  members: [
    { id: "m_reviewer", schema: "ReviewArtifact" },
    { id: "m_planner", schema: "PlanArtifact" },
  ],
  flow: {
    name: "main",
    steps: [{
      id: "review_step",
      type: "member",
      role: "m_reviewer",
      schema: "ReviewArtifact",
      expectedSchemaRef: "schemas/ReviewArtifact.json",
    }, {
      id: "s_adaptive",
      type: "adaptive",
      flowmasterId: "m_planner",
      resultSchema: "ReviewArtifact",
      profileTemplateIds: ["m_reviewer"],
    }],
  },
}, "ReviewArtifact", "RenamedVerdict");
assert.equal(schemaRename.renamed, true);
assert.deepEqual(schemaRename.schemas.map((schema) => schema.id), ["RenamedVerdict", "PlanArtifact"]);
assert.equal(schemaRename.members[0].schema, "RenamedVerdict");
assert.equal(schemaRename.members[1].schema, "PlanArtifact");
assert.equal(schemaRename.flow.steps[0].schema, "RenamedVerdict");
assert.equal(schemaRename.flow.steps[0].expectedSchemaRef, "schemas/RenamedVerdict.json");
assert.equal(
  schemaRename.flow.steps[1].resultSchema,
  "RenamedVerdict",
  "adaptive resultSchema must follow the schema rename (mirrors Meta rewrite_adaptive_result_schema_references)",
);
assert.deepEqual(schemaRename.selection, { kind: "schema", id: "RenamedVerdict" });

const duplicateSchemaRename = controller.renameSchemaDefinition({
  schemas: [
    { id: "ReviewArtifact", fields: [] },
    { id: "PlanArtifact", fields: [] },
  ],
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  flow: { name: "main", steps: [] },
}, "ReviewArtifact", "PlanArtifact");
assert.equal(duplicateSchemaRename.renamed, false);
assert.equal(duplicateSchemaRename.selection, null);
assert.equal(duplicateSchemaRename.reason, "duplicate_schema_id");
assert.equal(duplicateSchemaRename.members[0].schema, "ReviewArtifact");
assert.deepEqual(duplicateSchemaRename.flow, { name: "main", steps: [] });

assert.deepEqual(controller.basicConditionFromText('params.route == "docs"'), {
  namespace: "params",
  stepId: "params",
  field: "route",
  op: "==",
  val: "docs",
});
assert.deepEqual(controller.basicConditionFromText("steps.review.verdict > 3"), {
  namespace: "steps",
  stepId: "review",
  field: "verdict",
  op: ">",
  val: "3",
});
assert.equal(controller.basicConditionText({
  namespace: "params",
  stepId: "params",
  field: "route",
  val: "docs",
}, { defaultOperator: "==" }), 'params.route == "docs"');
assert.equal(controller.basicConditionText({
  stepId: "review",
  field: "verdict",
  op: "==",
  val: "green",
}), 'steps.review.verdict == "green"');
assert.equal(controller.basicConditionLabel({
  stepId: "review",
  field: "score",
  op: ">",
  val: 3,
}, [{ stepId: "review", label: "Reviewer" }]), "Reviewer.score > 3");
assert.equal(controller.basicRepeatUntilExpression({
  cond: { stepId: "review_loop", field: "verdict", op: "==", val: "green" },
  steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }],
}, members, { defaultOperator: "==" }), 'Reviewer.verdict == "green"');
assert.equal(controller.basicRepeatUntilExpression({
  until: 'steps.review.verdict == "green"',
  cond: { stepId: "missing", field: "verdict", op: "==", val: "green" },
  steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }],
}, members, { defaultOperator: "==" }), 'steps.review.verdict == "green"');
assert.equal(controller.basicRepeatUntilExpression({
  cond: { stepId: "review_loop", field: "verdict", val: "green" },
  steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }],
}, members, {}), "");
const basicCanvasContract = {
  mob_definition: {
    defaults: {
      collection_policy: "all",
      condition_operator: "==",
    },
    collection_policies: ["all", "quorum"],
    condition_operators: ["=="],
  },
};
assert.deepEqual(controller.basicForkCanvasState({
  step: {
    id: "parallel_canvas",
    type: "parallel",
    branches: [
      { id: "left", label: "Left", steps: [] },
      { id: "right", label: "Right", steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }] },
    ],
  },
  contract: basicCanvasContract,
}), {
  isParallel: true,
  className: "bld-fork bld-fork--parallel",
  lanes: [
    { id: "left", label: "Left", steps: [] },
    { id: "right", label: "Right", steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }] },
  ],
  showRail: true,
  showJoin: true,
  joinLabel: "⋈ join · all",
});
assert.deepEqual(controller.basicForkCanvasState({
  step: {
    id: "branch_canvas",
    type: "branch",
    branches: [{ id: "docs", label: "Docs", steps: [] }],
    fallback: [{ id: "fallback_review", type: "member", role: "m_reviewer" }],
  },
  contract: basicCanvasContract,
  basicView: {
    ...hydratedCatalogs.basicView,
    branchFallbackTitle: "Otherwise",
  },
}).lanes.map((lane) => [lane.id, lane.label, lane.steps.length]), [
  ["docs", "Docs", 0],
  ["fallback", "Otherwise", 1],
]);
assert.equal(controller.basicRepeatIterationLabel({
  iterationInput: "review_loop",
  steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }],
}, members, hydratedCatalogs.basicView), "unsupported: feeds Reviewer's output");
assert.deepEqual(controller.basicRepeatCanvasState({
  step: {
    cond: { stepId: "review_loop", field: "verdict", op: "==", val: "green" },
    maxIterations: 4,
    iterationInput: "carry",
    steps: [{ id: "review_loop", type: "member", role: "m_reviewer" }],
  },
  members,
  contract: basicCanvasContract,
  basicView: hydratedCatalogs.basicView,
}), {
  repeatUntilExpression: 'Reviewer.verdict == "green"',
  whileLabel: "while",
  notLabel: "not",
  conditionLabel: 'Reviewer.verdict == "green"',
  maxIterationsLabel: "max 4",
  loopBackLabel: "↑ loop back · carries last output",
  exitLabel: '↓ exit when Reviewer.verdict == "green"',
});
assert.deepEqual(controller.basicStepCardState({
  step: { type: "parallel" },
  members,
  contract: basicCanvasContract,
  basicView: hydratedCatalogs.basicView,
}), {
  icon: "‖",
  iconTint: "member",
  title: "Parallel",
  desc: "fan-out → join · all",
  configured: true,
  isFlowCard: true,
});
assert.deepEqual(controller.basicStepCardState({
  step: { type: "member", role: "m_reviewer" },
  members,
  contract: basicCanvasContract,
  basicView: hydratedCatalogs.basicView,
}), {
  icon: "◆",
  iconTint: "accent",
  title: "Reviewer",
  desc: "reviewer · openai/gpt-5",
  configured: true,
  isFlowCard: false,
});
assert.equal(controller.conditionValueLiteral("green"), '"green"');
assert.equal(controller.conditionValueLiteral("42"), "42");
assert.equal(controller.conditionValueLiteral("true"), "true");
assert.deepEqual(controller.conditionValueControl({
  type: "enum",
  enumValues: ["green", 7],
}, "green", hydratedCatalogs.conditionView), {
  kind: "enum",
  values: ["green", "7"],
  value: "green",
  optionRows: [
    { value: "", label: "—" },
    { value: "green", label: "green" },
    { value: "7", label: "7" },
  ],
  placeholder: "",
});
assert.deepEqual(controller.conditionValueControl({ type: "boolean" }, "", hydratedCatalogs.conditionView), {
  kind: "boolean",
  values: ["true", "false"],
  value: "",
  optionRows: [
    { value: "", label: "—" },
    { value: "true", label: "true" },
    { value: "false", label: "false" },
  ],
  placeholder: "",
});
assert.equal(controller.conditionValueControl({ type: "bool" }, false, hydratedCatalogs.conditionView).value, "false");
assert.deepEqual(controller.conditionValueControl({ type: "string", enumValues: ["ignored"] }, "typed", hydratedCatalogs.conditionView), {
  kind: "text",
  values: [],
  value: "typed",
  optionRows: [],
  placeholder: "value",
});
assert.deepEqual(controller.conditionValueControl(null, "", hydratedCatalogs.conditionView), { kind: "text", values: [], value: "", optionRows: [], placeholder: "value" });
assert.deepEqual(controller.conditionValueControl({ type: "enum", enumValues: ["yes"] }, "", {
  emptyValueLabel: "(none)",
  textValuePlaceholder: "literal",
}).optionRows, [
  { value: "", label: "(none)" },
  { value: "yes", label: "yes" },
]);

const memberStepPrunedFlow = controller.reconcileFlowMemberSteps({
  name: "member-prune-proof",
  steps: [
    { id: "keep_top", type: "member", role: "m_reviewer" },
    { id: "drop_top", type: "member", role: "m_deleted" },
    {
      id: "repeat_keep",
      type: "repeat",
      steps: [
        { id: "drop_repeat", type: "member", role: "m_deleted" },
        { id: "keep_repeat", type: "member", role: "m_reviewer" },
      ],
    },
    {
      id: "branch_keep",
      type: "branch",
      branches: [
        { id: "drop_branch", steps: [{ id: "drop_branch_step", type: "member", role: "m_deleted" }] },
        { id: "keep_branch", steps: [{ id: "keep_branch_step", type: "member", role: "m_reviewer" }] },
      ],
      fallback: [{ id: "drop_fallback_step", type: "member", role: "m_deleted" }],
    },
    {
      id: "parallel_drop",
      type: "parallel",
      branches: [
        { id: "drop_parallel", steps: [{ id: "drop_parallel_step", type: "member", role: "m_deleted" }] },
      ],
    },
  ],
}, [members[0]]);
// Containers stay even when emptied — mirrors Meta's
// prune_step_array_for_members, which only drops member steps.
assert.deepEqual(memberStepPrunedFlow.steps.map((step) => step.id), ["keep_top", "repeat_keep", "branch_keep", "parallel_drop"]);
assert.deepEqual(memberStepPrunedFlow.steps[1].steps.map((step) => step.id), ["keep_repeat"]);
assert.deepEqual(memberStepPrunedFlow.steps[2].branches.map((branch) => branch.id), ["drop_branch", "keep_branch"]);
assert.deepEqual(memberStepPrunedFlow.steps[2].branches[0].steps, []);
assert.deepEqual(memberStepPrunedFlow.steps[2].fallback, []);
assert.deepEqual(memberStepPrunedFlow.steps[3].branches[0].steps, []);

const controlRoleFlow = controller.reconcileFlowControlRoles({
  name: "control-role-proof",
  steps: [{
    id: "parallel_review",
    type: "parallel",
    controllerRole: "m_deleted",
    controllerMemberId: "m_deleted",
    controlRole: "m_deleted",
    branches: [{
      id: "br_review",
      steps: [{
        id: "branch_nested",
        type: "branch",
        controllerRole: "m_reviewer",
        branches: [],
        fallback: [],
      }],
    }],
  }],
}, [members[0]]);
assert(!("controllerRole" in controlRoleFlow.steps[0]));
assert(!("controllerMemberId" in controlRoleFlow.steps[0]));
assert(!("controlRole" in controlRoleFlow.steps[0]));
assert.equal(controlRoleFlow.steps[0].branches[0].steps[0].controllerRole, "m_reviewer");

const graphControlInstances = controller.reconcileGraphControlRoles([
  { id: "join_stale", isGate: true, gateKind: "join", controllerRole: "m_deleted", joinRole: "m_deleted" },
  { id: "join_valid", isGate: true, gateKind: "join", controllerRole: "m_reviewer" },
], [members[0]]);
assert(!("controllerRole" in graphControlInstances[0]));
assert(!("joinRole" in graphControlInstances[0]));
assert.equal(graphControlInstances[1].controllerRole, "m_reviewer");

const graphMemberSync = controller.reconcileGraphMemberInstances({
  instances: [
    { id: "gate_branch", isGate: true, gateKind: "branch" },
    { id: "review_step", memberId: "m_reviewer" },
    { id: "deleted_step", memberId: "m_deleted" },
    { id: "source_file", isTerminal: true, kind: "success", label: "mob.toml" },
  ],
  edges: [
    { id: "keep_gate_review", from: "gate_branch", to: "review_step", kind: "next" },
    { id: "drop_deleted_out", from: "deleted_step", to: "review_step", kind: "next" },
    { id: "drop_deleted_in", from: "review_step", to: "deleted_step", kind: "next" },
    { id: "keep_source", from: "review_step", to: "source_file", kind: "next" },
  ],
}, [members[0]]);
assert.deepEqual(graphMemberSync.instances.map((instance) => instance.id), ["gate_branch", "review_step", "source_file"]);
assert.deepEqual(graphMemberSync.edges.map((edge) => edge.id), ["keep_gate_review", "keep_source"]);

const aggregateMemberReconcile = controller.reconcileAuthoringForMembers({
  members: [{ id: "m_reviewer", name: "Reviewer", role: "reviewer", tools: ["shell"], schema: "ReviewArtifact" }],
  previousMembers: [
    { id: "m_reviewer", name: "Review Lead", role: "reviewer" },
    { id: "m_deleted", name: "Deleted", role: "deleted" },
  ],
  mobSettings: {
    orchestrator: "review_lead",
    roleWiring: [{ a: "review_lead", b: "deleted" }, { a: "deleted", b: "review_lead" }],
    backendDefault: "session",
  },
  flow: {
    name: "aggregate-member-reconcile",
    steps: [
      { id: "keep_review", type: "member", role: "m_reviewer", schema: "OldArtifact", allowedTools: ["shell", "git"] },
      { id: "drop_deleted", type: "member", role: "m_deleted" },
      { id: "parallel_review", type: "parallel", controllerRole: "m_deleted", branches: [{ id: "br", steps: [{ id: "nested_review", type: "member", role: "m_reviewer" }] }] },
    ],
  },
  instances: [
    { id: "join_stale", isGate: true, gateKind: "join", controllerRole: "m_deleted" },
    { id: "review_inst", memberId: "m_reviewer", schema: "OldArtifact", allowedTools: ["shell", "git"] },
    { id: "deleted_inst", memberId: "m_deleted" },
  ],
  edges: [
    { id: "keep_join_review", from: "join_stale", to: "review_inst", kind: "next" },
    { id: "drop_deleted", from: "deleted_inst", to: "review_inst", kind: "next" },
  ],
});
assert.deepEqual(aggregateMemberReconcile.flow.steps.map((step) => step.id), ["keep_review", "parallel_review"]);
assert.equal(aggregateMemberReconcile.flow.steps[0].schema, "ReviewArtifact");
assert.deepEqual(aggregateMemberReconcile.flow.steps[0].allowedTools, ["shell"]);
assert(!("controllerRole" in aggregateMemberReconcile.flow.steps[1]));
assert.deepEqual(aggregateMemberReconcile.instances.map((instance) => instance.id), ["join_stale", "review_inst"]);
assert(!("controllerRole" in aggregateMemberReconcile.instances[0]));
assert.deepEqual(aggregateMemberReconcile.instances[1].allowedTools, ["shell"]);
assert.deepEqual(aggregateMemberReconcile.edges.map((edge) => edge.id), ["keep_join_review"]);
assert.equal(aggregateMemberReconcile.mobSettings.orchestrator, "reviewer");
assert.deepEqual(aggregateMemberReconcile.mobSettings.roleWiring, []);

const memberUpdateCascade = controller.memberUpdateCascadePatch({
  memberId: "m_reviewer",
  members: [
    { id: "m_reviewer", name: "Review Lead", role: "reviewer", tools: ["shell", "git"], schema: "ReviewArtifact" },
  ],
  flow: {
    name: "member-update-cascade-proof",
    steps: [
      { id: "review_step", type: "member", role: "m_reviewer", allowedTools: ["shell", "git"], schema: "ReviewArtifact" },
    ],
  },
  instances: [
    { id: "review_inst", memberId: "m_reviewer", allowedTools: ["shell", "git"], schema: "ReviewArtifact" },
  ],
  edges: [],
  mobSettings: {
    orchestrator: "review_lead",
    roleWiring: [{ a: "review_lead", b: "review_lead" }],
  },
}, {
  name: "Reviewer",
  tools: ["shell"],
});
assert.equal(memberUpdateCascade.ok, true);
assert.deepEqual(memberUpdateCascade.members[0], {
  id: "m_reviewer",
  name: "Reviewer",
  role: "reviewer",
  tools: ["shell"],
  schema: "ReviewArtifact",
});
assert.deepEqual(memberUpdateCascade.flow.steps[0].allowedTools, ["shell"]);
assert.deepEqual(memberUpdateCascade.instances[0].allowedTools, ["shell"]);
assert.equal(memberUpdateCascade.mobSettings.orchestrator, "reviewer");
assert.deepEqual(memberUpdateCascade.mobSettings.roleWiring, [{ a: "reviewer", b: "reviewer" }]);
assert.deepEqual(controller.memberUpdateCascadePatch({
  memberId: "missing",
  members: [{ id: "m_reviewer" }],
  flow: aggregateMemberReconcile.flow,
  instances: aggregateMemberReconcile.instances,
  edges: aggregateMemberReconcile.edges,
  mobSettings: { orchestrator: "reviewer" },
}, { name: "Nope" }), {
  ok: false,
  error: "member not found",
  patch: null,
  member: null,
  members: [{ id: "m_reviewer" }],
  flow: aggregateMemberReconcile.flow,
  instances: aggregateMemberReconcile.instances,
  edges: aggregateMemberReconcile.edges,
  mobSettings: {
    orchestrator: "reviewer",
    autoWireOrchestrator: false,
    roleWiring: [],
    backendDefault: "",
    externalAddressBase: "",
    advanced: {
      topology: null,
      supervisor: null,
      limits: null,
      spawnPolicy: null,
      eventRouter: null,
    },
  },
});

const aggregateContractReconcile = controller.reconcileAuthoringWithContract({
  contractLoaded: true,
  contract: {
    deploy_settings: {
      command: "rkat mob run",
      surfaces: ["cli"],
      trust_policies: ["permissive"],
      realm_backends: ["sqlite"],
    },
    mob_definition: {
      runtime_modes: ["turn_driven"],
      profile_binding: ["inline"],
      profile_backends: ["session"],
    },
  },
  modelCatalog: [{ id: "gpt-5.5", label: "GPT-5.5" }],
  toolCatalog: [{ id: "shell", label: "Shell" }],
  skillRealms: [{ id: "main", skills: [{ id: "mob.review" }] }],
  schemas: [{ id: "ReviewArtifact", fields: [] }],
  deploySettings: {
    command: "wrong",
    surface: "desktop",
    trustPolicy: "strict",
    realmBackend: "memory",
    model: "bad-model",
  },
  members: [{
    id: "m_worker",
    name: "Worker",
    role: "worker",
    profileBinding: "realm",
    runtimeMode: "autonomous_host",
    backend: "external",
    model: "bad-model",
    tools: ["shell", "git"],
    skills: ["mob.review", "missing.skill"],
    schema: "MissingArtifact",
  }],
  mobSettings: {
    orchestrator: "worker",
    backendDefault: "external",
    roleWiring: [{ a: "worker", b: "worker" }],
  },
  flow: {
    name: "contract-reconcile",
    steps: [{
      id: "work",
      type: "member",
      role: "m_worker",
      schema: "MissingArtifact",
      expectedSchemaRef: "schemas/MissingArtifact.json",
      allowedTools: ["shell", "git"],
      blockedTools: ["git"],
    }],
  },
  instances: [{ id: "i_worker", memberId: "m_worker", allowedTools: ["shell", "git"], blockedTools: ["git"] }],
  edges: [],
});
assert.deepEqual(aggregateContractReconcile.deploySettings, {
  command: "rkat mob run",
  surface: "",
  trustPolicy: "",
  model: "",
  maxDuration: "",
  maxToolCalls: null,
  maxTotalTokens: null,
  isolated: false,
  realm: "",
  instance: "",
  realmBackend: "",
  contextRoot: "",
  stateRoot: "",
  userConfigRoot: "",
  prompt: "",
});
assert.deepEqual(aggregateContractReconcile.members[0].skills, ["mob.review"]);
assert.deepEqual(aggregateContractReconcile.members[0].tools, ["shell"]);
assert.equal(aggregateContractReconcile.members[0].schema, "");
assert.equal(aggregateContractReconcile.members[0].profileBinding, "");
assert.equal(aggregateContractReconcile.members[0].runtimeMode, "");
assert.equal(aggregateContractReconcile.members[0].backend, "");
assert.equal(aggregateContractReconcile.members[0].model, "");
assert.deepEqual(aggregateContractReconcile.flow.steps[0].allowedTools, ["shell"]);
assert.deepEqual(aggregateContractReconcile.flow.steps[0].blockedTools, []);
assert(!("schema" in aggregateContractReconcile.flow.steps[0]));
assert(!("expectedSchemaRef" in aggregateContractReconcile.flow.steps[0]));
assert.deepEqual(aggregateContractReconcile.instances[0].allowedTools, ["shell"]);
assert.deepEqual(aggregateContractReconcile.instances[0].blockedTools, []);
assert.equal(aggregateContractReconcile.mobSettings.backendDefault, "");
assert.equal(aggregateContractReconcile.mobSettings.orchestrator, "worker");
assert.equal(aggregateContractReconcile.changed, true);

const stableAggregateContractReconcile = controller.reconcileAuthoringWithContract({
  contractLoaded: true,
  contract: {
    deploy_settings: {
      command: "rkat mob run",
      surfaces: ["cli"],
      trust_policies: ["permissive"],
      realm_backends: ["sqlite"],
    },
    mob_definition: {
      runtime_modes: ["turn_driven"],
      profile_binding: ["inline"],
      profile_backends: ["session"],
    },
  },
  modelCatalog: [{ id: "gpt-5.5", label: "GPT-5.5" }],
  toolCatalog: [{ id: "shell", label: "Shell" }],
  skillRealms: [{ id: "main", skills: [{ id: "mob.review" }] }],
  schemas: [{ id: "ReviewArtifact", fields: [] }],
  deploySettings: aggregateContractReconcile.deploySettings,
  members: aggregateContractReconcile.members,
  mobSettings: aggregateContractReconcile.mobSettings,
  flow: aggregateContractReconcile.flow,
  instances: aggregateContractReconcile.instances,
  edges: aggregateContractReconcile.edges,
});
assert.equal(stableAggregateContractReconcile.changed, false);

const launchSourceFlow = controller.reconcileFlowLaunchSources({
  name: "launch-source-proof",
  steps: [
    { id: "plan", type: "member", role: "m_reviewer", launchMode: { kind: "Fresh" } },
    {
      id: "branch_launch",
      type: "branch",
      branches: [{
        id: "br_launch",
        steps: [{
          id: "review_fork",
          type: "member",
          role: "m_reviewer",
          launchMode: { kind: "Fork", from: "deleted_step", context: "full_history" },
        }],
      }],
      fallback: [],
    },
  ],
}, [members[0]]);
assert.deepEqual(launchSourceFlow.steps[1].branches[0].steps[0].launchMode, {
  kind: "Fresh",
});
const validLaunchSourceFlow = controller.reconcileFlowLaunchSources({
  name: "launch-source-valid-proof",
  steps: [
    { id: "plan", type: "member", role: "m_reviewer", launchMode: { kind: "Fresh" } },
    { id: "review_fork", type: "member", role: "m_reviewer", launchMode: { kind: "Fork", from: "plan", context: "full_history" } },
  ],
}, [members[0]]);
assert.equal(validLaunchSourceFlow.steps[1].launchMode.from, "plan");

const graphLaunchInstances = controller.reconcileGraphLaunchSources([
  { id: "plan", memberId: "m_reviewer", launchMode: { kind: "Fresh" } },
  { id: "review_fork", memberId: "m_reviewer", launchMode: { kind: "Fork", from: "deleted_instance", context: "full_history" } },
  { id: "review_member_fork", memberId: "m_reviewer", launchMode: { kind: "Fork", from: "m_reviewer", context: "full_history" } },
], [members[0]]);
assert.deepEqual(graphLaunchInstances[1].launchMode.kind, "Fresh");
assert.equal(graphLaunchInstances[2].launchMode.from, "m_reviewer");

const scopedToolMembers = [
  { id: "m_tool", name: "Tool User", role: "tool_user", tools: ["builtins", "shell"] },
];
const scopedToolFlow = controller.reconcileFlowStepToolScopes({
  name: "tool-scope-proof",
  steps: [
    { id: "input_1", type: "input", task: "", inputParams: [] },
    {
      id: "branch_1",
      type: "branch",
      branches: [{
        id: "br_1",
        label: "Branch",
        steps: [{
          id: "tool_step",
          type: "member",
          role: "m_tool",
          allowedTools: ["builtins", "mob"],
          blockedTools: ["shell", "memory"],
        }],
      }],
      fallback: [],
    },
  ],
}, scopedToolMembers);
assert.deepEqual(scopedToolFlow.steps[1].branches[0].steps[0].allowedTools, ["builtins"]);
assert.deepEqual(scopedToolFlow.steps[1].branches[0].steps[0].blockedTools, ["shell"]);

const scopedToolInstances = controller.reconcileGraphStepToolScopes([
  { id: "tool_inst", memberId: "m_tool", allowedTools: ["builtins", "mob"], blockedTools: ["shell", "memory"] },
  { id: "gate", isGate: true, allowedTools: ["mob"], blockedTools: ["shell"] },
], scopedToolMembers);
assert.deepEqual(scopedToolInstances[0].allowedTools, ["builtins"]);
assert.deepEqual(scopedToolInstances[0].blockedTools, ["shell"]);
assert.deepEqual(scopedToolInstances[1].allowedTools, ["mob"]);

const skillScopedMembers = [
  { id: "m_skill", name: "Skill User", role: "skill_user", skills: ["mob.workpad", "mob.missing"] },
];
assert.equal(controller.reconcileMemberSkillRefs(skillScopedMembers, []), skillScopedMembers);
assert.deepEqual(controller.reconcileMemberSkillRefs(skillScopedMembers, [], { strictEmpty: true })[0].skills, []);
const reconciledSkillMembers = controller.reconcileMemberSkillRefs(skillScopedMembers, [{
  id: "realm",
  skills: [{ id: "mob.workpad" }, { id: "mob.review" }],
}]);
assert.deepEqual(reconciledSkillMembers[0].skills, ["mob.workpad"]);

const pendingDeploySettings = { surface: "custom", model: "custom-model" };
assert.equal(controller.reconcileDeploySettingsWithContract(pendingDeploySettings, null, []), pendingDeploySettings);
assert.equal(controller.reconcileDeploySettingsWithContract({ model: "custom-model" }, null, []).model, "custom-model");
assert.equal(controller.reconcileDeploySettingsWithContract({ model: "custom-model" }, null, [], { strictEmptyModels: true }).model, "");
const reconciledDeploySettings = controller.reconcileDeploySettingsWithContract({
  command: "legacy deploy command",
  surface: "console",
  trustPolicy: "loose",
  realmBackend: "memory",
  model: "missing-model",
  prompt: "Run it.",
}, {
  deploy_settings: {
    command: "rkat mob run",
    surfaces: ["cli", "rpc"],
    trust_policies: ["permissive", "strict"],
    realm_backends: ["jsonl", "sqlite"],
  },
}, [{ id: "openai/gpt-5" }]);
assert.equal(reconciledDeploySettings.command, "rkat mob run");
assert.equal(reconciledDeploySettings.surface, "");
assert.equal(reconciledDeploySettings.trustPolicy, "");
assert.equal(reconciledDeploySettings.realmBackend, "");
assert.equal(reconciledDeploySettings.model, "");

const pendingContractMembers = [{ id: "m_custom", model: "custom-model", runtimeMode: "custom", tools: ["custom-tool"] }];
assert.equal(controller.reconcileMembersWithContract(pendingContractMembers, null, {}, []), pendingContractMembers);
const reconciledContractMembers = controller.reconcileMembersWithContract([
  {
    id: "m_bad",
    profileBinding: "legacy_profile",
    runtimeMode: "invalid_runtime",
    backend: "sidecar",
    model: "missing-model",
    tools: ["builtins", "missing-tool"],
  },
  {
    id: "m_host",
    profileBinding: "inline",
    runtimeMode: "autonomous_host",
    backend: "session",
    model: "openai/gpt-5",
    tools: ["shell"],
  },
], {
  mob_definition: {
    defaults: { runtime_mode: "turn_driven" },
    runtime_modes: ["turn_driven", "autonomous_host"],
    deploy_runtime_mode_compatibility: {
      cli: {
        allowed: ["turn_driven"],
        blocked: {
          autonomous_host: "RPC surface only; rkat mob run requires turn_driven profiles.",
        },
      },
    },
    profile_binding: ["inline"],
    profile_backends: ["session", "external"],
  },
}, { surface: "cli" }, [{ id: "openai/gpt-5" }], [{ id: "builtins" }]);
assert.equal(reconciledContractMembers[0].profileBinding, "");
assert.equal(reconciledContractMembers[0].runtimeMode, "");
assert.equal(reconciledContractMembers[0].backend, "");
assert.equal(reconciledContractMembers[0].model, "");
assert.deepEqual(reconciledContractMembers[0].tools, ["builtins"]);
assert.equal(reconciledContractMembers[1].runtimeMode, "turn_driven");
assert.deepEqual(reconciledContractMembers[1].tools, []);
assert.equal(controller.reconcileMembersWithContract(pendingContractMembers, null, {}, [], [], { strictEmptyModels: true })[0].model, "");
assert.deepEqual(controller.reconcileMembersWithContract(pendingContractMembers, null, {}, [], [], { strictEmptyTools: true })[0].tools, []);

const pendingMobSettings = { backendDefault: "custom-backend" };
assert.equal(controller.reconcileMobSettingsWithContract(pendingMobSettings, null), pendingMobSettings);
assert.equal(controller.reconcileMobSettingsWithContract({
  backendDefault: "sidecar",
  externalAddressBase: "http://127.0.0.1:9000",
}, {
  mob_definition: { profile_backends: ["session", "external"] },
}).backendDefault, "");

const schemaReferenceFlow = {
  name: "schema-reference-proof",
  steps: [
    { id: "input_1", type: "input" },
    { id: "review_step", type: "member", role: "m_reviewer", schema: "ReviewArtifact" },
    {
      id: "branch_review",
      type: "branch",
      branches: [{
        id: "br_green",
        label: "Green",
        condition: 'steps.review_step.verdict == "green"',
        cond: { namespace: "steps", stepId: "review_step", field: "verdict", op: "==", val: "green" },
        steps: [],
      }],
      fallback: [],
    },
    {
      id: "loop_review",
      type: "repeat",
      until: 'steps.review_loop.verdict == "green"',
      cond: { namespace: "steps", stepId: "review_loop", field: "verdict", op: "==", val: "green" },
      steps: [{ id: "review_loop", type: "member", role: "m_reviewer", schema: "ReviewArtifact" }],
    },
  ],
};
const schemaReferenceEdges = [{
  id: "e_branch_green",
  from: "g_branch_review",
  to: "review_step",
  kind: "cond",
  label: 'steps.review_step.verdict == "green"',
  cond: { var: "steps.review_step.verdict", op: "==", val: "green" },
}];
const renamedReferences = controller.reconcileSchemaFieldReferences({
  flow: schemaReferenceFlow,
  edges: schemaReferenceEdges,
  members,
  instances: [{ id: "review_step", memberId: "m_reviewer" }, { id: "review_loop", memberId: "m_reviewer" }],
  schemaId: "ReviewArtifact",
  oldName: "verdict",
  newName: "decision",
});
assert.equal(renamedReferences.flow.steps[2].branches[0].cond.field, "decision");
assert.equal(renamedReferences.flow.steps[2].branches[0].condition, 'steps.review_step.decision == "green"');
assert.equal(renamedReferences.flow.steps[3].cond.field, "decision");
assert.equal(renamedReferences.flow.steps[3].until, 'steps.review_loop.decision == "green"');
assert.equal(renamedReferences.edges[0].cond.var, "steps.review_step.decision");
assert.equal(renamedReferences.edges[0].label, 'steps.review_step.decision == "green"');

const deletedReferences = controller.reconcileSchemaFieldReferences({
  flow: renamedReferences.flow,
  edges: renamedReferences.edges,
  members,
  instances: [{ id: "review_step", memberId: "m_reviewer" }, { id: "review_loop", memberId: "m_reviewer" }],
  schemaId: "ReviewArtifact",
  oldName: "decision",
  newName: "",
});
assert.deepEqual(deletedReferences.flow.steps[2].branches[0].cond, {});
assert.equal(deletedReferences.flow.steps[2].branches[0].condition, "");
assert.deepEqual(deletedReferences.flow.steps[3].cond, {});
assert.equal(deletedReferences.flow.steps[3].until, "");
assert.equal(deletedReferences.edges[0].cond, null);
assert.equal(deletedReferences.edges[0].label, "");

assert.equal(document.flow.steps[1].timeoutMs, 120000);
assert.deepEqual(document.flow.steps[1].allowedTools, ["git"]);
assert.deepEqual(document.flow.steps[1].blockedTools, ["shell"]);
assert.equal(document.flow.steps[1].outputFormat, "text");

const [agentDefinition] = controller.agentDefinitionsFromCatalogs({
  agent_definitions: [{
    id: "reviewer",
    role: "reviewer",
    name: "Reviewer",
    model: "gpt-5.5",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_review_pr",
    sourceMobpackName: "Review PR",
    sourceOrigin: "meta/sample-mobpack",
    sourceDocumentPath: "document.members[]",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    backend: "sidecar",
    schema: "ReviewArtifact",
    schemaSourceDocumentPath: "document.schemas[]",
    schemaDefinition: {
      id: "ReviewArtifact",
      description: "Review output",
      fields: [{ id: "f1", name: "verdict", type: "enum", required: true, enumValues: ["green", "red"] }],
    },
    tools: ["mob"],
    toolDefinitions: [{ id: "mob", kind: "runtime", source: "meerkat_mob::ToolConfig" }],
    skills: ["mob.review"],
    skillDefinitions: [{ id: "mob.review", source: "inline", sourceMobpack: "sample_review_pr" }],
  }],
});
assert.equal(agentDefinition.backend, "sidecar");
assert.equal(agentDefinition.sourceMobpack, "sample_review_pr");
assert.equal(agentDefinition.sourceMobpackName, "Review PR");
assert.equal(agentDefinition.sourceOrigin, "meta/sample-mobpack");
assert.equal(agentDefinition.schemaSourceDocumentPath, "document.schemas[]");
assert.deepEqual(agentDefinition.toolDefinitions, [{ id: "mob", kind: "runtime", source: "meerkat_mob::ToolConfig" }]);
assert.deepEqual(agentDefinition.skillDefinitions, [{ id: "mob.review", source: "inline", sourceMobpack: "sample_review_pr" }]);

assert.deepEqual(controller.agentDefinitionsFromCatalogs({
  members: [{
    id: "legacy-member",
    role: "legacy",
    name: "Legacy",
    model: "gpt-5.5",
  }],
}), []);

assert.deepEqual(controller.agentDefinitionsFromCatalogs({
  agent_definitions: [{
    id: "origin_less",
    role: "origin_less",
    name: "Origin Less",
    model: "gpt-5.5",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_origin_less",
    sourceDocumentPath: "document.members[]",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
}), []);

assert.deepEqual(controller.agentDefinitionsFromCatalogs({
  agent_definitions: [{
    id: "partial",
    role: "partial",
    name: "Partial",
    model: "gpt-5.5",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_partial",
    sourceDocumentPath: "document.members[]",
  }],
}), []);

assert.deepEqual(controller.agentDefinitionsFromCatalogs({
  agent_definitions: [{
    id: "model_less",
    role: "model_less",
    name: "Model Less",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    sourceMobpack: "sample_partial",
    sourceDocumentPath: "document.members[]",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
}), []);

assert.equal(controller.memberPromptSkeleton({
  name: "Reviewer",
  role: "reviewer",
  schema: "ReviewArtifact",
  systemPrompt: "  Gate the implementation.\n\nEmit a verdict.  ",
}), [
  "You are Reviewer, a member of a Meerkat mob.",
  "",
  "## Mandate",
  "Gate the implementation. Emit a verdict.",
  "",
  "## Operating rules",
  "- Read the shared mob workpad and prior members' output before acting.",
  "- Do exactly what this step requires — no more, no less.",
  "- Emit a ReviewArtifact as your structured output.",
  "- Hand off cleanly: state what you did and what the next member needs.",
].join("\n"));
assert.match(controller.memberPromptSkeleton({ role: "planner" }), /Act as the planner of the mob\./);
assert.match(controller.memberPromptSkeleton({ name: "Coder" }), /Return a concise, well-structured result\./);
assert.deepEqual(controller.memberNamePatch(" Quality Reviewer "), { name: " Quality Reviewer " });
assert.deepEqual(controller.memberRealmProfilePatch(" qa_profile "), { realmProfile: "qa_profile" });
assert.deepEqual(controller.memberSystemPromptPatch("  Review carefully.\n"), { systemPrompt: "  Review carefully.\n" });

assert.deepEqual(controller.agentDefinitionsFromCatalogs({
  agent_definitions: [{
    id: "",
    role: "missing_id",
    name: "Missing id",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }, {
    id: "missing_role",
    role: "",
    name: "Missing role",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }, {
    id: "missing_name",
    role: "missing_name",
    name: "",
    definitionType: "meta/profile-member",
    source: "meta/mobpack-profile-member",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }],
}), []);

const agentContract = {
  mob_definition: {
    profile_binding: ["inline"],
    profile_binding_restrictions: {
      realm_profile: {
        deployable: false,
        label: "realm_profile — import-only; rkat mob validate forbids realm refs in packs",
        reason: "rkat mob validate rejects mobpack profiles that use realm_profile references; export deployable packs with inline profiles.",
      },
    },
    runtime_modes: ["turn_driven", "autonomous_host"],
    runtime_mode_labels: {
      turn_driven: "turn_driven — explicit turn dispatch",
      autonomous_host: "autonomous_host — RPC keep-alive member loop",
    },
    deploy_runtime_mode_compatibility: {
      cli: {
        allowed: ["turn_driven"],
        blocked: {
          autonomous_host: "RPC surface only; rkat mob run requires turn_driven profiles.",
        },
      },
      rpc: {
        allowed: ["turn_driven", "autonomous_host"],
        blocked: {},
      },
    },
    profile_backends: ["session", "external"],
  },
};
assert.deepEqual(
  controller.profileBindingOptions(agentContract, "").map((option) => option.value),
  ["inline"],
);
assert.deepEqual(
  controller.profileBindingOptions(agentContract, "realm_profile").map((option) => [option.value, option.disabled, option.reason]),
  [
    ["inline", false, ""],
    ["realm_profile", true, "rkat mob validate rejects mobpack profiles that use realm_profile references; export deployable packs with inline profiles."],
  ],
);
assert.deepEqual(
  controller.runtimeModeOptions(agentContract, { surface: "rpc" }, "").map((option) => option.value),
  ["turn_driven", "autonomous_host"],
);
assert.deepEqual(
  controller.runtimeModeOptions(agentContract, { surface: "rpc" }, "").map((option) => option.label),
  ["turn_driven — explicit turn dispatch", "autonomous_host — RPC keep-alive member loop"],
);
assert.deepEqual(
  controller.runtimeModeOptions(agentContract, { surface: "cli" }, "").map((option) => [option.value, option.disabled, option.reason]),
  [
    ["turn_driven", false, ""],
    ["autonomous_host", true, "RPC surface only; rkat mob run requires turn_driven profiles."],
  ],
);
const tweaksState = controller.tweaksControlState({
  deploySettings: { surface: "cli", trustPolicy: "permissive", realmBackend: "sqlite" },
  mobSettings: { backendDefault: "session" },
  members: [{ id: "m_reviewer", role: "reviewer", name: "Reviewer" }],
  modelCatalog: [{ id: "openai/gpt-5.5", label: "GPT-5.5", vendor: "OpenAI" }],
  settingsView: TEST_SETTINGS_VIEW,
  contract: {
    deploy_settings: {
      surfaces: ["cli", "rpc"],
      trust_policies: ["permissive", "strict"],
      realm_backends: ["sqlite"],
    },
    mob_definition: {
      profile_backends: ["session", "external"],
    },
  },
});
// The load-mob select is gone (the library owns loading); settings state
// no longer projects panel chrome or loadable rows.
assert.equal("loadableFlowOptions" in tweaksState, false);
assert.equal("panelTitle" in tweaksState, false);
assert.equal("loadMobTitle" in tweaksState, false);
assert.equal(tweaksState.canvasTitle, "Canvas");
assert.equal(tweaksState.edgeStyleLabel, "Edges");
assert.deepEqual(tweaksState.edgeStyleOptions, [
  { value: "text", label: "Text" },
  { value: "icons", label: "Icons" },
  { value: "colored", label: "Color" },
]);
assert.equal(tweaksState.densityLabel, "Density");
assert.deepEqual(tweaksState.densityOptions, [
  { value: "compact", label: "Compact" },
  { value: "comfortable", label: "Comfy" },
]);
assert.equal(tweaksState.themeTitle, "Theme");
assert.equal(tweaksState.themeModeLabel, "Mode");
assert.deepEqual(tweaksState.themeModeOptions, [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
]);
assert.equal(tweaksState.mobTitle, "Mob");
assert.equal(tweaksState.orchestratorLabel, "Orchestrator");
assert.equal(tweaksState.autoWireLabel, "Auto wire");
assert.deepEqual(tweaksState.autoWireOptions, [
  { value: "no", label: "No" },
  { value: "yes", label: "Yes" },
]);
assert.equal(tweaksState.defaultBackendLabel, "Default backend");
assert.equal(tweaksState.externalBaseLabel, "External base");
assert.equal(tweaksState.externalBasePlaceholder, "http://127.0.0.1:9000");
assert.equal(tweaksState.deployTitle, "Deploy");
assert.equal(tweaksState.surfaceLabel, "Surface");
assert.equal(tweaksState.trustLabel, "Trust");
assert.equal(tweaksState.modelLabel, "Model");
assert.equal(tweaksState.durationLabel, "Duration");
assert.equal(tweaksState.durationPlaceholder, "30s");
assert.equal(tweaksState.toolCallsLabel, "Tool calls");
assert.equal(tweaksState.toolCallsMin, 0);
assert.equal(tweaksState.toolCallsMax, 999);
assert.equal(tweaksState.tokensLabel, "Tokens");
assert.equal(tweaksState.tokensMin, 0);
assert.equal(tweaksState.tokensMax, 200000);
assert.equal(tweaksState.realmLabel, "Realm");
assert.deepEqual(tweaksState.realmOptions, [
  { value: "isolated", label: "Isolated" },
  { value: "shared", label: "Shared" },
]);
assert.equal(tweaksState.realmIdLabel, "Realm ID");
assert.equal(tweaksState.realmIdPlaceholder, "realm id");
assert.equal(tweaksState.backendLabel, "Backend");
assert.equal(tweaksState.promptLabel, "Prompt");
assert.equal(tweaksState.promptPlaceholder, "Deploy prompt");
assert.equal(tweaksState.commandLabel, "Command");
assert.equal(tweaksState.commandFallback, "--");
assert.equal(tweaksState.inspectorTitle, "Inspector");
assert.equal(tweaksState.inspectorLayoutLabel, "Layout");
assert.deepEqual(tweaksState.inspectorLayoutOptions, [
  { value: "right", label: "Right" },
  { value: "bottom", label: "Bottom" },
  { value: "modal", label: "Modal" },
]);
assert.deepEqual(tweaksState.profileOptions, [
  { value: "", label: "none" },
  { value: "reviewer", label: "reviewer" },
]);
assert.deepEqual(tweaksState.profileChoices, [{ value: "reviewer", label: "reviewer" }]);
assert.deepEqual(tweaksState.modelOptions, [
  { value: "", label: "default" },
  { value: "openai/gpt-5.5", label: "GPT-5.5 · OpenAI" },
]);
assert.deepEqual(tweaksState.surfaceOptions.map((option) => option.value), ["cli", "rpc"]);
assert.deepEqual(tweaksState.trustOptions.map((option) => option.value), ["permissive", "strict"]);
assert.deepEqual(tweaksState.realmBackendOptions.map((option) => option.value), ["sqlite"]);
assert.deepEqual(tweaksState.mobBackendOptions.map((option) => option.value), ["session", "external"]);
const unsupportedDeployTweaksState = controller.tweaksControlState({
  deploySettings: { surface: "grpc", trustPolicy: "dangerous", realmBackend: "memory" },
  mobSettings: {},
  settingsView: TEST_SETTINGS_VIEW,
  contract: {
    deploy_settings: {
      surfaces: ["cli"],
      trust_policies: ["permissive"],
      realm_backends: ["jsonl"],
    },
    mob_definition: {
      profile_backends: ["session"],
    },
  },
});
assert.deepEqual(unsupportedDeployTweaksState.surfaceOptions.at(-1), {
  value: "grpc",
  label: "grpc — not in Meta deploy_settings.surfaces",
  disabled: true,
  reason: "Unsupported by the Meta deploy_settings.surfaces contract.",
});
assert.deepEqual(unsupportedDeployTweaksState.trustOptions.at(-1), {
  value: "dangerous",
  label: "dangerous — not in Meta deploy_settings.trust_policies",
  disabled: true,
  reason: "Unsupported by the Meta deploy_settings.trust_policies contract.",
});
assert.deepEqual(unsupportedDeployTweaksState.realmBackendOptions.at(-1), {
  value: "memory",
  label: "memory — not in Meta deploy_settings.realm_backends",
  disabled: true,
  reason: "Unsupported by the Meta deploy_settings.realm_backends contract.",
});
assert.deepEqual(controller.memberProfileBindingPatch({ role: "reviewer", name: "Reviewer" }, "realm_profile", agentContract), {});
assert.deepEqual(controller.memberProfileBindingPatch({ realmProfile: "qa_profile" }, "inline", agentContract), {
  profileBinding: "inline",
  realmProfile: "",
});
assert.deepEqual(controller.memberRuntimeModePatch("turn_driven", agentContract, { surface: "cli" }), { runtimeMode: "turn_driven" });
assert.deepEqual(controller.memberRuntimeModePatch("autonomous_host", agentContract, { surface: "cli" }), {});
assert.deepEqual(controller.memberRuntimeModePatch("", agentContract, { surface: "cli" }), {});
assert.deepEqual(controller.memberModelPatch(" openai/gpt-5 ", [{ id: "openai/gpt-5" }]), { model: "openai/gpt-5" });
assert.deepEqual(controller.memberModelPatch("openai/ghost", [{ id: "openai/gpt-5" }]), {});
assert.deepEqual(controller.memberModelPatch("", [{ id: "openai/gpt-5" }]), {});
assert.deepEqual(controller.memberSchemaPatch(" ReviewArtifact ", [{ id: "ReviewArtifact" }]), { schema: "ReviewArtifact" });
assert.deepEqual(controller.memberSchemaPatch("GhostArtifact", [{ id: "ReviewArtifact" }]), {});
assert.deepEqual(controller.memberSchemaPatch("", [{ id: "ReviewArtifact" }]), { schema: "" });
const memberSchemaCascade = controller.memberSchemaCascadePatch({
  memberId: "m_reviewer",
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  schemas: [
    { id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict", type: "enum" }] },
    { id: "SummaryArtifact", fields: [{ id: "f2", name: "summary", type: "string" }] },
  ],
  flow: {
    name: "member-schema-cascade",
    steps: [
      { id: "review_step", type: "member", role: "m_reviewer" },
      {
        id: "route",
        type: "branch",
        branches: [{
          id: "br_green",
          cond: { stepId: "review_step", field: "verdict", op: "==", val: "green" },
          condition: "steps.review_step.verdict == \"green\"",
          steps: [],
        }],
        fallback: [],
      },
    ],
  },
  edges: [{
    id: "e_review_done",
    from: "review_inst",
    to: "done",
    kind: "cond",
    label: "steps.review_inst.verdict == \"green\"",
    cond: { var: "steps.review_inst.verdict", op: "==", val: "green" },
  }],
  instances: [
    { id: "review_inst", memberId: "m_reviewer" },
    { id: "done", isTerminal: true },
  ],
}, "SummaryArtifact");
assert.equal(memberSchemaCascade.ok, true);
assert.deepEqual(memberSchemaCascade.patch, { schema: "SummaryArtifact" });
assert.deepEqual(memberSchemaCascade.members, [{ id: "m_reviewer", schema: "SummaryArtifact" }]);
assert.deepEqual(memberSchemaCascade.instances, [
  { id: "review_inst", memberId: "m_reviewer" },
  { id: "done", isTerminal: true },
]);
assert.deepEqual(memberSchemaCascade.flow.steps[1].branches[0].cond, {});
assert.equal(memberSchemaCascade.flow.steps[1].branches[0].condition, "");
assert.deepEqual(memberSchemaCascade.edges[0].cond, null);
assert.equal(memberSchemaCascade.edges[0].label, "");
assert.deepEqual(controller.memberSchemaCascadePatch({
  memberId: "m_reviewer",
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  schemas: [{ id: "ReviewArtifact" }],
}, "GhostArtifact"), {
  ok: false,
  error: "unknown schema",
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  flow: undefined,
  edges: undefined,
  instances: [],
  patch: null,
});
assert.deepEqual(controller.memberBackendPatch("external", agentContract), { backend: "external" });
assert.deepEqual(controller.memberBackendPatch("daemon", agentContract), {});
assert.deepEqual(controller.memberMaxInlinePeerNotificationsPatch("4"), { maxInlinePeerNotifications: 4 });
assert.deepEqual(controller.memberMaxInlinePeerNotificationsPatch(""), { maxInlinePeerNotifications: null });
assert.deepEqual(controller.memberMaxInlinePeerNotificationsPatch("-2"), { maxInlinePeerNotifications: null });
assert.deepEqual(controller.memberProviderParamsEditorState({
  providerParams: { thinking_budget: 4096, top_k: 20 },
}, hydratedCatalogs.agentDetailView), {
  label: "Provider params",
  text: '{\n  "thinking_budget": 4096,\n  "top_k": 20\n}',
  placeholder: '{"thinking_budget":4096}',
  rows: 4,
  invalidJsonLabel: "invalid JSON",
});
assert.deepEqual(controller.memberProviderParamsEditorState({ providerParams: { top_k: 20 } }, {
  ...hydratedCatalogs.agentDetailView,
  providerParamsLabel: "Provider settings",
  providerParamsPlaceholder: '{"top_k":20}',
  providerParamsRows: 6,
  providerParamsInvalidJsonLabel: "bad JSON",
}), {
  label: "Provider settings",
  text: '{\n  "top_k": 20\n}',
  placeholder: '{"top_k":20}',
  rows: 6,
  invalidJsonLabel: "bad JSON",
});
assert.equal(controller.memberProviderParamsEditorState({ providerParams: null }, hydratedCatalogs.agentDetailView).text, "");
assert.deepEqual(controller.memberProviderParamsPatch('{"thinking_budget":4096}', hydratedCatalogs.agentDetailView), {
  ok: true,
  patch: { providerParams: { thinking_budget: 4096 } },
  error: "",
});
assert.deepEqual(controller.memberProviderParamsPatch("", hydratedCatalogs.agentDetailView), {
  ok: true,
  patch: { providerParams: null },
  error: "",
});
assert.deepEqual(controller.memberProviderParamsPatch("[]", {
  ...hydratedCatalogs.agentDetailView,
  providerParamsObjectRequiredError: "provider settings must be an object",
}), {
  ok: false,
  patch: null,
  error: "provider settings must be an object",
});
assert.equal(controller.memberProviderParamsPatch("{", hydratedCatalogs.agentDetailView).ok, false);

const agentMembersForProjection = [
  { id: "m_planner", name: "Planner", role: "planner", model: "gpt-5.5", schema: "PlanArtifact" },
  {
    id: "m_reviewer",
    name: "Reviewer",
    role: "reviewer",
    model: "gpt-5.5",
    schema: "ReviewArtifact",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    backend: "session",
    sourceDefinition: {
      definitionType: "meta/profile-member",
      definitionId: "reviewer",
      source: "meta/mobpack-profile-member",
      sourceMobpack: "sample_review_pr",
      sourceMobpackName: "Review PR",
      sourceOrigin: "meta/sample-mobpack",
      sourceDocumentPath: "document.members[]",
      schemaSourceDocumentPath: "document.schemas[]",
      toolDefinitions: [{ id: "mob", source: "meerkat_mob::ToolConfig" }],
      skillDefinitions: [{ id: "mob.review", sourceMobpack: "sample_review_pr" }],
    },
  },
];
const agentSchemasForProjection = [
  { id: "PlanArtifact", fields: [{ id: "f1" }] },
  { id: "ReviewArtifact", fields: [{ id: "f1" }, { id: "f2" }] },
];
const agentInstancesForProjection = [
  { id: "n_review_1", memberId: "m_reviewer", col: 1, row: 2, lane: "main" },
  { id: "n_review_2", memberId: "m_reviewer", col: 2, row: 3 },
];
assert.deepEqual(controller.agentListState({
  members: agentMembersForProjection,
  instances: agentInstancesForProjection,
  schemas: agentSchemasForProjection,
  selection: { kind: "schema", id: "ReviewArtifact" },
  agentView: hydratedCatalogs.agentView,
}), {
  agentsHeading: "AGENTS",
  schemasHeading: "SCHEMAS",
  addSchemaLabel: "+ new schema",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  schemaAddFallbackError: "add_schema failed",
  emptyTitle: "AGENT LIBRARY",
  emptyLines: [
    "Select an agent or schema on the left.",
    "Agents are reusable across topologies. Edit one here and every placement updates.",
  ],
  missingSchemaLabel: "Schema not found.",
  missingAgentLabel: "Agent not found.",
  memberCount: 2,
  schemaCount: 2,
  memberRows: [
    {
      id: "m_planner",
      name: "Planner",
      role: "planner",
      model: "gpt-5.5",
      member: agentMembersForProjection[0],
      selected: false,
      itemClass: "agents-list__item",
      bulletRole: "planner",
      bulletStyle: { "--role-color": "var(--accent)" },
      subLabel: "planner · gpt-5.5",
      placedCount: 0,
      placedLabel: "unplaced",
      isUnplaced: true,
      placedClass: "agents-list__placed is-zero",
    },
    {
      id: "m_reviewer",
      name: "Reviewer",
      role: "reviewer",
      model: "gpt-5.5",
      member: agentMembersForProjection[1],
      selected: false,
      itemClass: "agents-list__item",
      bulletRole: "reviewer",
      bulletStyle: { "--role-color": "var(--warn, #c98810)" },
      subLabel: "reviewer · gpt-5.5",
      placedCount: 2,
      placedLabel: "×2",
      isUnplaced: false,
      placedClass: "agents-list__placed",
    },
  ],
  schemaRows: [
    {
      id: "PlanArtifact",
      schema: agentSchemasForProjection[0],
      selected: false,
      itemClass: "agents-list__item",
      bulletRole: "schema",
      bulletStyle: { "--role-color": "var(--subtle)" },
      fieldCount: 1,
      fieldLabel: "1 field",
      usedCount: 1,
      usageLabel: "used by 1",
      subLabel: "1 field · used by 1",
    },
    {
      id: "ReviewArtifact",
      schema: agentSchemasForProjection[1],
      selected: true,
      itemClass: "agents-list__item is-selected",
      bulletRole: "schema",
      bulletStyle: { "--role-color": "var(--subtle)" },
      fieldCount: 2,
      fieldLabel: "2 fields",
      usedCount: 1,
      usageLabel: "used by 1",
      subLabel: "2 fields · used by 1",
    },
  ],
});
const reskinnedAgentListState = controller.agentListState({
  members: agentMembersForProjection,
  instances: agentInstancesForProjection,
  schemas: agentSchemasForProjection,
  selection: { kind: "schema", id: "ReviewArtifact" },
  agentView: {
    ...hydratedCatalogs.agentView,
    memberSubLabelTemplate: "{model} / {role}",
    memberPlacedEmptyLabel: "not placed",
    memberPlacedCountTemplate: "placed {count}",
    schemaFieldSingularTemplate: "{count} column",
    schemaFieldPluralTemplate: "{count} columns",
    schemaUsageLabelTemplate: "{count} refs",
    sidebarSubLabelSeparator: " | ",
  },
});
assert.equal(reskinnedAgentListState.memberRows[0].subLabel, "gpt-5.5 / planner");
assert.equal(reskinnedAgentListState.memberRows[0].placedLabel, "not placed");
assert.equal(reskinnedAgentListState.memberRows[1].placedLabel, "placed 2");
assert.equal(reskinnedAgentListState.schemaRows[0].fieldLabel, "1 column");
assert.equal(reskinnedAgentListState.schemaRows[1].fieldLabel, "2 columns");
assert.equal(reskinnedAgentListState.schemaRows[1].usageLabel, "1 refs");
assert.equal(reskinnedAgentListState.schemaRows[1].subLabel, "2 columns | 1 refs");
assert.deepEqual(controller.agentListState({}), {
  agentsHeading: "",
  schemasHeading: "",
  addSchemaLabel: "",
  authoringOperationUnavailableError: "",
  schemaAddFallbackError: "",
  emptyTitle: "",
  emptyLines: [],
  missingSchemaLabel: "",
  missingAgentLabel: "",
  memberCount: 0,
  schemaCount: 0,
  memberRows: [],
  schemaRows: [],
});
assert.deepEqual(controller.agentSelectionState({
  selection: { kind: "agent", id: "m_reviewer" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}).member, agentMembersForProjection[1]);
const emptyAgentSelection = controller.agentSelectionState({
  selection: null,
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
});
assert.equal(emptyAgentSelection.emptyState.title, "AGENT LIBRARY");
assert.deepEqual(emptyAgentSelection.emptyState.lines, [
  "Select an agent or schema on the left.",
  "Agents are reusable across topologies. Edit one here and every placement updates.",
]);
assert.equal(controller.agentSelectionState({
  selection: { kind: "schema", id: "Missing" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}).missing, true);
assert.equal(controller.agentSelectionState({
  selection: { kind: "schema", id: "Missing" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}).missingSchemaLabel, "Schema not found.");
assert.equal(controller.agentSelectionState({
  selection: { kind: "agent", id: "Missing" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}).missingAgentLabel, "Agent not found.");
assert.deepEqual(controller.agentDefaultSelectionProjection({
  selection: null,
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}), {
  kind: "agent",
  id: "m_planner",
});
assert.deepEqual(controller.agentDefaultSelectionProjection({
  selection: { kind: "agent", id: "m_reviewer" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}), {
  kind: "agent",
  id: "m_reviewer",
});
assert.deepEqual(controller.agentDefaultSelectionProjection({
  selection: { kind: "agent", id: "missing" },
  members: agentMembersForProjection,
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}), {
  kind: "agent",
  id: "m_planner",
});
assert.deepEqual(controller.agentDefaultSelectionProjection({
  selection: null,
  members: [],
  schemas: agentSchemasForProjection,
  agentView: hydratedCatalogs.agentView,
}), {
  kind: "schema",
  id: "PlanArtifact",
});
assert.equal(controller.agentDefaultSelectionProjection({
  selection: null,
  members: [],
  schemas: [],
  agentView: hydratedCatalogs.agentView,
}), null);
assert.deepEqual(controller.agentListSelectionProjection("agent", "m_reviewer"), {
  kind: "agent",
  id: "m_reviewer",
});
assert.deepEqual(controller.agentListSelectionProjection("schema", "ReviewGate"), {
  kind: "schema",
  id: "ReviewGate",
});
assert.equal(controller.agentListSelectionProjection("edge", "e_1"), null);
assert.equal(controller.agentListSelectionProjection("agent", " "), null);
const agentEditorState = controller.agentEditorControlState({
  member: agentMembersForProjection[1],
  instances: agentInstancesForProjection,
  schemas: agentSchemasForProjection,
  modelCatalog: [{ id: "gpt-5.5", label: "GPT-5.5", vendor: "OpenAI" }],
  agentDetailView: {
    usedInLabel: "placed in",
    instanceSingular: "slot",
    instancePlural: "slots",
    deleteLabel: "REMOVE",
    deleteConfirmIntro: "Remove agent",
    deleteConfirmPlacedPrefix: "It appears in",
    deleteCancelLabel: "KEEP",
    cellSingular: "cell",
    cellPlural: "cells",
    deleteConfirmCellsSuffix: "graph placements will be removed.",
    usageTitlePrefix: "PLACEMENTS",
    emptyUsageHint: "No graph placements.",
    agentEyebrowPrefix: "PROFILE",
    identityTitle: "PROFILE",
    profileBindingLabel: "Binding",
    missingProfileBindingLabel: "missing binding",
    realmProfileLabel: "Realm ref",
    realmProfilePlaceholder: "realm/profile",
    realmProfileImportHintFallback: "Realm refs are import-only.",
    realmProfileTitle: "REALM REF",
    realmProfileReferenceHintBefore: "Imported profile",
    realmProfileReferenceHintAfterFallback: "requires inline conversion.",
    modelLabel: "Model id",
    runtimeModeLabel: "Runtime",
    runtimeSectionTitle: "ADVANCED RUNTIME",
    missingRuntimeModeLabel: "missing runtime",
    backendLabel: "Profile backend",
    backendDefinitionDefaultLabel: "use definition backend",
    inlinePeerNotificationsLabel: "Inline notifications",
    inlinePeerNotificationsPlaceholder: "default",
    providerParamsLabel: "Provider settings",
    providerParamsPlaceholder: '{"top_k":20}',
    providerParamsRows: 6,
    providerParamsInvalidJsonLabel: "bad JSON",
    providerParamsObjectRequiredError: "provider settings must be an object",
    systemPromptTitle: "PEER DESCRIPTION",
    applySkeletonLabel: "SKELETON",
    applySkeletonTitle: "Apply skeleton",
    systemPromptPlaceholder: "Describe the mandate.",
    outputSchemaTitle: "OUTPUT CONTRACT",
    schemaNoneLabel: "none",
    schemaRequiredLabel: "required",
    editSchemaLabel: "Open schema",
    emptySchemaHint: "Free-form output.",
    sourceTitle: "SOURCE",
    sourceEmptyHint: "Created here.",
    sourceDefinitionLabel: "Definition",
    sourceMobpackLabel: "Mobpack",
    sourceOriginLabel: "Origin",
    sourceDocumentPathLabel: "Member path",
    sourceSchemaPathLabel: "Schema path",
    sourceToolsLabel: "Tool refs",
    sourceSkillsLabel: "Skill refs",
  },
  deploySettings: { surface: "cli" },
  contract: {
    mob_definition: {
      profile_binding: ["inline"],
      profile_binding_restrictions: {
        realm_profile: {
          deployable: false,
          label: "realm_profile — import-only; rkat mob validate forbids realm refs in packs",
          reason: "rkat mob validate rejects mobpack profiles that use realm_profile references; export deployable packs with inline profiles.",
        },
      },
      runtime_modes: ["turn_driven"],
      profile_backends: ["session", "external"],
    },
  },
});
assert.equal(agentEditorState.placedAt.length, 2);
assert.equal(agentEditorState.placedCount, 2);
assert.equal(agentEditorState.idLine, "m_reviewer · placed in 2 slots");
assert.equal(agentEditorState.deleteLabel, "REMOVE");
assert.equal(agentEditorState.deleteCancelLabel, "KEEP");
assert.equal(agentEditorState.deleteNeedsConfirmation, true);
assert.equal(agentEditorState.deleteConfirmMessage, "Remove agent \"Reviewer\"? It appears in 2 cells - graph placements will be removed.");
assert.deepEqual(controller.agentDeleteConfirmationState(agentEditorState, false), {
  open: false,
  needsConfirmation: true,
  message: "Remove agent \"Reviewer\"? It appears in 2 cells - graph placements will be removed.",
  confirmLabel: "REMOVE",
  cancelLabel: "KEEP",
});
assert.deepEqual(controller.agentDeleteConfirmationState(agentEditorState, true), {
  open: true,
  needsConfirmation: true,
  message: "Remove agent \"Reviewer\"? It appears in 2 cells - graph placements will be removed.",
  confirmLabel: "REMOVE",
  cancelLabel: "KEEP",
});
assert.equal(agentEditorState.usageTitle, "PLACEMENTS · 2");
assert.equal(agentEditorState.emptyUsageHint, "No graph placements.");
assert.equal(agentEditorState.eyebrow, "PROFILE · reviewer");
assert.deepEqual(agentEditorState.usageRows.map((row) => [row.id, row.cellLabel, row.laneLabel]), [
  ["n_review_1", "cell (2,3)", "main"],
  ["n_review_2", "cell (3,4)", "—"],
]);
assert.equal(agentEditorState.identityTitle, "PROFILE");
assert.equal(agentEditorState.profileBindingLabel, "Binding");
assert.equal(agentEditorState.realmProfileLabel, "Realm ref");
assert.equal(agentEditorState.realmProfilePlaceholder, "realm/profile");
assert.equal(agentEditorState.realmProfileImportHint, "rkat mob validate rejects mobpack profiles that use realm_profile references; export deployable packs with inline profiles.");
assert.equal(agentEditorState.realmProfileTitle, "REALM REF");
assert.equal(agentEditorState.realmProfileReferenceLabel, "reviewer");
assert.equal(agentEditorState.realmProfileReferenceHintBefore, "Imported profile");
assert.equal(agentEditorState.realmProfileReferenceHintAfter, "from a target realm. rkat mob validate rejects mobpack profiles that use realm_profile references; export deployable packs with inline profiles.");
assert.equal(agentEditorState.modelLabel, "Model id");
assert.equal(agentEditorState.runtimeModeLabel, "Runtime");
assert.equal(agentEditorState.runtimeSectionTitle, "ADVANCED RUNTIME");
assert.equal(agentEditorState.backendLabel, "Profile backend");
assert.equal(agentEditorState.inlinePeerNotificationsLabel, "Inline notifications");
assert.equal(agentEditorState.inlinePeerNotificationsPlaceholder, "default");
assert.equal(agentEditorState.systemPromptTitle, "PEER DESCRIPTION");
assert.equal(agentEditorState.applySkeletonLabel, "SKELETON");
assert.equal(agentEditorState.applySkeletonTitle, "Apply skeleton");
assert.equal(agentEditorState.systemPromptPlaceholder, "Describe the mandate.");
assert.deepEqual(controller.agentEditorControlState({
  member: { id: "m_unplaced", name: "Unplaced", role: "writer" },
  instances: [],
  schemas: [],
}).deleteNeedsConfirmation, false);
assert.equal(agentEditorState.schema.id, "ReviewArtifact");
assert.equal(agentEditorState.outputSchemaTitle, "OUTPUT CONTRACT");
assert.equal(agentEditorState.hasOutputSchema, true);
assert.deepEqual(agentEditorState.schemaPreviewRows, [
  { id: "f1", name: undefined, type: undefined, required: false, requiredLabel: "" },
  { id: "f2", name: undefined, type: undefined, required: false, requiredLabel: "" },
]);
assert.equal(agentEditorState.editSchemaLabel, "Open schema");
assert.deepEqual(agentEditorState.editSchemaSelection, { kind: "schema", id: "ReviewArtifact" });
assert.equal(agentEditorState.emptySchemaHint, "Free-form output.");
assert.deepEqual(agentEditorState.sourceProvenance, {
  title: "SOURCE",
  emptyHint: "Created here.",
  hasRows: true,
  rows: [
    { label: "Definition", value: "reviewer" },
    { label: "Mobpack", value: "Review PR" },
    { label: "Origin", value: "meta/sample-mobpack" },
    { label: "Member path", value: "document.members[]" },
    { label: "Schema path", value: "document.schemas[]" },
    { label: "Tool refs", value: "mob (meerkat_mob::ToolConfig)" },
    { label: "Skill refs", value: "mob.review (sample_review_pr)" },
  ],
});
assert.deepEqual(controller.agentSourceProvenanceState({ id: "m_local" }, {
  sourceTitle: "SOURCE",
  sourceEmptyHint: "Created here.",
}).hasRows, false);
assert.equal(agentEditorState.profileBinding, "inline");
assert.equal(agentEditorState.runtimeMode, "turn_driven");
assert.equal(agentEditorState.backendValue, "session");
assert.deepEqual(agentEditorState.backendOptions.map((option) => [option.value, option.label]), [
  ["", "use definition backend"],
  ["session", "session"],
  ["external", "external"],
]);
assert.deepEqual(agentEditorState.modelOptions.map((option) => option.label), ["GPT-5.5 · OpenAI"]);
assert.deepEqual(controller.agentEditorControlState({
  member: { id: "m_custom", model: "custom/model" },
  modelCatalog: [],
}).modelOptions, [{ value: "custom/model", label: "custom/model", model: null }]);
assert.deepEqual(agentEditorState.schemaOptions.map((option) => [option.value, option.label]), [
  ["", "none"],
  ["PlanArtifact", "PlanArtifact"],
  ["ReviewArtifact", "ReviewArtifact"],
]);
assert.deepEqual(controller.agentDefinitionOptions([{
  id: "planner",
  role: "planner",
}, {
  id: "reviewer",
  role: "reviewer",
  label: "Quality Reviewer",
}, {
  role: "missing-id",
}]), {
  hasDefinitions: true,
  optionRows: [
    {
      value: "planner",
      label: "planner",
      definition: { id: "planner", role: "planner" },
    },
    {
      value: "reviewer",
      label: "Quality Reviewer",
      definition: { id: "reviewer", role: "reviewer", label: "Quality Reviewer" },
    },
  ],
});
assert.deepEqual(controller.agentDefinitionOptions([{
  id: "sample_a__01_reviewer",
  role: "reviewer",
  label: "Reviewer",
  sourceMobpackName: "Planner Coder Review",
}, {
  id: "sample_b__01_reviewer",
  role: "reviewer",
  label: "Reviewer",
  sourceMobpackName: "Docs Only",
}]).optionRows.map((row) => [row.value, row.label]), [
  ["sample_a__01_reviewer", "Reviewer · Planner Coder Review"],
  ["sample_b__01_reviewer", "Reviewer · Docs Only"],
]);
assert.deepEqual(controller.agentDefinitionOptions([]), { hasDefinitions: false, optionRows: [] });
assert.deepEqual(controller.agentDefinitionCatalogState([], hydratedCatalogs.agentView), {
  title: "DEFINITION CATALOG",
  empty: "No Meta profile-member definitions available.",
  hasRows: false,
  rows: [],
});
assert.deepEqual(controller.agentDefinitionCatalogState([{
  id: "sample_review",
  role: "reviewer",
  label: "Quality Reviewer",
  source: "meta/mobpack-profile-member",
  sourceMobpackName: "Planner Coder Review",
  sourceOrigin: "meta/sample-mobpack",
  toolDefinitions: [{ id: "builtins", source: "meerkat_mob::ToolConfig" }],
  skillDefinitions: [{ id: "mob.review", sourceMobpack: "sample_planner_coder_review_loop" }],
}], hydratedCatalogs.agentView), {
  title: "DEFINITION CATALOG",
  empty: "No Meta profile-member definitions available.",
  hasRows: true,
  rows: [{
    id: "sample_review",
    title: "Quality Reviewer",
    role: "reviewer",
    sourceLabel: "source",
    toolsLabel: "tools",
    skillsLabel: "skills",
    source: "Planner Coder Review · meta/sample-mobpack",
    tools: "builtins (meerkat_mob::ToolConfig)",
    skills: "mob.review (sample_planner_coder_review_loop)",
    definition: {
      id: "sample_review",
      role: "reviewer",
      label: "Quality Reviewer",
      source: "meta/mobpack-profile-member",
      sourceMobpackName: "Planner Coder Review",
      sourceOrigin: "meta/sample-mobpack",
      toolDefinitions: [{ id: "builtins", source: "meerkat_mob::ToolConfig" }],
      skillDefinitions: [{ id: "mob.review", sourceMobpack: "sample_planner_coder_review_loop" }],
    },
  }],
});
assert.deepEqual(controller.agentDefinitionAddControlState([], hydratedCatalogs.agentView), {
  hasDefinitions: false,
  optionRows: [],
  controlClass: "agents-list__add",
  disabled: true,
  title: "Meta schema contract has not provided agent definitions yet.",
  unavailableLabel: "agents unavailable",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  placeholderOption: { value: "", label: "+ new agent..." },
  value: "",
});
assert.deepEqual(controller.agentDefinitionAddErrorState({
  ok: false,
  error: "Meta agent definition references unavailable tool(s): ghost",
}, hydratedCatalogs.agentView), {
  hasError: true,
  text: "Agent definition unavailable: Meta agent definition references unavailable tool(s): ghost",
  rawError: "Meta agent definition references unavailable tool(s): ghost",
});
assert.deepEqual(controller.agentDefinitionAddErrorState({ ok: true }, hydratedCatalogs.agentView), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.memberSchemaChangeErrorState({ ok: false, error: "unknown schema" }), {
  hasError: true,
  text: "unknown schema",
  rawError: "unknown schema",
});
assert.deepEqual(controller.memberSchemaChangeErrorState({ ok: true }), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.memberSchemaChangeErrorState(null, "schema update failed"), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.memberSchemaChangeErrorState({ ok: false }, "schema update failed"), {
  hasError: true,
  text: "schema update failed",
  rawError: "schema update failed",
});
assert.deepEqual(controller.agentDefinitionAddControlState([{
  id: "reviewer",
  role: "reviewer",
  label: "Quality Reviewer",
}], hydratedCatalogs.agentView), {
  hasDefinitions: true,
  optionRows: [{
    value: "reviewer",
    label: "Quality Reviewer",
    definition: { id: "reviewer", role: "reviewer", label: "Quality Reviewer" },
  }],
  controlClass: "agents-list__add agents-list__add--select",
  disabled: false,
  title: "Create an agent from a Meta profile-member definition.",
  unavailableLabel: "agents unavailable",
  authoringOperationUnavailableError: "Meta authoring operation API is unavailable",
  placeholderOption: { value: "", label: "+ new agent..." },
  value: "",
});
assert.deepEqual(controller.schemaEditorControlState({
  schema: agentSchemasForProjection[1],
  members: agentMembersForProjection,
  schemaView: {
    eyebrow: "SCHEMA CONTRACT",
    descriptionTitle: "SUMMARY",
    descriptionPlaceholder: "Describe emitted data.",
    fieldsTitlePrefix: "COLUMNS",
    fieldsTitleTemplate: "{prefix} · {count}",
    addFieldLabel: "+ column",
    headerLabels: {
      name: "FIELD",
      type: "KIND",
      required: "NEED",
      description: "DETAIL",
      action: "",
    },
    emptyFieldsHint: "No contract fields.",
    usedByPrefix: "REFERENCED BY",
    usedByTitleTemplate: "{prefix} · {count}",
    usageSingularTemplate: "used by {count} agent",
    usagePluralTemplate: "used by {count} agents",
    emptyUsedByHint: "No agent references.",
    deleteLabel: "REMOVE",
    deleteBlockedTitle: "Clear agent schema refs first",
  },
}), {
  eyebrow: "SCHEMA CONTRACT",
  descriptionTitle: "SUMMARY",
  descriptionPlaceholder: "Describe emitted data.",
  fieldsTitle: "COLUMNS · 2",
  addFieldLabel: "+ column",
  authoringOperationUnavailableError: "",
  schemaOperationFallbackError: "",
  fieldAddFallbackError: "",
  headerLabels: {
    name: "FIELD",
    type: "KIND",
    required: "NEED",
    description: "DETAIL",
    action: "",
  },
  fieldRows: [
    { id: "f1", field: agentSchemasForProjection[1].fields[0] },
    { id: "f2", field: agentSchemasForProjection[1].fields[1] },
  ],
  emptyFieldsHint: "No contract fields.",
  usedBy: [{
    id: "m_reviewer",
    name: "Reviewer",
    role: "reviewer",
    model: "gpt-5.5",
    selection: { kind: "agent", id: "m_reviewer" },
    member: agentMembersForProjection[1],
  }],
  usedCount: 1,
  usageLabel: "used by 1 agent",
  usedByTitle: "REFERENCED BY · 1",
  emptyUsedByHint: "No agent references.",
  deleteLabel: "REMOVE",
  canDelete: false,
  deleteTitle: "Clear agent schema refs first",
});
const reskinnedSchemaEditorState = controller.schemaEditorControlState({
  schema: agentSchemasForProjection[1],
  members: agentMembersForProjection,
  schemaView: {
    ...hydratedCatalogs.schemaView,
    fieldsTitlePrefix: "PROPS",
    fieldsTitleTemplate: "{count} {prefix}",
    usedByPrefix: "REFS",
    usedByTitleTemplate: "{count} {prefix}",
    usageSingularTemplate: "{count} profile",
    usagePluralTemplate: "{count} profiles",
  },
});
assert.equal(reskinnedSchemaEditorState.fieldsTitle, "2 PROPS");
assert.equal(reskinnedSchemaEditorState.usedByTitle, "1 REFS");
assert.equal(reskinnedSchemaEditorState.usageLabel, "1 profile");
assert.equal(controller.schemaEditorControlState({
  schema: agentSchemasForProjection[1],
  members: [
    ...agentMembersForProjection,
    { id: "m_extra", name: "Extra", role: "extra", model: "gpt-5.5", schema: "ReviewArtifact" },
  ],
  schemaView: {
    ...hydratedCatalogs.schemaView,
    usagePluralTemplate: "{count} profiles",
  },
}).usageLabel, "2 profiles");

const schemaDraftContract = {
  mob_definition: {
    defaults: { schema_field_type: "enum" },
    editor_schema_field_types: ["enum", "string"],
    editor_schema_draft: {
      schema_id_prefix: "Artifact",
      initial_field: {
        name: "field_one",
        required: true,
        description: "",
        enumValues: [],
      },
      added_field: {
        name: "new_field",
        required: false,
        description: "",
        enumValues: [],
      },
    },
  },
};
assert.equal(controller.schemaDefinitionAddPatch, undefined);
assert.equal(controller.schemaDefinitionAddTransition, undefined);
assert.deepEqual(controller.schemaDescriptionPatch(" Review output.\n"), { description: " Review output.\n" });
assert.equal(controller.uniqueSchemaFieldName([
  { id: "f1", name: "field" },
  { id: "f2", name: "field_2" },
], "9 field!"), "_9_field");
assert.equal(controller.schemaFieldAddPatch, undefined);
assert.deepEqual(controller.schemaDefinitionAddErrorState({
  ok: false,
  error: "Meta schema is missing mob_definition.editor_schema_draft",
}), {
  hasError: true,
  text: "Meta schema is missing mob_definition.editor_schema_draft",
  rawError: "Meta schema is missing mob_definition.editor_schema_draft",
});
assert.deepEqual(controller.schemaDefinitionAddErrorState({ ok: true }), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.schemaDefinitionAddErrorState(null, "add_schema failed"), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.schemaFieldAddErrorState({
  ok: false,
  error: "Meta schema is missing mob_definition.editor_schema_draft",
}), {
  hasError: true,
  text: "Meta schema is missing mob_definition.editor_schema_draft",
  rawError: "Meta schema is missing mob_definition.editor_schema_draft",
});
assert.deepEqual(controller.schemaFieldAddErrorState({ ok: true }), {
  hasError: false,
  text: "",
  rawError: "",
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [{ id: "f1", name: "old", type: "string" }],
}, "f1", { name: "new" }, schemaDraftContract), {
  fields: [{ id: "f1", name: "new", type: "string" }],
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [{ id: "f1", name: "old", type: "string" }],
}, "f1", { name: "" }, {
  mob_definition: {
    defaults: { schema_field_type: "string" },
    editor_schema_field_types: ["string"],
    editor_schema_draft: {
      schema_id_prefix: "Result",
      initial_field: { name: "result_field", required: true, description: "", enumValues: [] },
      added_field: { name: "result_field", required: false, description: "", enumValues: [] },
    },
  },
}), {
  fields: [{ id: "f1", name: "result_field", type: "string" }],
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [
    { id: "f1", name: "old", type: "string" },
    { id: "f2", name: "new", type: "string" },
  ],
}, "f1", { name: "new" }, schemaDraftContract), {
  fields: [
    { id: "f1", name: "new_2", type: "string" },
    { id: "f2", name: "new", type: "string" },
  ],
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [{ id: "f1", name: "old", type: "string" }],
}, "f1", { name: "" }, schemaDraftContract), {
  fields: [{ id: "f1", name: "new_field", type: "string" }],
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [{ id: "f1", name: "old", type: "string", enumValues: [] }],
}, "f1", { type: "object" }, schemaDraftContract), {
  fields: [{ id: "f1", name: "old", type: "string", enumValues: [] }],
});
assert.deepEqual(controller.schemaFieldUpdatePatch({
  fields: [{ id: "f1", name: "old", type: "string", enumValues: [] }],
}, "f1", { type: "enum" }, schemaDraftContract), {
  fields: [{ id: "f1", name: "old", type: "enum", enumValues: ["value"] }],
});
const updatedSchemaFieldCascade = controller.schemaFieldUpdateCascadePatch({
  schema: {
    id: "ReviewArtifact",
    fields: [{ id: "f1", name: "verdict", type: "enum", enumValues: ["green", "red"] }],
  },
  schemas: [
    { id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict", type: "enum", enumValues: ["green", "red"] }] },
  ],
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  flow: {
    name: "field-update-cascade",
    steps: [
      { id: "review_step", type: "member", role: "m_reviewer" },
      {
        id: "route",
        type: "branch",
        branches: [{
          id: "br_red",
          cond: { stepId: "review_step", field: "verdict", op: "==", val: "red" },
          condition: "steps.review_step.verdict == \"red\"",
          steps: [],
        }],
        fallback: [],
      },
    ],
  },
  edges: [{
    id: "e_review_done",
    from: "review_inst",
    to: "done",
    kind: "cond",
    label: "steps.review_inst.verdict == \"red\"",
    cond: { var: "steps.review_inst.verdict", op: "==", val: "red" },
  }],
  instances: [
    { id: "review_inst", memberId: "m_reviewer" },
    { id: "done", isTerminal: true },
  ],
}, "f1", { enumValues: ["green"] }, schemaDraftContract);
assert.deepEqual(updatedSchemaFieldCascade.schema.fields[0].enumValues, ["green"]);
assert.deepEqual(updatedSchemaFieldCascade.schemas[0].fields[0].enumValues, ["green"]);
assert.deepEqual(updatedSchemaFieldCascade.flow.steps[1].branches[0].cond, {});
assert.equal(updatedSchemaFieldCascade.flow.steps[1].branches[0].condition, "");
assert.deepEqual(updatedSchemaFieldCascade.edges[0].cond, null);
assert.equal(updatedSchemaFieldCascade.edges[0].label, "");
assert.deepEqual(controller.schemaFieldDeletePatch({
  fields: [{ id: "f1", name: "old" }, { id: "f2", name: "keep" }],
}, "f1"), {
  removed: { id: "f1", name: "old" },
  patch: { fields: [{ id: "f2", name: "keep" }] },
});
const deletedSchemaFieldCascade = controller.schemaFieldDeleteCascadePatch({
  schema: {
    id: "ReviewArtifact",
    fields: [{ id: "f1", name: "verdict" }, { id: "f2", name: "summary" }],
  },
  schemas: [
    { id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict" }, { id: "f2", name: "summary" }] },
  ],
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  flow: {
    name: "field-delete-cascade",
    steps: [
      { id: "review_step", type: "member", role: "m_reviewer" },
      {
        id: "route",
        type: "branch",
        branches: [{
          id: "br_green",
          cond: { stepId: "review_step", field: "verdict", op: "==", val: "green" },
          condition: "steps.review_step.verdict == \"green\"",
          steps: [],
        }],
        fallback: [],
      },
    ],
  },
  edges: [{
    id: "e_review_done",
    from: "review_inst",
    to: "done",
    kind: "cond",
    label: "steps.review_inst.verdict == \"green\"",
    cond: { var: "steps.review_inst.verdict", op: "==", val: "green" },
  }],
  instances: [
    { id: "review_inst", memberId: "m_reviewer" },
    { id: "done", isTerminal: true },
  ],
}, "f1");
assert.deepEqual(deletedSchemaFieldCascade.removed, { id: "f1", name: "verdict" });
assert.deepEqual(deletedSchemaFieldCascade.schema.fields, [{ id: "f2", name: "summary" }]);
assert.deepEqual(deletedSchemaFieldCascade.schemas[0].fields, [{ id: "f2", name: "summary" }]);
assert.deepEqual(deletedSchemaFieldCascade.flow.steps[1].branches[0].cond, {});
assert.equal(deletedSchemaFieldCascade.flow.steps[1].branches[0].condition, "");
assert.deepEqual(deletedSchemaFieldCascade.edges[0].cond, null);
assert.equal(deletedSchemaFieldCascade.edges[0].label, "");

const renamedSchemaFieldCascade = controller.schemaFieldRenameCascadePatch({
  schema: {
    id: "ReviewArtifact",
    fields: [{ id: "f1", name: "verdict", type: "enum" }, { id: "f2", name: "summary", type: "string" }],
  },
  schemas: [
    { id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict", type: "enum" }, { id: "f2", name: "summary", type: "string" }] },
  ],
  members: [{ id: "m_reviewer", schema: "ReviewArtifact" }],
  flow: {
    name: "field-rename-cascade",
    steps: [
      { id: "review_step", type: "member", role: "m_reviewer" },
      {
        id: "route",
        type: "branch",
        branches: [{
          id: "br_green",
          cond: { stepId: "review_step", field: "verdict", op: "==", val: "green" },
          condition: "steps.review_step.verdict == \"green\"",
          steps: [],
        }],
        fallback: [],
      },
    ],
  },
  edges: [{
    id: "e_review_done",
    from: "review_inst",
    to: "done",
    kind: "cond",
    label: "steps.review_inst.verdict == \"green\"",
    cond: { var: "steps.review_inst.verdict", op: "==", val: "green" },
  }],
  instances: [
    { id: "review_inst", memberId: "m_reviewer" },
    { id: "done", isTerminal: true },
  ],
}, "f1", "outcome", "verdict", schemaDraftContract);
assert.equal(renamedSchemaFieldCascade.schema.fields[0].name, "outcome");
assert.equal(renamedSchemaFieldCascade.schemas[0].fields[0].name, "outcome");
assert.equal(renamedSchemaFieldCascade.flow.steps[1].branches[0].cond.field, "outcome");
assert.equal(renamedSchemaFieldCascade.flow.steps[1].branches[0].condition, "steps.review_step.outcome == \"green\"");
assert.deepEqual(renamedSchemaFieldCascade.edges[0].cond, { var: "steps.review_inst.outcome", op: "==", val: "green" });
assert.equal(renamedSchemaFieldCascade.edges[0].label, "steps.review_inst.outcome == \"green\"");

const directMemberContract = {
  mob_definition: {
    profile_binding: ["inline"],
    profile_binding_restrictions: {
      inline: { deployable: true },
    },
    runtime_modes: ["turn_driven", "autonomous_host"],
  },
};

assert.deepEqual(controller.studioAddMemberPatch({
  members: [{ id: "m_old" }],
}, { id: "m_new" }), {
  ok: false,
  error: "member must include id and role/name",
  members: [{ id: "m_old" }],
  member: null,
});
assert.deepEqual(controller.studioAddMemberPatch({
  members: [{ id: "m_old" }],
  contract: directMemberContract,
}, {
  id: "m_new",
  name: "New",
  role: "new",
  model: "gpt-5.5",
  profileBinding: "inline",
  runtimeMode: "turn_driven",
  tools: [],
  skills: [],
}), {
  ok: true,
  error: "",
  members: [
    { id: "m_old" },
    {
      id: "m_new",
      name: "New",
      role: "new",
      model: "gpt-5.5",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      tools: [],
      skills: [],
    },
  ],
  member: {
    id: "m_new",
    name: "New",
    role: "new",
    model: "gpt-5.5",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    tools: [],
    skills: [],
  },
});
assert.deepEqual(controller.studioAddMemberPatch({
  members: [{ id: "m_old" }],
  contract: directMemberContract,
}, {
  id: "m_old",
  name: "Duplicate",
  role: "duplicate",
  model: "gpt-5.5",
  profileBinding: "inline",
  runtimeMode: "turn_driven",
}), {
  ok: false,
  error: "member id already exists",
  members: [{ id: "m_old" }],
  member: null,
});
assert.deepEqual(controller.studioAddMemberPatch({
  members: [{ id: "m_old" }],
}, {
  id: "m_new",
  name: "New",
  role: "new",
  model: "gpt-5.5",
  profileBinding: "inline",
  runtimeMode: "turn_driven",
}), {
  ok: false,
  error: "Meta schema contract must allow deployable inline profileBinding",
  members: [{ id: "m_old" }],
  member: null,
});
assert.deepEqual(controller.studioAddMemberPatch({
  members: [{ id: "m_old" }],
  contract: {
    mob_definition: {
      profile_binding: ["inline"],
      profile_binding_restrictions: { inline: { deployable: true } },
      runtime_modes: ["turn_driven"],
    },
  },
}, {
  id: "m_new",
  name: "New",
  role: "new",
  model: "gpt-5.5",
  profileBinding: "inline",
  runtimeMode: "autonomous_host",
}), {
  ok: false,
  error: "member runtimeMode must be allowed by mob_definition.runtime_modes",
  members: [{ id: "m_old" }],
  member: null,
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_old", name: "Old" }],
}, "m_old", { name: "New" }), {
  ok: true,
  error: "",
  members: [{ id: "m_old", name: "New" }],
  member: { id: "m_old", name: "New" },
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_old", name: "Old" }],
}, "m_old", { id: "m_new" }), {
  ok: false,
  error: "member id changes must use projection reconciliation",
  members: [{ id: "m_old", name: "Old" }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
  contract: directMemberContract,
}, "m_real", { model: "" }), {
  ok: false,
  error: "inline member updates must keep a model",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
  contract: directMemberContract,
}, "m_real", { profileBinding: "realm_profile" }), {
  ok: false,
  error: "member updates must keep deployable inline profileBinding",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
  contract: directMemberContract,
}, "m_real", { runtimeMode: "daemon" }), {
  ok: false,
  error: "member updates must use a mob_definition.runtime_modes value",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
}, "m_real", { runtimeMode: "turn_driven" }), {
  ok: false,
  error: "Meta schema contract must allow deployable inline profileBinding",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
});
assert.equal(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven" }],
  contract: directMemberContract,
}, "m_real", { runtimeMode: "autonomous_host" }).ok, true);
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", tools: [] }],
}, "m_real", { tools: "shell" }), {
  ok: false,
  error: "member tools must be an array of non-empty strings",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", tools: [] }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", skills: [] }],
}, "m_real", { skills: ["mob.review", ""] }), {
  ok: false,
  error: "member skills must be an array of non-empty strings",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", skills: [] }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", providerParams: null }],
}, "m_real", { providerParams: ["bad"] }), {
  ok: false,
  error: "member providerParams must be a JSON object",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", providerParams: null }],
});
assert.deepEqual(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", maxInlinePeerNotifications: null }],
}, "m_real", { maxInlinePeerNotifications: "many" }), {
  ok: false,
  error: "member maxInlinePeerNotifications must be an integer >= -1 or blank",
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", maxInlinePeerNotifications: null }],
});
assert.equal(controller.studioUpdateMemberPatch({
  members: [{ id: "m_real", name: "Real", role: "real", model: "gpt-5.5", profileBinding: "inline", runtimeMode: "turn_driven", tools: [], skills: [], providerParams: null, maxInlinePeerNotifications: null }],
}, "m_real", { tools: ["shell"], skills: ["mob.review"], providerParams: { thinking_budget: 2048 }, maxInlinePeerNotifications: -1 }).ok, true);
assert.deepEqual(controller.studioDeleteMemberPatch({
  members: [{ id: "m_keep" }, { id: "m_drop" }],
  instances: [
    { id: "i_keep", memberId: "m_keep" },
    { id: "i_drop", memberId: "m_drop" },
    { id: "done", isTerminal: true },
  ],
  edges: [
    { id: "e_keep", from: "i_keep", to: "done" },
    { id: "e_drop_out", from: "i_drop", to: "done" },
    { id: "e_drop_in", from: "i_keep", to: "i_drop" },
  ],
}, "m_drop"), {
  members: [{ id: "m_keep" }],
  instances: [
    { id: "i_keep", memberId: "m_keep" },
    { id: "done", isTerminal: true },
  ],
  edges: [{ id: "e_keep", from: "i_keep", to: "done" }],
});
const memberDeleteCascade = controller.memberDeleteCascadePatch({
  memberId: "m_drop",
  members: [
    { id: "m_keep", name: "Keep", role: "keep", tools: ["shell"], schema: "SummaryArtifact" },
    { id: "m_drop", name: "Drop", role: "drop", tools: ["git"], schema: "ReviewArtifact" },
  ],
  flow: {
    name: "delete-cascade-proof",
    steps: [
      { id: "keep_step", type: "member", role: "m_keep", allowedTools: ["shell"] },
      { id: "drop_step", type: "member", role: "m_drop", allowedTools: ["git"] },
      {
        id: "parallel_step",
        type: "parallel",
        controllerRole: "m_drop",
        branches: [{
          id: "fork_branch",
          steps: [{ id: "fork_keep", type: "member", role: "m_keep", launchMode: { kind: "Fork", from: "drop_step" } }],
        }],
      },
    ],
  },
  instances: [
    { id: "i_keep", memberId: "m_keep", allowedTools: ["shell"] },
    { id: "i_drop", memberId: "m_drop", allowedTools: ["git"] },
    { id: "join", isGate: true, gateKind: "join", controllerRole: "m_drop" },
    { id: "fork_keep", memberId: "m_keep", launchMode: { kind: "Fork", from: "i_drop" } },
  ],
  edges: [
    { id: "keep_join", from: "i_keep", to: "join" },
    { id: "drop_join", from: "i_drop", to: "join" },
    { id: "join_fork", from: "join", to: "fork_keep" },
  ],
  mobSettings: {
    orchestrator: "drop",
    roleWiring: [{ a: "keep", b: "drop" }],
    backendDefault: "session",
  },
});
assert.equal(memberDeleteCascade.ok, true);
assert.deepEqual(memberDeleteCascade.members.map((member) => member.id), ["m_keep"]);
assert.deepEqual(memberDeleteCascade.flow.steps.map((step) => step.id), ["keep_step", "parallel_step"]);
assert(!("controllerRole" in memberDeleteCascade.flow.steps[1]));
assert.deepEqual(memberDeleteCascade.flow.steps[1].branches[0].steps[0].launchMode, { kind: "Fresh" });
assert.deepEqual(memberDeleteCascade.instances.map((instance) => instance.id), ["i_keep", "join", "fork_keep"]);
assert(!("controllerRole" in memberDeleteCascade.instances[1]));
assert.deepEqual(memberDeleteCascade.instances[2].launchMode, { kind: "Fresh" });
assert.deepEqual(memberDeleteCascade.edges.map((edge) => edge.id), ["keep_join", "join_fork"]);
assert.equal(memberDeleteCascade.mobSettings.orchestrator, "");
assert.deepEqual(memberDeleteCascade.mobSettings.roleWiring, []);
assert.equal(memberDeleteCascade.selection, null);
assert.deepEqual(controller.memberDeleteCascadePatch({ memberId: "missing", members: [{ id: "m_keep" }] }), {
  ok: false,
  error: "member not found",
  members: [{ id: "m_keep" }],
  instances: [],
  edges: [],
  flow: undefined,
  mobSettings: controller.normalizeMobSettings(undefined),
  selection: null,
});
assert.deepEqual(controller.studioUpdateInstancePatch({
  instances: [{ id: "a", memberId: "m_a", col: 1 }],
  members: [{ id: "m_a" }],
}, "a", { col: 2 }), {
  ok: true,
  error: "",
  instances: [{ id: "a", memberId: "m_a", col: 2 }],
  instance: { id: "a", memberId: "m_a", col: 2 },
});
assert.deepEqual(controller.studioUpdateInstancePatch({
  instances: [{ id: "a", memberId: "m_a", col: 1 }, { id: "b", isTerminal: true }],
  members: [{ id: "m_a" }],
}, "a", { memberId: "m_missing" }), {
  ok: false,
  error: "member graph node must reference an existing member",
  instances: [{ id: "a", memberId: "m_a", col: 1 }, { id: "b", isTerminal: true }],
});
assert.deepEqual(controller.studioUpdateInstancePatch({
  instances: [{ id: "a", memberId: "m_a", col: 1 }, { id: "b", isTerminal: true }],
  members: [{ id: "m_a" }],
}, "a", { id: "b" }), {
  ok: false,
  error: "graph node id already exists",
  instances: [{ id: "a", memberId: "m_a", col: 1 }, { id: "b", isTerminal: true }],
});
assert.deepEqual(controller.studioMoveInstancePatch({
  instances: [
    { id: "a", col: 0, row: 0 },
    { id: "b", col: 2, row: 2 },
  ],
}, "a", { col: 2, row: 2 }, { col: 0, row: 0 }), {
  instances: [
    { id: "a", col: 2, row: 2 },
    { id: "b", col: 0, row: 0 },
  ],
});
assert.deepEqual(controller.studioMoveInstancePatch({
  instances: [{ id: "a", col: 0, row: 0 }],
}, "missing", { col: 1, row: 1 }, { col: 0, row: 0 }), {
  instances: [{ id: "a", col: 0, row: 0 }],
});
assert.deepEqual(controller.studioDeleteInstancePatch({
  instances: [{ id: "a" }, { id: "b" }, { id: "c" }],
  edges: [{ id: "ab", from: "a", to: "b" }, { id: "ca", from: "c", to: "a" }, { id: "bc", from: "b", to: "c" }],
}, "a"), {
  instances: [{ id: "b" }, { id: "c" }],
  edges: [{ id: "bc", from: "b", to: "c" }],
  selection: { kind: null, id: null },
});
assert.deepEqual(controller.studioDeleteInstancePatch({
  instances: [
    { id: "source", memberId: "m_source" },
    { id: "review", memberId: "m_review", launchMode: { kind: "Fork", from: "source", context: "full_history" } },
    { id: "done", isTerminal: true },
  ],
  edges: [
    { id: "review_done", from: "review", to: "done", kind: "cond", label: 'steps.source.verdict == "green"', cond: { var: "steps.source.verdict", op: "==", val: "green" } },
    { id: "source_done", from: "source", to: "done", kind: "next" },
  ],
}, "source"), {
  instances: [
    { id: "review", memberId: "m_review", launchMode: { kind: "Fresh" } },
    { id: "done", isTerminal: true },
  ],
  edges: [
    { id: "review_done", from: "review", to: "done", kind: "cond", label: "", cond: null },
  ],
  selection: { kind: null, id: null },
});
assert.deepEqual(controller.studioUpdateEdgePatch({
  edges: [{ id: "ab", label: "" }],
  instances: [{ id: "a" }, { id: "b" }],
}, "ab", { label: "next" }), {
  ok: false,
  error: "edge must include id, from, and to",
  edges: [{ id: "ab", label: "" }],
});
assert.deepEqual(controller.studioUpdateEdgePatch({
  edges: [{ id: "ab", from: "a", to: "b", label: "" }],
  instances: [{ id: "a" }, { id: "b" }],
}, "ab", { label: "next" }), {
  ok: true,
  error: "",
  edges: [{ id: "ab", from: "a", to: "b", label: "next" }],
  edge: { id: "ab", from: "a", to: "b", label: "next" },
});
assert.deepEqual(controller.studioUpdateEdgePatch({
  edges: [
    { id: "ab", from: "a", to: "b" },
    { id: "bc", from: "b", to: "c" },
  ],
  instances: [{ id: "a" }, { id: "b" }, { id: "c" }],
}, "ab", { from: "b", to: "c" }), {
  ok: false,
  error: "edge already exists",
  edges: [
    { id: "ab", from: "a", to: "b" },
    { id: "bc", from: "b", to: "c" },
  ],
});
assert.deepEqual(controller.studioDeleteEdgePatch({
  edges: [{ id: "keep" }, { id: "drop" }],
}, "drop"), {
  edges: [{ id: "keep" }],
  selection: { kind: null, id: null },
});
assert.deepEqual(controller.studioAddSchemaPatch({
  schemas: [{ id: "PlanArtifact" }],
}, { id: "ReviewArtifact" }), {
  ok: true,
  error: "",
  schemas: [{ id: "PlanArtifact" }, { id: "ReviewArtifact" }],
  schema: { id: "ReviewArtifact" },
});
assert.deepEqual(controller.studioAddSchemaPatch({
  schemas: [{ id: "PlanArtifact" }],
}, { id: "PlanArtifact" }), {
  ok: false,
  error: "schema id already exists",
  schemas: [{ id: "PlanArtifact" }],
  schema: null,
});
assert.deepEqual(controller.studioUpdateSchemaPatch({
  schemas: [{ id: "ReviewArtifact", description: "" }],
}, "ReviewArtifact", { description: "Review output" }), {
  ok: true,
  error: "",
  schemas: [{ id: "ReviewArtifact", description: "Review output" }],
  schema: { id: "ReviewArtifact", description: "Review output" },
});
assert.deepEqual(controller.studioUpdateSchemaPatch({
  schemas: [{ id: "ReviewArtifact", description: "" }, { id: "PlanArtifact", description: "" }],
}, "ReviewArtifact", { id: "PlanArtifact" }), {
  ok: false,
  error: "schema id changes must use renameSchemaDefinition",
  schemas: [{ id: "ReviewArtifact", description: "" }, { id: "PlanArtifact", description: "" }],
});
assert.deepEqual(controller.studioDeleteSchemaPatch({
  schemas: [{ id: "ReviewArtifact" }, { id: "PlanArtifact" }],
  members: [
    { id: "m_review", schema: "ReviewArtifact" },
    { id: "m_plan", schema: "PlanArtifact" },
  ],
}, "ReviewArtifact"), {
  schemas: [{ id: "PlanArtifact" }],
  members: [
    { id: "m_review", schema: "" },
    { id: "m_plan", schema: "PlanArtifact" },
  ],
  selection: null,
});
const schemaDeleteCascade = controller.studioDeleteSchemaPatch({
  schemas: [
    { id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict", type: "enum" }] },
    { id: "PlanArtifact", fields: [{ id: "f2", name: "summary", type: "string" }] },
  ],
  members: [
    { id: "m_review", schema: "ReviewArtifact" },
    { id: "m_plan", schema: "PlanArtifact" },
  ],
  flow: {
    name: "delete-schema-cascade",
    steps: [
      { id: "review_step", type: "member", role: "m_review" },
      {
        id: "route",
        type: "branch",
        branches: [{
          id: "br_green",
          cond: { stepId: "review_step", field: "verdict", op: "==", val: "green" },
          condition: "steps.review_step.verdict == \"green\"",
          steps: [],
        }],
        fallback: [],
      },
    ],
  },
  edges: [{
    id: "e_review_done",
    from: "review_inst",
    to: "done",
    kind: "cond",
    label: "steps.review_inst.verdict == \"green\"",
    cond: { var: "steps.review_inst.verdict", op: "==", val: "green" },
  }],
  instances: [
    { id: "review_inst", memberId: "m_review", col: 0, row: 0 },
    { id: "done", isTerminal: true, col: 1, row: 0 },
  ],
}, "ReviewArtifact");
assert.deepEqual(schemaDeleteCascade.schemas, [
  { id: "PlanArtifact", fields: [{ id: "f2", name: "summary", type: "string" }] },
]);
assert.deepEqual(schemaDeleteCascade.members, [
  { id: "m_review", schema: "" },
  { id: "m_plan", schema: "PlanArtifact" },
]);
assert.equal(schemaDeleteCascade.selection, null);
assert.deepEqual(schemaDeleteCascade.flow.steps[1].branches[0].cond, {});
assert.equal(schemaDeleteCascade.flow.steps[1].branches[0].condition, "");
assert.deepEqual(schemaDeleteCascade.edges[0].cond, null);
assert.equal(schemaDeleteCascade.edges[0].label, "");

assert.deepEqual(controller.diagnosticsToRows({
  ok: true,
  display_rows: [{
    kind: "ok",
    glyph: "✓",
    head: "server row",
    sub: "Meta source",
    meta: "rkat mob validate",
  }],
  diagnostics: [{
    severity: "error",
    code: "client_fallback_must_not_win",
    message: "This diagnostic should not be converted when API rows exist.",
  }],
}), [{
  kind: "ok",
  glyph: "✓",
  head: "server row",
  sub: "Meta source",
  meta: "rkat mob validate",
}]);
assert.deepEqual(controller.diagnosticsToRows({
  ok: true,
  display_rows: [
    { kind: "ok", head: "server omitted glyph" },
    null,
  ],
}), [{
  kind: "ok",
  glyph: "",
  head: "server omitted glyph",
  sub: "",
  meta: "",
}]);

assert.deepEqual(controller.diagnosticsToRows(null), []);
assert.deepEqual(controller.diagnosticsToRows({}), []);
assert.deepEqual(controller.diagnosticsToRows({
  ok: false,
  diagnostics: [],
  validation_source: "meerkat_mob::SpecValidator",
}), []);

assert.deepEqual(controller.deployResultToRows({
  display_rows: [{
    kind: "warn",
    glyph: "△",
    head: "server deploy row",
    sub: "rkat mob run",
    meta: "/tmp/example.mobpack",
  }],
  validation: { ok: false, diagnostics: [] },
}), [{
  kind: "warn",
  glyph: "△",
  head: "server deploy row",
  sub: "rkat mob run",
  meta: "/tmp/example.mobpack",
}]);
assert.deepEqual(controller.deployResultToRows({
  display_rows: [{ glyph: "△", head: "server omitted kind" }],
}), [{
  kind: "",
  glyph: "△",
  head: "server omitted kind",
  sub: "",
  meta: "",
}]);

assert.deepEqual(controller.deployResultToRows({
  command: "rkat mob run /tmp/example.mobpack --prompt prompt",
  validation: { ok: true, diagnostics: [] },
}), []);

const validationOutcome = controller.validationOutcome({ mob_id: "validate_me" }, {
  ok: true,
  display_rows: [{ kind: "ok", glyph: "✓", head: "valid", sub: "", meta: "validate" }],
});
assert.equal(validationOutcome.stage, "valid");
assert.equal(validationOutcome.validation.ok, true);
assert.equal(validationOutcome.validationRows[0].head, "valid");

const invalidOutcome = controller.validationOutcome({ mob_id: "validate_me" }, {
  ok: false,
  display_rows: [{ kind: "crit", glyph: "!", head: "invalid", sub: "", meta: "validate" }],
});
assert.equal(invalidOutcome.stage, "draft");
assert.equal(invalidOutcome.validationRows[0].kind, "crit");
assert.deepEqual(controller.validationSheetOpenTransition(), { validate: true });
assert.deepEqual(controller.validationSheetCloseTransition(), { validate: false });
assert.deepEqual(controller.deployPlanTraceReadyTransition({ mob_id: "trace_me" }, { plan_trace: [] }), {
  deployPlanOpen: true,
  deployPlanDocument: { mob_id: "trace_me" },
  deployPlanResult: { plan_trace: [] },
  incrementDeployPlanKey: true,
});
assert.deepEqual(controller.deployPlanTraceCloseTransition(), { deployPlanOpen: false });
assert.deepEqual(controller.apiOverlayClearTransition(), {
  deployPlanOpen: false,
  validate: false,
});

const publishOutcome = controller.exportOutcome({ mob_id: "publish_me" }, {
  content_base64: "cGFjaw==",
  media_type: "application/vnd.meerkat.mobpack",
  filename: "publish-me.mobpack",
  validation: {
    ok: true,
    display_rows: [{ kind: "ok", glyph: "✓", head: "exported", sub: "", meta: "export" }],
  },
});
assert.equal(publishOutcome.stage, "published");
assert.equal(publishOutcome.validationRows[0].head, "exported");
assert.throws(
  () => controller.exportOutcome({ mob_id: "publish_me" }, {
    media_type: "application/vnd.meerkat.mobpack",
    filename: "publish-me.mobpack",
    validation: { ok: true, display_rows: [] },
  }),
  /content_base64/,
);
assert.throws(
  () => controller.exportOutcome({ mob_id: "publish_me" }, {
    content_base64: "cGFjaw==",
    filename: "publish-me.mobpack",
    validation: { ok: true, display_rows: [] },
  }),
  /media_type/,
);
assert.throws(
  () => controller.exportOutcome({ mob_id: "publish_me" }, {
    content_base64: "cGFjaw==",
    media_type: "application/vnd.meerkat.mobpack",
    validation: { ok: true, display_rows: [] },
  }),
  /filename/,
);

const rejectedPublishOutcome = controller.exportOutcome({ mob_id: "publish_me" }, {
  validation: {
    ok: false,
    display_rows: [{ kind: "crit", glyph: "!", head: "rejected", sub: "", meta: "export" }],
  },
});
assert.equal(rejectedPublishOutcome.stage, "draft");

const deployPlanOutcome = controller.deployOutcome({ mob_id: "deploy_me" }, {
  display_rows: [{ kind: "ok", glyph: "✓", head: "planned", sub: "", meta: "deploy" }],
  validation: {
    ok: true,
  },
}, { execute: false });
assert.equal(deployPlanOutcome.stage, "draft");
assert.equal(deployPlanOutcome.validationRows[0].head, "planned");

const successfulRunOutcome = controller.deployOutcome({ mob_id: "deploy_me" }, {
  executed: true,
  success: true,
  status_code: 0,
  display_rows: [{ kind: "ok", glyph: "✓", head: "deployed", sub: "", meta: "deploy" }],
  validation: {
    ok: true,
  },
}, { execute: true });
assert.equal(successfulRunOutcome.stage, "deployed");
assert.equal(successfulRunOutcome.validation.ok, true);

for (const [label, result] of [
  ["missing success", { executed: true, status_code: 0 }],
  ["missing executed", { success: true, status_code: 0 }],
  ["missing status", { executed: true, success: true }],
  ["nonzero status", { executed: true, success: true, status_code: 1 }],
  ["plan-shaped response", { command: "rkat mob run /tmp/deploy.mobpack --prompt prompt" }],
]) {
  const incompleteRunOutcome = controller.deployOutcome({ mob_id: "deploy_me" }, {
    ...result,
    validation: {
      ok: true,
    },
  }, { execute: true });
  assert.equal(incompleteRunOutcome.stage, "draft", label);
}

assert.deepEqual(controller.deployPlanTraceState({ mob_id: "deploy_me" }, {
  command: "rkat mob run /tmp/deploy.mobpack --prompt prompt",
  pack_path: "/tmp/deploy.mobpack",
  plan_trace: [
    { node: "step_1", head: "MOBPACK · deploy_me", body: "ready" },
    { node: "step_2", head: "VALIDATION · ACCEPTED", body: "ok" },
  ],
}, { deployView: TEST_DEPLOY_VIEW }), {
  steps: [
    { node: "step_1", head: "MOBPACK · deploy_me", body: "ready" },
    { node: "step_2", head: "VALIDATION · ACCEPTED", body: "ok" },
  ],
  eyebrow: "DEPLOY PLAN",
  title: "deploy_me",
  subtitle: "rkat mob run /tmp/deploy.mobpack --prompt prompt",
  packLabel: "/tmp/deploy.mobpack",
  firstLabel: "first",
  closeLabel: "×",
  stepLabel: "step",
  previousLabel: "‹",
  nextLabel: "›",
});
assert.deepEqual(controller.deployPlanTraceState({ name: "fallback mob" }, {}, { deployView: TEST_DEPLOY_VIEW }), {
  steps: [{
    node: null,
    head: "DEPLOY TRACE UNAVAILABLE",
    body: "meta/mobpacks/deploy did not return plan_trace.",
  }],
  eyebrow: "DEPLOY PLAN",
  title: "fallback mob",
  subtitle: "",
  packLabel: "",
  firstLabel: "first",
  closeLabel: "×",
  stepLabel: "step",
  previousLabel: "‹",
  nextLabel: "›",
});

const failedRunOutcome = controller.deployOutcome({ mob_id: "deploy_me" }, {
  executed: true,
  success: false,
  status_code: 1,
  display_rows: [{ kind: "crit", glyph: "!", head: "run failed", sub: "", meta: "deploy" }],
  validation: {
    ok: true,
  },
}, { execute: true });
assert.equal(failedRunOutcome.stage, "draft");
assert.equal(failedRunOutcome.validation.ok, true);

const deployPlanError = controller.deployErrorOutcome(new Error("planner offline"), { execute: false, errorView: hydratedCatalogs.errorView });
assert.equal(deployPlanError.stage, "draft");
assert.deepEqual(deployPlanError.validationRows[0], {
  kind: "crit",
  glyph: "!",
  head: "Deploy plan failed",
  sub: "planner offline",
  meta: "meta/mobpacks/deploy",
});

const deployRunError = controller.deployErrorOutcome(new Error("runner offline"), { execute: true, errorView: hydratedCatalogs.errorView });
assert.equal(deployRunError.validationRows[0].head, "Deploy failed");
assert.equal(deployRunError.validationRows[0].meta, "meta/mobpacks/deploy");

assert.deepEqual(controller.validationSheetState([
  { kind: "ok", head: "mob" },
  { kind: "warn", head: "signature" },
  { kind: "crit", head: "missing profile" },
  { kind: "", head: "unclassified" },
], { deployView: TEST_DEPLOY_VIEW }), {
  rows: [
    { kind: "ok", head: "mob" },
    { kind: "warn", head: "signature" },
    { kind: "crit", head: "missing profile" },
    { kind: "", head: "unclassified" },
  ],
  counts: { ok: 1, warn: 2, crit: 1 },
  eyebrow: "VALIDATE · Meta",
  title: "1 passed · 2 warnings · 1 blocking",
  publishLabel: "PUBLISH",
  deployPlanLabel: "DEPLOY PLAN",
  deployLabel: "DEPLOY",
  closeLabel: "×",
  actionsDisabled: true,
  publishDisabled: true,
  deployPlanDisabled: true,
  deployRunDisabled: true,
});
assert.equal(controller.validationSheetState([{ kind: "ok" }, { kind: "warn" }], { deployView: TEST_DEPLOY_VIEW }).actionsDisabled, false);
assert.equal(controller.validationSheetState([{ kind: "ok" }], { stage: "draft", deployView: TEST_DEPLOY_VIEW }).actionsDisabled, true);
assert.equal(controller.validationSheetState([{ kind: "ok" }], { stage: "valid", deployView: TEST_DEPLOY_VIEW }).actionsDisabled, false);
assert.equal(controller.validationSheetState([{ kind: "crit" }], { stage: "valid", deployView: TEST_DEPLOY_VIEW }).actionsDisabled, true);
const validationSheetHostDeployBlocked = controller.validationSheetState([{ kind: "ok" }], {
  stage: "valid",
  deployView: TEST_DEPLOY_VIEW,
  capabilities: { authoring_capabilities: { deploy_execute_allowed: false } },
});
assert.equal(validationSheetHostDeployBlocked.publishDisabled, false);
assert.equal(validationSheetHostDeployBlocked.deployPlanDisabled, false);
assert.equal(validationSheetHostDeployBlocked.deployRunDisabled, true);

assert.deepEqual(controller.sourceErrorOutcome(new Error("missing toml"), { errorView: hydratedCatalogs.errorView }).validationRows[0], {
  kind: "crit",
  glyph: "!",
  head: "Source render failed",
  sub: "missing toml",
  meta: "meta/mobpacks/source",
});
assert.equal(controller.validationErrorOutcome(new Error("rpc down"), { errorView: hydratedCatalogs.errorView }).validationRows[0].head, "Meta API unavailable");
assert.equal(controller.validationErrorOutcome(new Error("rpc down"), { errorView: hydratedCatalogs.errorView }).validationRows[0].meta, "/flow-editor/rpc");
assert.equal(controller.exportErrorOutcome(new Error("pack failed"), { errorView: hydratedCatalogs.errorView }).validationRows[0].head, "Export failed");
assert.equal(controller.exportErrorOutcome(new Error("pack failed"), { errorView: hydratedCatalogs.errorView }).validationRows[0].meta, "/flow-editor/rpc");
assert.deepEqual(controller.importErrorOutcome(new Error("bad archive"), { filename: "bad.mobpack", errorView: hydratedCatalogs.errorView }).validationRows[0], {
  kind: "crit",
  glyph: "!",
  head: "Import failed",
  sub: "bad archive",
  meta: "bad.mobpack",
});
// The ?open=<missing id> boot intent surfaces the standard failure sheet
// with schema-backed labels and the requested id as the detail row.
assert.deepEqual(controller.bootIntentOpenFailureOutcome({ kind: "open", id: "f_missing", embedded: false }, { errorView: hydratedCatalogs.errorView }), {
  validationRows: [{
    kind: "crit",
    glyph: "!",
    head: "Mob not found",
    sub: "f_missing",
    meta: "flow-editor?open",
  }],
  stage: "draft",
});
assert.deepEqual(controller.deployErrorOutcome(new Error("no plan"), {
  execute: false,
  errorView: { ...hydratedCatalogs.errorView, deployPlanFailedHead: "Plan API failed", deployErrorMeta: "deploy-meta" },
}).validationRows[0], {
  kind: "crit",
  glyph: "!",
  head: "Plan API failed",
  sub: "no plan",
  meta: "deploy-meta",
});

assert.deepEqual(controller.sourceProjectionClearTransition(), {
  sourceOpen: false,
  sourceDocument: null,
  inlineSourceOpen: false,
  inlineSourceSurface: null,
  inlineSourceDocument: null,
  inlineSourceBusy: false,
});
const readySourceDocument = { filename: "ready.mobpack" };
assert.deepEqual(controller.sourceDrawerReadyTransition(readySourceDocument), {
  sourceOpen: true,
  sourceDocument: readySourceDocument,
});
assert.deepEqual(controller.sourceDrawerReadyTransition(null), {
  sourceOpen: false,
  sourceDocument: null,
});
assert.deepEqual(controller.inlineSourcePendingTransition("graph"), {
  inlineSourceOpen: true,
  inlineSourceSurface: "graph",
  inlineSourceBusy: true,
});
assert.deepEqual(controller.inlineSourceToggleTransition({
  open: false,
  currentSurface: "",
  targetSurface: "basic",
}), {
  shouldOpen: true,
  patch: {
    inlineSourceOpen: true,
    inlineSourceSurface: "basic",
    inlineSourceBusy: true,
  },
});
assert.deepEqual(controller.inlineSourceToggleTransition({
  open: true,
  currentSurface: "basic",
  targetSurface: "basic",
}), {
  shouldOpen: false,
  patch: {
    sourceOpen: false,
    sourceDocument: null,
    inlineSourceOpen: false,
    inlineSourceSurface: null,
    inlineSourceDocument: null,
    inlineSourceBusy: false,
  },
});
assert.deepEqual(controller.inlineSourceToggleButtonState({
  open: false,
  currentSurface: "",
  targetSurface: "basic",
  basicView: hydratedCatalogs.basicView,
  sourceView: hydratedCatalogs.sourceView,
}), {
  active: false,
  label: "{ } mob.toml",
});
assert.deepEqual(controller.inlineSourceToggleButtonState({
  open: true,
  currentSurface: "basic",
  targetSurface: "basic",
  basicView: hydratedCatalogs.basicView,
  sourceView: hydratedCatalogs.sourceView,
}), {
  active: true,
  label: "×",
});
assert.deepEqual(controller.inlineSourceReadyTransition(readySourceDocument), {
  inlineSourceDocument: readySourceDocument,
  inlineSourceBusy: false,
});
assert.deepEqual(controller.inlineSourceBusyTransition(false), {
  inlineSourceBusy: false,
});
assert.equal(controller.inlineSourceRequestPath({
  id: "source_mob_toml",
  kind: "source",
}, {
  graphView: hydratedCatalogs.graphView,
  sourceView: hydratedCatalogs.sourceView,
}), "meta/mob.toml");
assert.equal(controller.inlineSourceRequestPath({
  sourcePath: "definition.json",
}, {
  graphView: hydratedCatalogs.graphView,
  sourceView: hydratedCatalogs.sourceView,
}), "definition.json");

const sourceProjection = controller.sourceDocumentFromSourceResult({
  name: "Source Proof",
  mob_id: "source_proof",
  mob_toml: "[stale]",
}, {
  source: "meta/mobpacks/source",
  mob_toml: "[top-level stale export text]",
  source_files: [{
    path: "meta/mob.toml",
    media_type: "text/toml",
    size_bytes: 26,
    content_base64: "unused",
    sha256: "2496e694a40dca7b5f535d6cb5969d867b6989aa9fa258535c048947073086fa",
    text: "[mob]\nid = \"source_proof\"\n",
  }, {
    path: "definition.json",
    media_type: "application/json",
    size_bytes: 31,
    content_base64: "unused",
    sha256: "json-sha",
    text: "{\"mob\":{\"id\":\"source_proof\"}}\n",
  }],
  filename: "source-proof.mobpack",
  media_type: "application/vnd.meta.mobpack",
  validation: {
    ok: true,
    display_rows: [{
      kind: "ok",
      glyph: "✓",
      head: "exported",
      sub: "mob.toml",
      meta: "meta/mobpacks/export",
    }],
  },
}, {
  sourceView: hydratedCatalogs.sourceView,
});
assert.equal(sourceProjection.document.name, "Source Proof");
assert.equal(sourceProjection.document.mob_toml, "[stale]");
assert.notEqual(sourceProjection.document.mob_toml, "[top-level stale export text]");
assert.equal(sourceProjection.sourceDocument.filename, "source-proof.mobpack");
assert.equal(sourceProjection.sourceDocument.media_type, "application/vnd.meta.mobpack");
assert.equal(sourceProjection.sourceDocument.sourcePath, "meta/mob.toml");
assert.equal(sourceProjection.sourceDocument.sourceFile.media_type, "text/toml");
assert.equal(sourceProjection.sourceDocument.sourceFile.text, "[mob]\nid = \"source_proof\"\n");
assert.equal(sourceProjection.sourceDocument.sourceFiles.length, 2);
assert.equal(sourceProjection.sourceDocument.sourceDigest, "2496e694a40dca7b5f535d6cb5969d867b6989aa9fa258535c048947073086fa");
assert.equal(sourceProjection.sourceDocument.source, "meta/mobpacks/source");
assert.deepEqual(sourceProjection.sourceDocument.sourceView, hydratedCatalogs.sourceView);
assert.equal(sourceProjection.sourceDocument.validation.ok, true);
assert.equal(sourceProjection.validationRows[0].head, "exported");
assert.equal(sourceProjection.stage, "valid");
assert.deepEqual(controller.sourceEditorState(sourceProjection.sourceDocument), {
  source: "[mob]\nid = \"source_proof\"\n",
  sourceHtml: '<span class="toml-table">[mob]</span>\n<span class="toml-key">id</span> = "source_proof"\n',
  drawerEyebrow: "SOURCE · mob.toml",
  inlineTitle: "mob.toml",
  sourceLabel: "meta/mobpacks/source · meta/mob.toml · source-proof.mobpack · application/vnd.meta.mobpack",
  validationSource: "",
  bodyClass: "source-drawer__body",
  selectedPath: "meta/mob.toml",
  fileRows: [
    {
      path: "meta/mob.toml",
      label: "meta/mob.toml",
      value: "meta/mob.toml",
      selected: true,
      className: "source-file-row is-selected",
      meta: "text/toml · 26b",
      file: sourceProjection.sourceDocument.sourceFiles[0],
    },
    {
      path: "definition.json",
      label: "definition.json",
      value: "definition.json",
      selected: false,
      className: "source-file-row",
      meta: "application/json · 31b",
      file: sourceProjection.sourceDocument.sourceFiles[1],
    },
  ],
  showLoading: false,
  loadingText: "rendering mob.toml from meta/mobpacks/source...",
  copyLabel: "copy",
  closeLabel: "×",
  copyDisabled: false,
});
const selectedDefinitionSource = controller.sourceEditorState(sourceProjection.sourceDocument, {
  sourcePath: "definition.json",
});
assert.equal(selectedDefinitionSource.source, "{\"mob\":{\"id\":\"source_proof\"}}\n");
assert.equal(selectedDefinitionSource.sourceHtml, "{\"mob\":{\"id\":\"source_proof\"}}\n");
assert.equal(selectedDefinitionSource.sourceLabel, "meta/mobpacks/source · definition.json · source-proof.mobpack · application/vnd.meta.mobpack");
assert.equal(selectedDefinitionSource.selectedPath, "definition.json");
assert.deepEqual(selectedDefinitionSource.fileRows.map((row) => [row.path, row.selected]), [
  ["meta/mob.toml", false],
  ["definition.json", true],
]);
assert.deepEqual(controller.sourceFileSelectionTransition(sourceProjection.sourceDocument, "definition.json"), {
  sourcePath: "definition.json",
});
assert.deepEqual(controller.sourceFileSelectionTransition(sourceProjection.sourceDocument, "missing.toml", "definition.json"), {
  sourcePath: "definition.json",
});
assert.deepEqual(controller.sourceFileSelectionTransition(sourceProjection.sourceDocument, "missing.toml", ""), {
  sourcePath: "meta/mob.toml",
});
const escapedSourceEditorState = controller.sourceEditorState({
  sourcePath: "meta/mob.toml",
  sourceFiles: [{
    path: "meta/mob.toml",
    media_type: "text/toml",
    text: "# <unsafe & comment>\n[mob]\nid = \"safe\"\n",
    size_bytes: 39,
  }],
}, { sourceView: hydratedCatalogs.sourceView });
assert.equal(
  escapedSourceEditorState.sourceHtml,
  '<span class="toml-comment"># &lt;unsafe &amp; comment&gt;</span>\n<span class="toml-table">[mob]</span>\n<span class="toml-key">id</span> = "safe"\n',
);
assert.deepEqual(controller.sourceEditorState({
  mob_toml: "# stale fallback must stay hidden\n",
}, { sourceView: hydratedCatalogs.sourceView }), {
  source: "",
  sourceHtml: "",
  drawerEyebrow: "SOURCE · mob.toml",
  inlineTitle: "mob.toml",
  sourceLabel: "",
  validationSource: "",
  bodyClass: "source-drawer__body",
  selectedPath: "",
  fileRows: [],
  showLoading: false,
  loadingText: "rendering mob.toml from meta/mobpacks/source...",
  copyLabel: "copy",
  closeLabel: "×",
  copyDisabled: true,
});
assert.deepEqual(controller.sourceEditorState(null, { busy: true, compact: true, sourceView: hydratedCatalogs.sourceView }), {
  source: "",
  sourceHtml: "",
  drawerEyebrow: "SOURCE · mob.toml",
  inlineTitle: "mob.toml",
  sourceLabel: "",
  validationSource: "",
  bodyClass: "bld-toml__body",
  selectedPath: "",
  fileRows: [],
  showLoading: true,
  loadingText: "rendering mob.toml from meta/mobpacks/source...",
  copyLabel: "copy",
  closeLabel: "×",
  copyDisabled: true,
});
assert.deepEqual(controller.sourceEditorState(null, { busy: true, compact: true }), {
  source: "",
  sourceHtml: "",
  drawerEyebrow: "",
  inlineTitle: "",
  sourceLabel: "",
  validationSource: "",
  bodyClass: "bld-toml__body",
  selectedPath: "",
  fileRows: [],
  showLoading: true,
  loadingText: "",
  copyLabel: "",
  closeLabel: "",
  copyDisabled: true,
});
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "Export result" }, {
    source: "meta/mobpacks/export",
    filename: "export-result.mobpack",
    media_type: "application/vnd.meta.mobpack",
    content_base64: "unused",
    source_files: [{
      path: "meta/mob.toml",
      text: "[mob]\nid = \"export_result\"\n",
      sha256: "sha",
    }],
  }),
  /source preview expected meta\/mobpacks\/source but received meta\/mobpacks\/export/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No files" }, { source_files: [] }),
  /source preview expected meta\/mobpacks\/source but received\s*$/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "Missing source" }, {
    source_files: [{
      path: "meta/mob.toml",
      media_type: "text/toml",
      size_bytes: 26,
      content_base64: "dG9tbA==",
      sha256: "toml-sha",
      text: "[mob]\nid = \"missing_source\"\n",
    }],
    filename: "missing-source.mobpack",
    media_type: "application/vnd.meta.mobpack",
  }),
  /source preview expected meta\/mobpacks\/source but received\s*$/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No TOML file" }, {
    source: "meta/mobpacks/source",
    source_files: [{ path: "manifest.toml", text: "name = \"missing\"" }],
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source did not return primary source file meta\/mob\.toml/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No TOML text" }, {
    source: "meta/mobpacks/source",
    source_files: [{ path: "meta/mob.toml", text: "" }],
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source did not return primary source text meta\/mob\.toml/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No filename" }, {
    source: "meta/mobpacks/source",
    mob_toml: "[mob]\nid = \"no_filename\"\n",
    source_files: [{ path: "meta/mob.toml", text: "[mob]\nid = \"no_filename\"\n" }],
    media_type: "application/vnd.meta.mobpack",
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source did not return filename/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No media" }, {
    source: "meta/mobpacks/source",
    mob_toml: "[mob]\nid = \"no_media\"\n",
    source_files: [{ path: "meta/mob.toml", text: "[mob]\nid = \"no_media\"\n" }],
    filename: "no-media.mobpack",
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source did not return media_type/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No sha" }, {
    source: "meta/mobpacks/source",
    mob_toml: "[mob]\nid = \"no_sha\"\n",
    source_files: [{ path: "meta/mob.toml", text: "[mob]\nid = \"no_sha\"\n" }],
    filename: "no-sha.mobpack",
    media_type: "application/vnd.meta.mobpack",
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source did not return primary source sha256 meta\/mob.toml/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No aux content" }, {
    source: "meta/mobpacks/source",
    source_files: [{
      path: "meta/mob.toml",
      media_type: "text/toml",
      size_bytes: 27,
      content_base64: "dG9tbA==",
      sha256: "toml-sha",
      text: "[mob]\nid = \"no_aux_content\"\n",
    }, {
      path: "definition.json",
      media_type: "application/json",
      size_bytes: 2,
      sha256: "json-sha",
      text: "{}",
    }],
    filename: "no-aux-content.mobpack",
    media_type: "application/vnd.meta.mobpack",
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source source_files\[1\] did not return content_base64/,
);
assert.throws(
  () => controller.sourceDocumentFromSourceResult({ name: "No aux text" }, {
    source: "meta/mobpacks/source",
    source_files: [{
      path: "meta/mob.toml",
      media_type: "text/toml",
      size_bytes: 24,
      content_base64: "dG9tbA==",
      sha256: "toml-sha",
      text: "[mob]\nid = \"no_aux_text\"\n",
    }, {
      path: "definition.json",
      media_type: "application/json",
      size_bytes: 2,
      content_base64: "e30=",
      sha256: "json-sha",
    }],
    filename: "no-aux-text.mobpack",
    media_type: "application/vnd.meta.mobpack",
  }, { sourceView: hydratedCatalogs.sourceView }),
  /meta\/mobpacks\/source source_files\[1\] did not return text/,
);

assert.deepEqual(controller.exportDownloadPayload({
  content_base64: "YWJj",
  media_type: "application/vnd.meta.mobpack",
  filename: "source-proof.mobpack",
}), {
  contentBase64: "YWJj",
  mediaType: "application/vnd.meta.mobpack",
  filename: "source-proof.mobpack",
});
assert.throws(
  () => controller.exportDownloadPayload({ media_type: "application/vnd.meta.mobpack", filename: "missing-content.mobpack" }),
  /meta\/mobpacks\/export did not return content_base64/,
);
assert.throws(
  () => controller.exportDownloadPayload({ content_base64: "YWJj", filename: "missing-media.mobpack" }),
  /meta\/mobpacks\/export did not return media_type/,
);
assert.throws(
  () => controller.exportDownloadPayload({ content_base64: "YWJj", media_type: "application/vnd.meta.mobpack" }),
  /meta\/mobpacks\/export did not return filename/,
);

assert.deepEqual(controller.importParamsFromDecodedFile({
  filename: "mob.toml",
  mediaType: "text/toml",
  text: "[mob]\nid = \"docs\"\n",
}), {
  source_name: "mob.toml",
  source_media_type: "text/toml",
  mob_toml: "[mob]\nid = \"docs\"\n",
});

assert.deepEqual(controller.importParamsFromDecodedFile({
  filename: "editor.json",
  mediaType: "application/json",
  text: "{\"document\":{\"mob_id\":\"docs\"},\"source_name\":\"stale.json\"}",
}), {
  source_name: "editor.json",
  source_media_type: "application/json",
  document: { mob_id: "docs" },
});

assert.deepEqual(controller.importParamsFromDecodedFile({
  filename: "editor.json",
  mediaType: "application/json",
  kind: "json",
  parsedJson: {
    source_name: "stale.json",
    source_media_type: "stale/type",
    document: { mob_id: "docs" },
  },
}), {
  source_name: "editor.json",
  source_media_type: "application/json",
  document: { mob_id: "docs" },
});

assert.deepEqual(controller.importParamsFromDecodedFile({
  filename: "array.json",
  mediaType: "application/json",
  kind: "json",
  parsedJson: [{ mob_id: "invalid-array" }],
}), {
  source_name: "array.json",
  source_media_type: "application/json",
  document: [{ mob_id: "invalid-array" }],
});

assert.deepEqual(controller.importParamsFromDecodedFile({
  filename: "docs.mobpack",
  mediaType: "application/vnd.meta.mobpack",
  kind: "binary",
  contentBase64: "YWJj",
}), {
  source_name: "docs.mobpack",
  source_media_type: "application/vnd.meta.mobpack",
  content_base64: "YWJj",
});

assert.throws(
  () => controller.importParamsFromDecodedFile({
    filename: "broken.json",
    mediaType: "application/json",
    text: "{nope",
  }),
  /broken\.json is not valid JSON/,
);

assert.deepEqual(controller.mobDefaultsFromSchema({
  mob_definition: {
    mob_settings: {
      defaults: {
        orchestrator: "planner",
        autoWireOrchestrator: true,
        roleWiring: [{ a: "planner", b: "coder" }],
        backendDefault: "sidecar",
        externalAddressBase: "http://127.0.0.1:9000",
        advanced: {
          topology: { kind: "mesh" },
          supervisor: { profile: "planner" },
          limits: { max_members: 4 },
          spawnPolicy: { mode: "manual" },
          eventRouter: { mode: "direct" },
        },
      },
    },
  },
}), {
  orchestrator: "planner",
  autoWireOrchestrator: true,
  roleWiring: [{ a: "planner", b: "coder" }],
  backendDefault: "sidecar",
  externalAddressBase: "http://127.0.0.1:9000",
  advanced: {
    topology: { kind: "mesh" },
    supervisor: { profile: "planner" },
    limits: { max_members: 4 },
    spawnPolicy: { mode: "manual" },
    eventRouter: { mode: "direct" },
  },
});

const previousProfileMembers = [
  { id: "m_planner", name: "Planner", role: "planner" },
  { id: "m_reviewer", name: "Reviewer", role: "reviewer" },
];
const renamedProfileMembers = [
  { id: "m_planner", name: "Planner", role: "planner" },
  { id: "m_reviewer", name: "Lead Reviewer", role: "reviewer" },
];
const renamedMobSettings = controller.reconcileMobSettingsProfiles({
  orchestrator: "reviewer",
  roleWiring: [{ a: "planner", b: "reviewer" }],
  backendDefault: "session",
}, previousProfileMembers, renamedProfileMembers);
assert.equal(renamedMobSettings.orchestrator, "lead_reviewer");
assert.deepEqual(renamedMobSettings.roleWiring, [{ a: "planner", b: "lead_reviewer" }]);

const deletedProfileMobSettings = controller.reconcileMobSettingsProfiles({
  orchestrator: "lead_reviewer",
  roleWiring: [{ a: "planner", b: "lead_reviewer" }],
  backendDefault: "session",
}, renamedProfileMembers, [{ id: "m_planner", name: "Planner", role: "planner" }]);
assert.equal(deletedProfileMobSettings.orchestrator, "");
assert.deepEqual(deletedProfileMobSettings.roleWiring, []);

const graphMembers = [
  { id: "m_left", name: "Left", role: "left", model: "gpt-5.5", tools: ["builtins"], skills: [] },
  { id: "m_right", name: "Right", role: "right", model: "gpt-5.5", tools: ["builtins"], skills: [] },
  { id: "m_review", name: "Review", role: "review", model: "gpt-5.5", tools: ["builtins"], skills: [], schema: "ReviewArtifact" },
];
const branchGraphEdgeContract = {
  mob_definition: {
    defaults: {
      graph_edge_kind: "next",
      graph_condition_edge_kind: "cond",
      graph_fanout_edge_kind: "fanout",
    },
    graph_edge_kinds: ["next", "cond", "fanout"],
  },
};

const branchFlow = controller.graphToFlow({
  previousFlow: {
    name: "main",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Route.",
      inputParams: [{ id: "p1", name: "kind", type: "string", required: true }],
    }],
  },
  members: graphMembers,
  instances: [
    { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
    { id: "j_branch_route", isGate: true, gateKind: "join", collection: "any", controllerRole: "m_review", col: 2, row: 0 },
  ],
  edges: [
    { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "docs", cond: { source: "params.kind", op: "==", value: "docs" } },
    { id: "e2", from: "g_branch_route", to: "right", kind: "cond", label: "code", cond: { namespace: "params", stepId: "params", field: "kind", op: "==", val: "code" } },
    { id: "e3", from: "left", to: "j_branch_route", kind: "next", label: "" },
    { id: "e4", from: "right", to: "j_branch_route", kind: "next", label: "" },
  ],
  contract: branchGraphEdgeContract,
});

const branchStep = branchFlow.steps[1];
assert.equal(branchStep.type, "branch");
assert.equal(branchStep.controllerRole, "m_review");
assert.deepEqual(branchStep.branches.map((branch) => branch.cond), [
  { namespace: "params", stepId: "params", field: "kind", op: "==", val: "docs" },
  { namespace: "params", stepId: "params", field: "kind", op: "==", val: "code" },
]);

const branchDocument = controller.buildDocument({
  flow: branchFlow,
  studio: {
    members: graphMembers,
    schemas: [],
    instances: [
      { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
      { id: "left", memberId: "m_left", col: 1, row: 0 },
      { id: "right", memberId: "m_right", col: 1, row: 1 },
      { id: "j_branch_route", isGate: true, gateKind: "join", collection: "any", controllerRole: "m_review", col: 2, row: 0 },
    ],
    edges: [
      { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "docs", cond: { source: "params.kind", op: "==", value: "docs" } },
      { id: "e2", from: "g_branch_route", to: "right", kind: "cond", label: "code", cond: { namespace: "params", stepId: "params", field: "kind", op: "==", val: "code" } },
      { id: "e3", from: "left", to: "j_branch_route", kind: "next", label: "" },
      { id: "e4", from: "right", to: "j_branch_route", kind: "next", label: "" },
    ],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "branch-projection-proof" },
  deploySettings: testDeploySettings(),
  contract: branchGraphEdgeContract,
});

assert.deepEqual(branchDocument.edges.slice(0, 2).map((edge) => edge.cond), [
  { var: "params.kind", op: "==", val: "docs" },
  { var: "params.kind", op: "==", val: "code" },
]);
assert(branchDocument.frames.some((frame) => frame.id === "frame_branch_route" && frame.kind === "Branch"));

const branchDocumentWithMissingEdgeKind = controller.buildDocument({
  flow: branchFlow,
  studio: {
    members: graphMembers,
    schemas: [],
    instances: [
      { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
      { id: "left", memberId: "m_left", col: 1, row: 0 },
      { id: "right", memberId: "m_right", col: 1, row: 1 },
      { id: "j_branch_route", isGate: true, gateKind: "join", collection: "any", controllerRole: "m_review", col: 2, row: 0 },
    ],
    edges: [
      { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "docs", cond: { source: "params.kind", op: "==", value: "docs" } },
      { id: "e2", from: "g_branch_route", to: "right", kind: "cond", label: "code", cond: { namespace: "params", stepId: "params", field: "kind", op: "==", val: "code" } },
      { id: "stale_missing_kind", from: "left", to: "j_branch_route", label: "" },
      { id: "e4", from: "right", to: "j_branch_route", kind: "next", label: "" },
    ],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "branch-missing-kind-proof" },
  deploySettings: testDeploySettings(),
  contract: branchGraphEdgeContract,
});
assert(!branchDocumentWithMissingEdgeKind.edges.some((edge) => edge.id === "stale_missing_kind"));
assert(branchDocumentWithMissingEdgeKind.edges.some((edge) => edge.from === "left" && edge.to === "j_branch_route" && edge.kind === "next"));

const basicConditionOptionMembers = [
  { id: "m_plan", name: "Planner", schema: "PlanArtifact" },
  { id: "m_review", name: "Reviewer", schema: "ReviewArtifact" },
  { id: "m_future", name: "Future", schema: "FutureArtifact" },
];
const basicConditionOptionFlow = {
  name: "basic-condition-options-proof",
  steps: [
    {
      id: "input_1",
      type: "input",
      task: "Route.",
      inputParams: [{ id: "p1", name: "route", type: "enum", enumValues: ["docs", "code"], required: true }],
    },
    { id: "plan_step", type: "member", role: "m_plan" },
    {
      id: "branch_1",
      type: "branch",
      branches: [{
        id: "br_a",
        label: "A",
        steps: [
          { id: "review_step", type: "member", role: "m_review" },
          {
            id: "nested_branch",
            type: "branch",
            branches: [{ id: "nested_a", label: "Nested A", steps: [] }],
            fallback: [],
          },
        ],
      }],
      fallback: [{ id: "future_step", type: "member", role: "m_future" }],
    },
  ],
};
assert.deepEqual(controller.basicConditionOptions(
  basicConditionOptionFlow,
  "branch_1",
  basicConditionOptionMembers,
  { ...hydratedCatalogs.basicView, inputParamSourceLabel: "Runtime input" },
).map((option) => ({
  stepId: option.stepId,
  namespace: option.namespace,
  label: option.label,
  member: option.member?.id || "",
  fields: (option.fields || []).map((field) => field.name),
})), [
  { stepId: "params", namespace: "params", label: "Runtime input", member: "", fields: ["route"] },
  { stepId: "plan_step", namespace: "steps", label: "Planner", member: "m_plan", fields: [] },
]);
assert.deepEqual(controller.basicConditionOptions(
  basicConditionOptionFlow,
  "nested_branch",
  basicConditionOptionMembers,
  hydratedCatalogs.basicView,
).map((option) => ({
  stepId: option.stepId,
  namespace: option.namespace,
  member: option.member?.id || "",
})), [
  { stepId: "params", namespace: "params", member: "" },
  { stepId: "plan_step", namespace: "steps", member: "m_plan" },
  { stepId: "review_step", namespace: "steps", member: "m_review" },
]);
assert.deepEqual(controller.basicConditionOptions(
  basicConditionOptionFlow,
  "future_step",
  basicConditionOptionMembers,
  hydratedCatalogs.basicView,
).map((option) => option.member?.id).filter(Boolean), ["m_plan"]);

const paramReferenceFlow = {
  name: "param-reference-proof",
  steps: [
    {
      id: "input_1",
      type: "input",
      task: "Route.",
      inputParams: [{ id: "p1", name: "kind", type: "string", required: true }],
    },
    {
      id: "branch_1",
      type: "branch",
      branches: [
        {
          id: "br_docs",
          label: "Docs",
          cond: { namespace: "params", stepId: "params", field: "kind", op: "==", val: "docs" },
          condition: "params.kind == \"docs\"",
          steps: [],
        },
        {
          id: "br_code",
          label: "Code",
          condition: "params.kind == \"code\"",
          steps: [],
        },
      ],
      fallback: [],
    },
  ],
};
const paramReferenceEdges = [
  { id: "e_param_1", from: "g_branch", to: "left", kind: "cond", label: "params.kind == \"docs\"", cond: { var: "params.kind", op: "==", val: "docs" } },
  { id: "e_param_2", from: "g_branch", to: "right", kind: "cond", label: "code", cond: { namespace: "params", stepId: "params", field: "kind", op: "==", val: "code" } },
];
const renamedParamReferences = controller.reconcileInputParamReferences({
  flow: paramReferenceFlow,
  edges: paramReferenceEdges,
  oldName: "kind",
  newName: "category",
});
assert.equal(renamedParamReferences.flow.steps[1].branches[0].cond.field, "category");
assert.equal(renamedParamReferences.flow.steps[1].branches[0].condition, "params.category == \"docs\"");
assert.equal(renamedParamReferences.flow.steps[1].branches[1].condition, "params.category == \"code\"");
assert.deepEqual(renamedParamReferences.edges.map(edge => edge.cond), [
  { var: "params.category", op: "==", val: "docs" },
  { var: "params.category", op: "==", val: "code" },
]);
assert.equal(renamedParamReferences.edges[0].label, "params.category == \"docs\"");
assert.equal(renamedParamReferences.edges[1].label, "code");

const deletedParamReferences = controller.reconcileInputParamReferences({
  flow: renamedParamReferences.flow,
  edges: renamedParamReferences.edges,
  oldName: "category",
  newName: "",
});
assert.deepEqual(deletedParamReferences.flow.steps[1].branches[0].cond, {});
assert.equal(deletedParamReferences.flow.steps[1].branches[0].condition, "");
assert.equal(deletedParamReferences.flow.steps[1].branches[1].condition, "");
assert.deepEqual(deletedParamReferences.edges.map(edge => edge.cond), [null, null]);
assert.deepEqual(deletedParamReferences.edges.map(edge => edge.label), ["", ""]);

const generatedConditionEdge = {
  id: "e_generated_condition",
  from: "review",
  to: "finish",
  kind: "cond",
  label: "",
  cond: { var: "steps.review.verdict", op: "==", val: "green" },
};
const generatedConditionPatch = controller.graphEdgeConditionPatch(generatedConditionEdge, { val: "red" });
assert.deepEqual(generatedConditionPatch.cond, { var: "steps.review.verdict", op: "==", val: "red" });
assert.equal(generatedConditionPatch.label, "steps.review.verdict == \"red\"");

const missingOperatorConditionPatch = controller.graphEdgeConditionPatch({
  id: "e_missing_operator",
  from: "review",
  to: "finish",
  kind: "cond",
  label: "",
  cond: { var: "steps.review.verdict", val: "green" },
}, { val: "red" });
assert.deepEqual(missingOperatorConditionPatch.cond, { var: "steps.review.verdict", op: "", val: "red" });
assert.equal(missingOperatorConditionPatch.label, "");

const schemaDefaultConditionPatch = controller.graphEdgeConditionPatch({
  id: "e_schema_default_operator",
  from: "review",
  to: "finish",
  kind: "cond",
  label: "",
  cond: { var: "steps.review.verdict", val: "green" },
}, { val: "red" }, { defaultOperator: "==" });
assert.deepEqual(schemaDefaultConditionPatch.cond, { var: "steps.review.verdict", op: "==", val: "red" });
assert.equal(schemaDefaultConditionPatch.label, "steps.review.verdict == \"red\"");

const customConditionEdge = {
  ...generatedConditionEdge,
  label: "needs reviewer",
};
const customConditionPatch = controller.graphEdgeConditionPatch(customConditionEdge, { op: ">" });
assert.deepEqual(customConditionPatch.cond, { var: "steps.review.verdict", op: ">", val: "green" });
assert.equal(customConditionPatch.label, "needs reviewer");

const graphConditionOperatorContract = {
  mob_definition: {
    defaults: { condition_operator: "==" },
    condition_operators: ["==", "!="],
  },
};
const namedOperatorConditionPatch = controller.graphEdgeConditionOperatorPatch(generatedConditionEdge, "!=", {
  contract: graphConditionOperatorContract,
});
assert.deepEqual(namedOperatorConditionPatch.cond, { var: "steps.review.verdict", op: "!=", val: "green" });
assert.equal(namedOperatorConditionPatch.label, 'steps.review.verdict != "green"');
assert.deepEqual(controller.graphEdgeConditionOperatorPatch(generatedConditionEdge, "contains_everything", {
  contract: graphConditionOperatorContract,
}), {});

const namedValueConditionPatch = controller.graphEdgeConditionValuePatch(generatedConditionEdge, "amber");
assert.deepEqual(namedValueConditionPatch.cond, { var: "steps.review.verdict", op: "==", val: "amber" });
assert.equal(namedValueConditionPatch.label, 'steps.review.verdict == "amber"');

const retargetedConditionPatch = controller.graphEdgeConditionPatch(customConditionEdge, {
  var: "params.route",
  op: "==",
  val: "docs",
}, { forceLabel: true });
assert.deepEqual(retargetedConditionPatch.cond, { var: "params.route", op: "==", val: "docs" });
assert.equal(retargetedConditionPatch.label, "params.route == \"docs\"");
assert.deepEqual(controller.parseGraphConditionVar("steps.review.verdict"), {
  instanceId: "review",
  field: "verdict",
  namespace: "steps",
});
assert.deepEqual(controller.parseGraphConditionVar("params.route"), {
  instanceId: "params",
  field: "route",
  namespace: "params",
});
assert.deepEqual(controller.graphConditionRefForEdge({
  cond: { source: "params.route", op: "==", value: "docs" },
}), { instanceId: "params", field: "route", namespace: "params" });
const graphConditionOptionRows = controller.graphConditionOptions({
  instances: [
    { id: "planner", memberId: "m_planner", col: 0, row: 0 },
    { id: "review", memberId: "m_reviewer", col: 1, row: 0 },
    { id: "target", memberId: "m_coder", col: 2, row: 0 },
    { id: "future", memberId: "m_reviewer", col: 4, row: 0 },
    { id: "g_branch", isGate: true, col: 1, row: 1 },
  ],
  members: [
    { id: "m_planner", name: "Planner", schema: "PlanArtifact" },
    { id: "m_reviewer", name: "Reviewer", schema: "ReviewArtifact" },
    { id: "m_coder", name: "Coder", schema: "" },
  ],
  schemas: [
    { id: "PlanArtifact", fields: [{ id: "f_plan", name: "plan", type: "string" }] },
    { id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum", enumValues: ["green", "red"] }] },
  ],
  edge: { from: "review", to: "target", cond: { var: "params.route", op: "==", val: "docs" } },
  flow: { steps: [{ type: "input", fields: "route: enum(code,docs)" }] },
  graphView: { ...hydratedCatalogs.graphView, graphInputParamSourceLabel: "Runtime input" },
});
assert.deepEqual(graphConditionOptionRows.map((option) => ({
  id: option.inst.id,
  member: option.member.name,
  fields: option.fields.map((field) => field.name),
  isParams: !!option.isParams,
})), [
  { id: "params", member: "Runtime input", fields: ["route"], isParams: true },
  { id: "planner", member: "Planner", fields: ["plan"], isParams: false },
  { id: "review", member: "Reviewer", fields: ["verdict"], isParams: false },
]);
assert.deepEqual(controller.graphFirstConditionPatch(
  { cond: { var: "steps.review.verdict", op: "==", val: "green" } },
  graphConditionOptionRows,
  { defaultOperator: "==" },
), { var: "steps.review.verdict", op: "==", val: "green" });
assert.deepEqual(controller.graphFirstConditionPatch(
  { cond: null },
  graphConditionOptionRows,
  { defaultOperator: "==" },
), { var: "params.route", op: "==", val: "" });
assert.deepEqual(controller.graphEdgeConditionOwnerPatch(
  { kind: "next", label: "", cond: null },
  graphConditionOptionRows,
  "review",
  { defaultOperator: "==", forceLabel: true, includeKind: true },
), {
  kind: "",
  cond: { var: "steps.review.verdict", op: "==", val: "" },
  label: "steps.review.verdict == \"\"",
});
assert.deepEqual(controller.graphEdgeConditionOwnerPatch(
  { kind: "next", label: "", cond: null },
  graphConditionOptionRows,
  "review",
  {
    defaultOperator: "==",
    forceLabel: true,
    includeKind: true,
    contract: {
      mob_definition: {
        defaults: { graph_condition_edge_kind: "cond" },
        graph_edge_kinds: ["next", "cond"],
      },
    },
  },
), {
  kind: "cond",
  cond: { var: "steps.review.verdict", op: "==", val: "" },
  label: "steps.review.verdict == \"\"",
});
assert.deepEqual(controller.graphEdgeConditionOwnerPatch(
  { kind: "cond", label: "params.route == \"docs\"", cond: { var: "params.route", op: "==", val: "docs" } },
  graphConditionOptionRows,
  "planner",
  { defaultOperator: "==", forceLabel: true },
), {
  cond: { var: "steps.planner.plan", op: "==", val: "docs" },
  label: "steps.planner.plan == \"docs\"",
});
assert.deepEqual(controller.graphEdgeConditionOwnerPatch(
  { kind: "cond", label: "params.route == \"docs\"", cond: { var: "params.route", op: "==", val: "docs" } },
  graphConditionOptionRows,
  "ghost_owner",
  { defaultOperator: "==", forceLabel: true },
), {});
assert.deepEqual(controller.graphEdgeConditionFieldPatch(
  { kind: "cond", label: "steps.review.verdict == \"green\"", cond: { var: "steps.review.verdict", op: "==", val: "green" } },
  graphConditionOptionRows,
  "verdict",
  { defaultOperator: "==", forceLabel: true },
), {
  cond: { var: "steps.review.verdict", op: "==", val: "green" },
  label: "steps.review.verdict == \"green\"",
});
assert.deepEqual(controller.graphEdgeConditionFieldPatch(
  { kind: "cond", label: "steps.review.verdict == \"green\"", cond: { var: "steps.review.verdict", op: "==", val: "green" } },
  graphConditionOptionRows,
  "ghost_field",
  { defaultOperator: "==", forceLabel: true },
), {});
assert.deepEqual(controller.graphEdgeConditionFieldPatch(
  { kind: "cond", label: "params.route == \"docs\"", cond: { var: "params.route", op: "==", val: "docs" } },
  graphConditionOptionRows,
  "",
  { defaultOperator: "==", forceLabel: true },
), {
  cond: { var: "", op: "==", val: "docs" },
  label: "",
});
assert.deepEqual(controller.graphConditionOptions({
  instances: [{ id: "review", memberId: "m_reviewer", col: 0, row: 0 }],
  members: [{ id: "m_reviewer", name: "Reviewer", schema: "ReviewArtifact" }],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
  edge: { from: "review", to: "review", cond: { var: "params.missing", op: "==", val: "x" } },
  flow: { steps: [{ type: "input", inputParams: [] }] },
}).map((option) => ({ id: option.inst.id, fields: option.fields.map((field) => field.name), isParams: !!option.isParams })), [
  { id: "params", fields: ["missing"], isParams: true },
]);

const generatedConditionKindPatch = controller.graphEdgeKindPatch({
  ...generatedConditionEdge,
  label: "steps.review.verdict == \"green\"",
}, "next");
assert.deepEqual(generatedConditionKindPatch, { kind: "next", cond: null, label: "" });

const customConditionKindPatch = controller.graphEdgeKindPatch(customConditionEdge, "fanout");
assert.deepEqual(customConditionKindPatch, { kind: "fanout", cond: null, label: "needs reviewer" });

const conditionKindPatch = controller.graphEdgeKindPatch({
  id: "e_next",
  from: "input",
  to: "writer",
  kind: "next",
  label: "",
  cond: null,
}, "cond", {
  defaultOperator: "==",
  conditionPatch: { var: "params.route", val: "docs" },
  forceLabel: true,
});
assert.deepEqual(conditionKindPatch, {
  kind: "cond",
  cond: null,
  label: "",
});
const contractBackedConditionKindPatch = controller.graphEdgeKindPatch({
  id: "e_next",
  from: "input",
  to: "writer",
  kind: "next",
  label: "",
  cond: null,
}, "cond", {
  defaultOperator: "==",
  conditionPatch: { var: "params.route", val: "docs" },
  forceLabel: true,
  contract: {
    mob_definition: {
      defaults: { graph_condition_edge_kind: "cond" },
      graph_edge_kinds: ["next", "cond"],
    },
  },
});
assert.deepEqual(contractBackedConditionKindPatch, {
  kind: "cond",
  cond: { var: "params.route", op: "==", val: "docs" },
  label: "params.route == \"docs\"",
});
const testEditorGraphDraft = {
  branch_gate_label: "branch",
  branch_condition_lane_label: "condition",
  branch_fallback_lane_label: "fallback",
  branch_join_label: "join · branch paths",
  fallback_edge_label: "fallback",
  parallel_lane_labels: ["lane 1", "lane 2"],
  parallel_edge_label: "parallel",
  rework_edge_label: "rework",
  terminal_edge_label_prefix: "to ",
  join_label_prefix: "join · ",
  join_quorum_label_prefix: "barrier · ",
  branch_frame_label_prefix: "BRANCH · ",
  branch_frame_singular_suffix: " path",
  branch_frame_plural_suffix: " paths",
  parallel_frame_label_prefix: "PARALLEL · ",
  parallel_frame_join_infix: " · join ",
  parallel_missing_dispatch_label: "missing dispatch",
  parallel_missing_collection_label: "missing collection",
  repeat_frame_label_prefix: "REPEAT-UNTIL · ",
  repeat_max_iterations_prefix: "max ",
  repeat_missing_max_iterations_label: "missing max_iterations",
  repeat_edge_until_prefix: "until ",
  repeat_edge_until_fallback: "until condition",
};
const graphProjectionTestContract = {
  mob_definition: {
    defaults: {
      graph_edge_kind: "next",
      graph_condition_edge_kind: "cond",
      graph_fanout_edge_kind: "fanout",
    },
    graph_edge_kinds: ["next", "fanout", "cond"],
    editor_graph_draft: testEditorGraphDraft,
  },
};
assert.deepEqual(controller.graphEdgeFallbackPatch({
  id: "e_cond",
  kind: "cond",
  label: "params.route == \"docs\"",
  cond: { var: "params.route", op: "==", val: "docs" },
}, {
  mob_definition: {
    defaults: { graph_edge_kind: "next" },
    graph_edge_kinds: ["next", "cond"],
    editor_graph_draft: testEditorGraphDraft,
  },
}), {
  kind: "next",
  label: "fallback",
  cond: null,
});
assert.deepEqual(controller.graphEdgeFallbackPatch({
  id: "e_cond",
  kind: "when",
  label: "params.route == \"docs\"",
  cond: { var: "params.route", op: "==", val: "docs" },
}, {
  mob_definition: {
    defaults: { graph_edge_kind: "straight" },
    graph_edge_kinds: ["straight", "when"],
    editor_graph_draft: {
      ...testEditorGraphDraft,
      fallback_edge_label: "otherwise",
    },
  },
}), {
  kind: "straight",
  label: "otherwise",
  cond: null,
});
assert.equal(controller.graphEdgeFallbackPatch({ kind: "cond" }, {
  mob_definition: { graph_edge_kinds: ["next", "cond"] },
}), null);

const schemaAvailabilityFlow = {
  name: "schema-availability-proof",
  steps: [
    { id: "input_1", type: "input", task: "Review.", inputParams: [] },
    { id: "review_step", type: "member", role: "m_review", schema: "ReviewArtifact" },
    {
      id: "branch_on_review",
      type: "branch",
      branches: [{
        id: "br_green",
        label: "Green",
        cond: { namespace: "steps", stepId: "review_step", field: "verdict", op: "==", val: "green" },
        condition: "steps.review_step.verdict == \"green\"",
        steps: [],
      }],
      fallback: [],
    },
  ],
};
const schemaAvailabilityEdges = [
  { id: "e_schema_1", from: "review_inst", to: "next", kind: "cond", label: "steps.review_inst.verdict == \"green\"", cond: { var: "steps.review_inst.verdict", op: "==", val: "green" } },
];
const schemaAvailabilityMembers = [
  { id: "m_review", name: "Review", role: "review", schema: "ReviewArtifact" },
];
const schemaAvailabilityInstances = [
  { id: "review_inst", memberId: "m_review", col: 0, row: 0 },
];
const availableConditions = controller.reconcileConditionFieldAvailability({
  flow: schemaAvailabilityFlow,
  edges: schemaAvailabilityEdges,
  members: schemaAvailabilityMembers,
  instances: schemaAvailabilityInstances,
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f1", name: "verdict", type: "enum" }] }],
});
assert.equal(availableConditions.flow, schemaAvailabilityFlow);
assert.equal(availableConditions.edges, schemaAvailabilityEdges);

const unavailableConditions = controller.reconcileConditionFieldAvailability({
  flow: schemaAvailabilityFlow,
  edges: schemaAvailabilityEdges,
  members: schemaAvailabilityMembers,
  instances: schemaAvailabilityInstances,
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f2", name: "summary", type: "string" }] }],
});
assert.deepEqual(unavailableConditions.flow.steps[2].branches[0].cond, {});
assert.equal(unavailableConditions.flow.steps[2].branches[0].condition, "");
assert.deepEqual(unavailableConditions.edges[0].cond, null);
assert.equal(unavailableConditions.edges[0].label, "");

const branchDocumentWithStaleFrame = controller.buildDocument({
  flow: branchFlow,
  studio: {
    members: graphMembers,
    schemas: [],
    instances: [
      { id: "g_branch_route", isGate: true, gateKind: "fork", dispatch: "fan_out", col: 7, row: 3 },
      { id: "left", memberId: "m_left", col: 1, row: 0 },
      { id: "right", memberId: "m_right", col: 1, row: 1 },
      { id: "j_branch_route", isGate: true, gateKind: "join", col: 2, row: 0 },
      { id: "g_parallel_deleted", isGate: true, gateKind: "fork", col: 4, row: 4 },
    ],
    edges: [],
    frames: [
      { id: "frame_branch_route", kind: "Parallel", colStart: 9, colEnd: 12, label: "custom layout frame" },
      { id: "frame_deleted_parallel", kind: "Parallel", colStart: 4, colEnd: 6, label: "stale" },
    ],
    skillRealms: [],
  },
  currentFlow: { name: "branch-frame-filter-proof" },
  deploySettings: testDeploySettings(),
  contract: graphProjectionTestContract,
});

assert.deepEqual(branchDocumentWithStaleFrame.frames.map((frame) => frame.id), ["frame_branch_route"]);
assert.equal(branchDocumentWithStaleFrame.frames[0].kind, "Branch");
assert.equal(branchDocumentWithStaleFrame.frames[0].label, "BRANCH · 2 paths");
assert.equal(branchDocumentWithStaleFrame.frames[0].colStart, 0);
assert.equal(branchDocumentWithStaleFrame.frames[0].colEnd, 2);
assert.deepEqual(branchDocumentWithStaleFrame.instances.map((instance) => instance.id), ["g_branch_route", "left", "right", "j_branch_route"]);
const exportedBranchGate = branchDocumentWithStaleFrame.instances.find((instance) => instance.id === "g_branch_route");
assert.equal(exportedBranchGate.gateKind, "branch");
assert.equal(exportedBranchGate.col, 7);
assert.equal(exportedBranchGate.row, 3);
assert.deepEqual(
  branchDocumentWithStaleFrame.edges.map((edge) => [edge.from, edge.to, edge.kind]),
  [
    ["g_branch_route", "left", "cond"],
    ["g_branch_route", "right", "cond"],
    ["left", "j_branch_route", "next"],
    ["right", "j_branch_route", "next"],
  ],
);

const parallelFlow = controller.graphToFlow({
  previousFlow,
  members: graphMembers,
  instances: [
    { id: "g_parallel_review", isGate: true, gateKind: "fork", dispatch: "fan_out", dependsMode: "any", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
    { id: "j_parallel_review", isGate: true, gateKind: "join", collection: "quorum", quorum: { mode: "NofM", n: 1, m: 2 }, col: 2, row: 0 },
  ],
  edges: [
    { id: "e1", from: "g_parallel_review", to: "left", kind: "fanout", label: "" },
    { id: "e2", from: "g_parallel_review", to: "right", kind: "fanout", label: "" },
    { id: "e3", from: "left", to: "j_parallel_review", kind: "next", label: "" },
    { id: "e4", from: "right", to: "j_parallel_review", kind: "next", label: "" },
  ],
});

const parallelStep = parallelFlow.steps[1];
assert.equal(parallelStep.type, "parallel");
assert.equal(parallelStep.dispatch, "fan_out");
assert.equal(parallelStep.collection, "quorum");
assert.equal(parallelStep.dependsMode, "any");
assert.equal(parallelStep.quorum, 1);
assert.deepEqual(parallelStep.branches.map((branch) => branch.steps[0].role), ["m_left", "m_right"]);

const incompleteParallelFlow = controller.graphToFlow({
  previousFlow,
  members: graphMembers,
  instances: [
    { id: "g_parallel_review", isGate: true, gateKind: "fork", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
    { id: "j_parallel_review", isGate: true, gateKind: "join", col: 2, row: 0 },
  ],
  edges: [
    { id: "e1", from: "g_parallel_review", to: "left", kind: "fanout", label: "" },
    { id: "e2", from: "g_parallel_review", to: "right", kind: "fanout", label: "" },
    { id: "e3", from: "left", to: "j_parallel_review", kind: "next", label: "" },
    { id: "e4", from: "right", to: "j_parallel_review", kind: "next", label: "" },
  ],
});

assert.equal(incompleteParallelFlow.steps[1].type, "parallel");
assert.equal(incompleteParallelFlow.steps[1].dispatch, "");
assert.equal(incompleteParallelFlow.steps[1].collection, "");
assert(!("dependsMode" in incompleteParallelFlow.steps[1]));

const incompleteParallelProjection = controller.graphProjectionForFlow(incompleteParallelFlow, graphMembers, graphProjectionTestContract);
const incompleteParallelFork = incompleteParallelProjection.instances.find((instance) => instance.id === "g_parallel_review");
const incompleteParallelJoin = incompleteParallelProjection.instances.find((instance) => instance.id === "j_parallel_review");
assert.equal(incompleteParallelFork.dispatch, "");
assert.equal(incompleteParallelFork.label, "");
assert.equal(incompleteParallelJoin.collection, "");
assert.equal(incompleteParallelJoin.label, "join · missing collection");
assert.equal(incompleteParallelProjection.frames[0].label, "PARALLEL · missing dispatch · join missing collection");
const noContractParallelProjection = controller.graphProjectionForFlow(incompleteParallelFlow, graphMembers, null);
assert.deepEqual(
  Array.from(new Set(noContractParallelProjection.edges.map((edge) => edge.kind))),
  [""],
  "Graph projection must not invent local edge kinds without the Meta contract",
);

const incompleteParallelDocument = controller.buildDocument({
  flow: incompleteParallelFlow,
  studio: {
    members: graphMembers,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "missing-parallel-metadata-proof" },
  deploySettings: testDeploySettings(),
  contract: graphProjectionTestContract,
});
const incompleteDocumentFork = incompleteParallelDocument.instances.find((instance) => instance.id === "g_parallel_review");
const incompleteDocumentJoin = incompleteParallelDocument.instances.find((instance) => instance.id === "j_parallel_review");
assert.equal(incompleteDocumentFork.dispatch, "");
assert.equal(incompleteDocumentFork.label, "");
assert.equal(incompleteDocumentJoin.collection, "");
assert.equal(incompleteParallelDocument.frames[0].label, "PARALLEL · missing dispatch · join missing collection");

const repeatFlow = controller.graphToFlow({
  previousFlow,
  members: graphMembers,
  instances: [
    { id: "review", memberId: "m_review", col: 0, row: 0 },
  ],
  edges: [
    { id: "e1", from: "review", to: "review", kind: "cond", label: "until green", cond: { path: "steps.review.verdict", op: "==", value: "green" } },
  ],
  contract: branchGraphEdgeContract,
});

const repeatStep = repeatFlow.steps[1];
assert.equal(repeatStep.type, "repeat");
assert.deepEqual(repeatStep.cond, { stepId: "review", field: "verdict", op: "==", val: "green" });
assert.equal(repeatStep.steps[0].role, "m_review");
assert.equal(repeatStep.loopId, "");
assert.equal(repeatStep.maxIterations, null);
assert.equal(repeatStep.iterationInput, "");
const incompleteRepeatProjection = controller.graphProjectionForFlow(repeatFlow, graphMembers, graphProjectionTestContract);
assert.equal(incompleteRepeatProjection.frames[0].label, "REPEAT-UNTIL · missing max_iterations");
assert.notEqual(incompleteRepeatProjection.frames[0].label, "REPEAT-UNTIL · max 3");

const authoredRepeatFlow = controller.graphToFlow({
  previousFlow: {
    name: "main",
    steps: [
      previousFlow.steps[0],
      {
        id: "loop_review",
        type: "repeat",
        loopId: "quality_loop",
        maxIterations: 4,
        iterationInput: "carry",
        steps: [{ id: "review", type: "member", role: "m_review", instruction: "Review the loop output." }],
      },
    ],
  },
  members: graphMembers,
  instances: [
    { id: "review", memberId: "m_review", col: 0, row: 0 },
  ],
  edges: [
    { id: "e1", from: "review", to: "review", kind: "cond", label: "until green", cond: { path: "steps.review.verdict", op: "==", value: "green" } },
  ],
  contract: branchGraphEdgeContract,
});
assert.equal(authoredRepeatFlow.steps[1].id, "loop_review");
assert.equal(authoredRepeatFlow.steps[1].loopId, "quality_loop");
assert.equal(authoredRepeatFlow.steps[1].maxIterations, 4);
assert.equal(authoredRepeatFlow.steps[1].iterationInput, "carry");
assert.equal(authoredRepeatFlow.steps[1].steps[0].instruction, "Review the loop output.");
const authoredRepeatProjection = controller.graphProjectionForFlow(authoredRepeatFlow, graphMembers, graphProjectionTestContract);
assert.equal(authoredRepeatProjection.frames[0].label, "REPEAT-UNTIL · max 4");

const repeatWithoutCondition = controller.graphToFlow({
  previousFlow,
  members: graphMembers,
  instances: [
    { id: "review", memberId: "m_review", col: 0, row: 0 },
  ],
  edges: [
    { id: "e1", from: "review", to: "review", kind: "cond", label: "", cond: {} },
  ],
  contract: branchGraphEdgeContract,
});

assert.equal(repeatWithoutCondition.steps[1].type, "repeat");
assert.equal(repeatWithoutCondition.steps[1].cond, null);

const repeatDocument = controller.buildDocument({
  flow: repeatFlow,
  studio: {
    members: graphMembers,
    schemas: [],
    instances: [{ id: "review", memberId: "m_review", col: 0, row: 0 }],
    edges: [{ id: "e1", from: "review", to: "review", kind: "cond", label: "until green", cond: { path: "steps.review.verdict", op: "==", value: "green" } }],
    frames: [],
    skillRealms: [],
  },
  currentFlow: { name: "repeat-projection-proof" },
  deploySettings: testDeploySettings(),
  contract: graphProjectionTestContract,
});

assert(repeatDocument.frames.some((frame) => frame.id === `frame_${repeatStep.id}` && frame.kind === "RepeatUntil"));

const incompleteGraphBranchFlow = controller.graphToFlow({
  previousFlow,
  members: graphMembers,
  instances: [
    { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
  ],
  edges: [
    { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "docs", cond: { source: "params.kind", op: "==" } },
    { id: "e2", from: "g_branch_route", to: "right", kind: "next", label: "fallback" },
  ],
  contract: branchGraphEdgeContract,
});

assert.equal(incompleteGraphBranchFlow.steps[1].type, "branch");
assert.equal(incompleteGraphBranchFlow.steps[1].branches[0].condition, "");
assert.equal(incompleteGraphBranchFlow.steps[1].branches[0].cond, null);

const missingOperatorGraphBranchFlow = controller.graphToFlow({
  members: graphMembers.slice(0, 2),
  previousFlow,
  instances: [
    { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
  ],
  edges: [
    { id: "e1", from: "g_branch_route", to: "left", kind: "cond", label: "", cond: { source: "params.kind", val: "docs" } },
    { id: "e2", from: "g_branch_route", to: "right", kind: "next", label: "fallback" },
  ],
  contract: branchGraphEdgeContract,
});

assert.equal(missingOperatorGraphBranchFlow.steps[1].type, "branch");
assert.equal(missingOperatorGraphBranchFlow.steps[1].branches[0].condition, "");
assert.equal(missingOperatorGraphBranchFlow.steps[1].branches[0].cond, null);

assert.deepEqual(
  controller.toolCatalogFromCatalogs({
    tool_catalog: [{ id: "mob", label: "Mob tools", kind: "runtime", field: "mob", desc: "real mob tools", source: "meerkat_mob::ToolConfig" }],
    tool_config: [{ id: "stale", label: "stale" }],
  }).map((tool) => [tool.id, tool.kind, tool.raw.field]),
  [["mob", "runtime", "mob"]],
);

assert.deepEqual(
  controller.toolCatalogFromCatalogs({
    tool_config: [{ id: "shell", label: "Shell", kind: "runtime", field: "shell", desc: "real shell tool", source: "meerkat_mob::ToolConfig" }],
  }).map((tool) => [tool.id, tool.kind, tool.raw.field]),
  [],
);

assert.deepEqual(
  controller.graphControlNodes({
    mob_definition: {
      graph_gate_kinds: ["branch", "fork", "join"],
      graph_palette_gate_kinds: ["branch", "fork"],
    },
  }, hydratedCatalogs.graphView).map((node) => node.gateKind),
  ["branch", "fork"],
);
const addNodeMenuState = controller.graphAddNodeMenuState({
  members: [
    { id: "m_planner", role: "planner", name: "Planner", model: "gpt-5.5" },
    { id: "m_reviewer", role: "reviewer", name: "Reviewer", model: "gpt-5.5" },
  ],
  contract: {
    mob_definition: {
      graph_gate_kinds: ["branch", "fork", "join"],
      graph_palette_gate_kinds: ["branch", "fork"],
    },
  },
  query: "fork",
  graphView: hydratedCatalogs.graphView,
});
assert.equal(addNodeMenuState.searchIcon, "⌕");
assert.equal(addNodeMenuState.searchPlaceholder, "Add a node…");
assert.equal(addNodeMenuState.closeLabel, "✕");
assert.equal(addNodeMenuState.closeTitle, "Close");
assert.equal(addNodeMenuState.agentsLabel, "Agents");
assert.equal(addNodeMenuState.controlsLabel, "Control flow");
assert.equal(addNodeMenuState.terminalsLabel, "Terminals");
assert.equal(addNodeMenuState.jumpLabel, "+ New agent in Agents →");
assert.deepEqual(addNodeMenuState.memberRows, []);
assert.deepEqual(addNodeMenuState.controlRows.map((row) => [row.id, row.gateKind, row.label, row.pick.kind, row.pick.gateKind]), [
  ["fork", "fork", "Parallel fork", "gate", "fork"],
]);
assert.deepEqual(addNodeMenuState.terminalRows, []);
assert.equal(addNodeMenuState.hasMembers, false);
assert.equal(addNodeMenuState.hasControls, true);
assert.equal(addNodeMenuState.hasTerminals, false);
assert.equal(addNodeMenuState.isEmpty, false);
const memberAddNodeMenuState = controller.graphAddNodeMenuState({
  members: [
    { id: "m_planner", role: "planner", name: "Planner", model: "gpt-5.5" },
  ],
  contract: {
    mob_definition: {
      graph_gate_kinds: ["branch"],
      graph_palette_gate_kinds: ["branch"],
    },
  },
  graphView: hydratedCatalogs.graphView,
});
assert.deepEqual(memberAddNodeMenuState.memberRows[0].dotStyle, { "--role-color": "var(--accent)" });
const noMemberAddNodeMenuState = controller.graphAddNodeMenuState({
  members: [],
  contract: {
    mob_definition: {
      graph_gate_kinds: ["branch", "fork", "join"],
      graph_palette_gate_kinds: ["branch", "fork"],
    },
  },
  graphView: hydratedCatalogs.graphView,
});
assert.deepEqual(noMemberAddNodeMenuState.controlRows.map((row) => row.id), ["branch", "fork"]);
assert.deepEqual(noMemberAddNodeMenuState.terminalRows, []);
assert.equal(noMemberAddNodeMenuState.hasControls, true);
assert.equal(noMemberAddNodeMenuState.hasTerminals, false);
const emptyAddNodeMenuState = controller.graphAddNodeMenuState({
  members: [{ id: "m_planner", role: "planner", name: "Planner", model: "gpt-5.5" }],
  contract: { mob_definition: { graph_gate_kinds: ["branch"], graph_palette_gate_kinds: ["branch"] } },
  query: "zzz",
  graphView: hydratedCatalogs.graphView,
});
assert.equal(emptyAddNodeMenuState.emptyLabel, "No matches for “zzz”");
assert.equal(emptyAddNodeMenuState.isEmpty, true);
assert.deepEqual(controller.graphAddMenuOpenProjection({
  col: 2,
  row: 3,
  grid: { padX: 80, padY: 60, cellW: 240, cellH: 180, gapX: 24, gapY: 18 },
}), {
  addAt: { col: 2, row: 3, x: 598, y: 90 },
});
assert.deepEqual(controller.graphAddMenuCloseProjection(), {
  addAt: null,
});

const basicPickerKickoffState = controller.basicStepPickerState({
  isKickoff: true,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(basicPickerKickoffState.mode, "kickoff");
assert.equal(basicPickerKickoffState.title, "Input");
assert.match(basicPickerKickoffState.kickoffHint, /mob's ingress/);

const basicPickerState = controller.basicStepPickerState({
  members: [
    { id: "m_planner", role: "planner", name: "Planner", model: "gpt-5.5", schema: "PlanArtifact" },
    { id: "m_reviewer", role: "reviewer", name: "Reviewer", model: "gpt-5.5" },
  ],
  contract: { mob_definition: { editor_flow_step_types: ["repeat", "branch", "parallel"] } },
  query: "parallel",
  basicView: hydratedCatalogs.basicView,
});
assert.equal(basicPickerState.mode, "picker");
assert.equal(basicPickerState.title, "Add step");
assert.equal(basicPickerState.searchIcon, "⌕");
assert.equal(basicPickerState.searchPlaceholder, "Search members & primitives…");
assert.equal(basicPickerState.membersLabel, "Mob members");
assert.equal(basicPickerState.flowLabel, "Flow");
assert.equal(basicPickerState.newBadgeLabel, "NEW");
assert.deepEqual(basicPickerState.memberRows, []);
assert.deepEqual(basicPickerState.primitiveRows.map((row) => [row.id, row.label, row.pick.kind]), [
  ["parallel", "Parallel", "parallel"],
]);
assert.equal(basicPickerState.hasConfiguredMembers, true);

const emptyBasicPickerState = controller.basicStepPickerState({
  members: [],
  contract: { mob_definition: { editor_flow_step_types: ["branch"] } },
  basicView: hydratedCatalogs.basicView,
});
assert.equal(emptyBasicPickerState.emptyMembersHint, "No members yet — define some in the Agents tab.");
assert.equal(emptyBasicPickerState.hasConfiguredMembers, false);
assert.deepEqual(emptyBasicPickerState.memberRows, []);
assert.deepEqual(emptyBasicPickerState.primitiveRows.map((row) => [row.id, row.glyph, row.tint]), [
  ["branch", "⑂", "member"],
]);
assert.deepEqual(emptyBasicPickerState.primitiveRows.map((row) => [row.id, row.disabled, row.pick]), [
  ["branch", false, { kind: "branch" }],
]);

const graphShapeContract = {
  mob_definition: {
    defaults: {
      launch_mode: "fresh",
      dispatch_mode: "fan_out",
      collection_policy: "all",
      dependency_mode: "all",
      condition_operator: "==",
      schema_field_type: "string",
      branch_param_type: "enum",
      runtime_mode: "turn_driven",
      graph_terminal_kind: "success",
      graph_edge_kind: "next",
      graph_condition_edge_kind: "cond",
      graph_fanout_edge_kind: "fanout",
    },
    graph_gate_kinds: ["branch", "fork", "join"],
    graph_palette_gate_kinds: ["branch", "fork"],
    graph_terminal_kinds: ["success", "failed", "human"],
    graph_edge_kinds: ["next", "fanout", "cond"],
    editor_graph_draft: testEditorGraphDraft,
    editor_input_step_draft: {
      default_step: {
        id: "input",
        type: "input",
        task: "Run the mobpack flow.",
        fields: "",
        inputParams: [],
      },
    },
    editor_flow_step_types: ["repeat", "branch", "parallel"],
    launch_modes: ["fresh", "resume", "fork"],
    runtime_modes: ["autonomous_host", "turn_driven"],
    dispatch_modes: ["fan_out", "one_to_one", "fan_in"],
    dispatch_mode_labels: {
      fan_out: "fan_out — broadcast to every lane",
      one_to_one: "one_to_one — pair inputs with lanes",
      fan_in: "fan_in — gather upstream outputs",
    },
    collection_policies: ["all", "any", "quorum"],
    collection_policy_labels: {
      all: "all — wait for every branch",
      any: "any — accept the first completed branch",
      quorum: "quorum — require N branches",
    },
    dependency_modes: ["all", "any"],
    dependency_mode_labels: {
      all: "all — every upstream node",
      any: "any — any upstream node",
    },
    option_unsupported_label_separator: " — not in Meta ",
    option_unsupported_reason_prefix: "Unsupported by the Meta ",
    option_unsupported_reason_suffix: " contract.",
    condition_operators: ["==", ">", "<"],
    editor_schema_field_types: ["string", "enum"],
    editor_input_param_draft: {
      added_field: {
        name: "param",
        required: true,
        description: "",
        enumValues: [],
      },
    },
  },
};

assert.equal(controller.contractDefaultValue({ mob_definition: {} }, "launch_mode"), "");
assert.equal(controller.contractDefaultValue(graphShapeContract, "launch_mode"), "Fresh");
assert.equal(controller.contractDefaultValue(graphShapeContract, "dispatch_mode"), "fan_out");
assert.equal(controller.contractDefaultValue(graphShapeContract, "collection_policy"), "all");
assert.equal(controller.contractDefaultValue(graphShapeContract, "dependency_mode"), "all");
assert.equal(controller.contractDefaultValue(graphShapeContract, "condition_operator"), "==");
assert.equal(controller.contractDefaultValue(graphShapeContract, "schema_field_type"), "string");
assert.equal(controller.contractDefaultValue(graphShapeContract, "branch_param_type"), "enum");
assert.equal(controller.contractDefaultValue(graphShapeContract, "graph_condition_edge_kind"), "cond");
assert.equal(controller.contractDefaultValue(graphShapeContract, "graph_fanout_edge_kind"), "fanout");
assert.equal(controller.contractDefaultValue(graphShapeContract, "graph_terminal_kind"), "success");
assert.equal(controller.contractDefaultValue(graphShapeContract, "runtime_mode"), "turn_driven");
const schemaDraftInputFlow = controller.graphToFlow({
  previousFlow: {
    name: "schema-input",
    steps: [{ id: "input_1", type: "member", role: "m_reviewer" }],
  },
  members,
  instances: [],
  edges: [],
  contract: graphShapeContract,
});
assert.deepEqual(schemaDraftInputFlow.steps, [{
  id: "input_2",
  type: "input",
  task: "Run the mobpack flow.",
  fields: "",
  inputParams: [],
}]);

const customProjectionContract = {
  mob_definition: {
    defaults: {
      graph_edge_kind: "straight",
      graph_condition_edge_kind: "when",
      graph_fanout_edge_kind: "spread",
      condition_operator: "==",
    },
    graph_edge_kinds: ["straight", "when", "spread"],
    editor_graph_draft: testEditorGraphDraft,
  },
};
const customProjectionFlow = {
  name: "custom projection",
  steps: [
    { id: "input", type: "input" },
    {
      id: "route",
      type: "branch",
      branches: [{ id: "br_docs", condition: "params.kind == \"docs\"", steps: [{ id: "left", type: "member", role: "m_left" }] }],
      fallback: [{ id: "right", type: "member", role: "m_right" }],
    },
    {
      id: "fan",
      type: "parallel",
      branches: [
        { id: "br_a", steps: [{ id: "review", type: "member", role: "m_review" }] },
        { id: "br_b", steps: [{ id: "writer", type: "member", role: "m_writer" }] },
      ],
    },
  ],
};
const gateState = controller.graphGateControlState({
  id: "join_1",
  gateKind: "join",
  quorum: { n: 2, m: 3 },
}, {
  edges: [
    { from: "a", to: "join_1" },
    { from: "b", to: "join_1" },
    { from: "join_1", to: "c" },
  ],
  members: [{ id: "m_joiner" }],
  contract: graphShapeContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(gateState.gateKind, "join");
assert.equal(gateState.eyebrow, "GATE · join");
assert.equal(gateState.title, "");
assert.equal(gateState.idLine, "join_1 · cell (1,1)");
assert.equal(gateState.deleteLabel, "DELETE");
assert.equal(gateState.labelTitle, "LABEL");
assert.equal(gateState.kindTitle, "KIND");
assert.equal(gateState.selectedGateKind.label, "join — wait for branches");
assert.equal(gateState.collectionTitle, "COLLECTION POLICY");
assert.equal(gateState.quorumIncomingLabel, "of 2 incoming");
assert.equal(gateState.joinMemberLabel, "Join member");
assert.deepEqual(gateState.joinMemberPlaceholderOption, { value: "", label: "— select member —" });
assert.equal(gateState.joinMemberHint, "Meta uses this real profile to resolve non-all fan-in.");
const reskinnedGateState = controller.graphGateControlState({
  id: "join_1",
  gateKind: "join",
}, {
  edges: [],
  members: [],
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    gateEyebrowTemplate: "CONTROL {kind}",
    gateIdLineTemplate: "{id} @ {col}/{row}",
    gateQuorumIncomingTemplate: "{count} upstream wires",
    gateMemberOptionTemplate: "{role}: {name}",
  },
});
assert.equal(reskinnedGateState.eyebrow, "CONTROL join");
assert.equal(reskinnedGateState.idLine, "join_1 @ 1/1");
assert.equal(reskinnedGateState.quorumIncomingLabel, "0 upstream wires");
assert.equal(gateState.dispatchTitle, "DISPATCH MODE");
assert.equal(gateState.dispatchHint, "Exports as the Meta parallel flow dispatch mode.");
assert.equal(gateState.conditionsTitle, "CONDITIONS");
assert.equal(gateState.emptyBranchHint, "add outgoing edges, then configure each as a typed condition or fallback");
assert.equal(gateState.wiringTitle, "WIRING");
assert.equal(gateState.incomingLabel, "incoming");
assert.equal(gateState.outgoingLabel, "outgoing");
assert.equal(gateState.collection, "quorum");
assert.equal(gateState.incoming.length, 2);
assert.equal(gateState.outgoing.length, 1);
assert.equal(gateState.firstMemberId, "m_joiner");
assert.equal(gateState.incomingCount, 2);
assert.equal(gateState.outgoingCount, 1);
assert.deepEqual(gateState.memberOptions, [{
  value: "m_joiner",
  label: "m_joiner · profile",
  member: { id: "m_joiner" },
}]);
assert.deepEqual(controller.graphGateControlState({
  id: "join_1",
  gateKind: "join",
}, {
  members: [{ id: "m_joiner", name: "Joiner", role: "arbiter" }],
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    gateMemberOptionTemplate: "{role}: {name}",
  },
}).memberOptions.map((option) => option.label), ["arbiter: Joiner"]);
const graphProjectionMembers = [
  { id: "m_writer", name: "Writer", role: "writer", schema: "Draft", tools: ["builtins", "shell", "git", "comms"] },
  { id: "m_review", name: "Reviewer", role: "reviewer", schema: "", tools: [] },
];
const customGraphProjectionMembers = [
  ...graphProjectionMembers,
  { id: "m_left", name: "Left", role: "left", schema: "", tools: [] },
  { id: "m_right", name: "Right", role: "right", schema: "", tools: [] },
];
const customProjection = controller.graphProjectionForFlow(customProjectionFlow, customGraphProjectionMembers, customProjectionContract);
assert.deepEqual(customProjection.edges.map((edge) => [edge.from, edge.to, edge.kind, edge.label]), [
  ["g_branch_route", "left", "when", "params.kind == \"docs\""],
  ["g_branch_route", "right", "straight", "fallback"],
  ["left", "j_branch_route", "straight", ""],
  ["right", "j_branch_route", "straight", ""],
  ["j_branch_route", "g_parallel_fan", "straight", ""],
  ["g_parallel_fan", "review", "spread", ""],
  ["g_parallel_fan", "writer", "spread", ""],
  ["review", "j_parallel_fan", "straight", ""],
  ["writer", "j_parallel_fan", "straight", ""],
]);
const customProjectionDocument = controller.buildDocument({
  flow: customProjectionFlow,
  studio: {
    members: customGraphProjectionMembers,
    schemas: [],
    instances: [],
    edges: [],
    frames: [],
  },
  deploySettings: testDeploySettings(),
  contract: customProjectionContract,
});
assert.deepEqual(customProjectionDocument.edges.map((edge) => edge.kind), customProjection.edges.map((edge) => edge.kind));
const customGraphRoundTripFlow = controller.graphToFlow({
  previousFlow: customProjectionFlow,
  members: customGraphProjectionMembers,
  instances: customProjection.instances,
  edges: customProjection.edges,
  contract: customProjectionContract,
});
assert.equal(customGraphRoundTripFlow.steps[1].type, "branch");
assert.deepEqual(customGraphRoundTripFlow.steps[1].branches.map((branch) => [branch.id, branch.cond]), [
  ["br_left", { namespace: "params", stepId: "params", field: "kind", op: "==", val: "docs" }],
]);
assert.deepEqual(customGraphRoundTripFlow.steps[1].fallback.map((step) => [step.id, step.role]), [
  ["right", "m_right"],
]);
assert.equal(customGraphRoundTripFlow.steps[2].type, "parallel");
assert.deepEqual(customGraphRoundTripFlow.steps[2].branches.map((branch) => branch.steps[0]?.role).sort(), ["m_review", "m_writer"]);
const customFallbackLabelContract = {
  mob_definition: {
    ...customProjectionContract.mob_definition,
    editor_graph_draft: {
      ...testEditorGraphDraft,
      fallback_edge_label: "otherwise",
    },
  },
};
const customFallbackLabelRoundTripFlow = controller.graphToFlow({
  previousFlow: customProjectionFlow,
  members: customGraphProjectionMembers,
  instances: [
    { id: "g_branch_route", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "left", memberId: "m_left", col: 1, row: 0 },
    { id: "right", memberId: "m_right", col: 1, row: 1 },
  ],
  edges: [
    { id: "e_left", from: "g_branch_route", to: "left", kind: "when", label: "params.kind == \"docs\"", cond: { var: "params.kind", op: "==", val: "docs" } },
    { id: "e_right", from: "g_branch_route", to: "right", kind: "when", label: "otherwise", cond: null },
  ],
  contract: customFallbackLabelContract,
});
assert.equal(customFallbackLabelRoundTripFlow.steps[1].type, "branch");
assert.deepEqual(customFallbackLabelRoundTripFlow.steps[1].branches.map((branch) => branch.steps[0]?.role), ["m_left"]);
assert.deepEqual(customFallbackLabelRoundTripFlow.steps[1].fallback.map((step) => step.role), ["m_right"]);
const customRepeatGraphFlow = controller.graphToFlow({
  previousFlow: {
    steps: [{ id: "input", type: "input", task: "Repeat until green.", inputParams: [] }],
  },
  members: customGraphProjectionMembers,
  instances: [{ id: "review", memberId: "m_review", col: 0, row: 0 }],
  edges: [{
    id: "e_repeat",
    from: "review",
    to: "review",
    kind: "when",
    label: "until green",
    cond: { path: "steps.review.verdict", op: "==", value: "green" },
  }],
  contract: customProjectionContract,
});
assert.equal(customRepeatGraphFlow.steps[1].type, "repeat");
assert.deepEqual(customRepeatGraphFlow.steps[1].cond, {
  stepId: "review",
  field: "verdict",
  op: "==",
  val: "green",
});
const graphProjectionInstances = [
  { id: "n_writer", memberId: "m_writer", col: 0, row: 0 },
  { id: "n_review", memberId: "m_review", col: 1, row: 0 },
  { id: "n_done", isTerminal: true, col: 2, row: 0 },
];
const graphProjectionEdges = [{ id: "e1", from: "n_writer", to: "n_review" }];
assert.deepEqual(controller.graphSelectionState({
  selection: { kind: "instance", id: "n_writer" },
  instances: graphProjectionInstances,
  edges: graphProjectionEdges,
}).instance, graphProjectionInstances[0]);
assert.equal(controller.graphSelectionState({
  selection: { kind: "edge", id: "missing" },
  instances: graphProjectionInstances,
  edges: graphProjectionEdges,
}).missing, true);
assert.deepEqual(controller.graphSelectionProjection("instance", "n_writer"), {
  kind: "instance",
  id: "n_writer",
});
assert.deepEqual(controller.graphSelectionProjection("edge", "e1"), {
  kind: "edge",
  id: "e1",
});
assert.deepEqual(controller.graphSelectionProjection("member", "m_writer"), {
  kind: null,
  id: null,
});
assert.deepEqual(controller.graphSelectionProjection("instance", " "), {
  kind: null,
  id: null,
});
assert.deepEqual(controller.graphTemplateInspectorState({
  studio: {
    members: graphProjectionMembers,
    instances: graphProjectionInstances,
    edges: graphProjectionEdges,
    frames: [{ id: "fr1" }],
  },
  template: { name: "Docs Mob", repo: "mob.toml", version: "0.1.0", trigger: "docs", defaultTrigger: true },
  templateView: hydratedCatalogs.graphTemplateView,
}).summaryRows.map((row) => [row.key, row.value]), [
  ["members", "2 placed / 2 in library"],
  ["instances", 2],
  ["terminals", 1],
  ["edges", 1],
  ["frames", 1],
]);
assert.deepEqual(controller.graphTemplateInspectorState({
  studio: {
    members: graphProjectionMembers,
    instances: graphProjectionInstances,
    edges: graphProjectionEdges,
    frames: [{ id: "fr1" }],
  },
  template: { name: "Docs Mob", repo: "mob.toml", version: "0.1.0", trigger: "docs", defaultTrigger: true },
  templateView: {
    ...hydratedCatalogs.graphTemplateView,
    summaryMembersLabel: "library profiles",
    summaryInstancesLabel: "placed nodes",
    summaryTerminalsLabel: "terminal nodes",
    summaryEdgesLabel: "wires",
    summaryFramesLabel: "flow frames",
    summaryMembersValueTemplate: "{placed} active / {total} defined",
  },
}).summaryRows.map((row) => [row.key, row.label, row.value]), [
  ["members", "library profiles", "2 active / 2 defined"],
  ["instances", "placed nodes", 2],
  ["terminals", "terminal nodes", 1],
  ["edges", "wires", 1],
  ["frames", "flow frames", 1],
]);
const populatedTemplateInspector = controller.graphTemplateInspectorState({
  studio: {
    members: graphProjectionMembers,
    instances: graphProjectionInstances,
    edges: graphProjectionEdges,
    frames: [{ id: "fr1" }],
  },
  template: { name: "Docs Mob", repo: "mob.toml", version: "0.1.0", trigger: "docs", defaultTrigger: true },
  templateView: hydratedCatalogs.graphTemplateView,
});
assert.equal(populatedTemplateInspector.templateEyebrow, "TEMPLATE");
assert.equal(populatedTemplateInspector.summaryTitle, "SUMMARY");
assert.equal(populatedTemplateInspector.triggersTitle, "TRIGGERS");
assert.deepEqual(populatedTemplateInspector.triggerRows, [
  { key: "labels", label: "labels", value: "docs" },
  { key: "default", label: "default", value: "yes" },
]);
assert.equal(populatedTemplateInspector.quickStartTitle, "QUICK START");
assert.equal(populatedTemplateInspector.quickStartRows[0].parts[1].text, "library member");
const emptyTemplateInspector = controller.graphTemplateInspectorState({ studio: {} });
assert.equal(emptyTemplateInspector.name, "");
assert.equal(emptyTemplateInspector.repo, "");
assert.equal(emptyTemplateInspector.version, "");
assert.deepEqual(emptyTemplateInspector.triggers.labels, []);
assert.equal(emptyTemplateInspector.templateEyebrow, "");
assert.deepEqual(emptyTemplateInspector.quickStartRows, []);
const instanceControlState = controller.graphInstanceControlState({
  inst: graphProjectionInstances[1],
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  graphView: hydratedCatalogs.graphView,
});
assert.equal(instanceControlState.member.id, "m_review");
assert.equal(instanceControlState.memberId, "m_review");
assert.equal(instanceControlState.eyebrow, "INSTANCE");
assert.equal(instanceControlState.title, "Reviewer");
assert.equal(instanceControlState.idLine, "n_review · cell (2,1)");
assert.equal(instanceControlState.deleteLabel, "DELETE");
assert.equal(instanceControlState.memberTitle, "Reviewer");
assert.equal(instanceControlState.memberRoleLabel, "MEMBER · reviewer");
assert.equal(instanceControlState.editMemberLabel, "EDIT MEMBER →");
assert.equal(instanceControlState.memberName, "Reviewer");
assert.deepEqual(instanceControlState.memberSummaryRows, [
  { key: "model", label: "model", value: "—" },
  { key: "schema", label: "schema", value: "—" },
  { key: "tools", label: "tools", value: "0" },
]);
assert.equal(instanceControlState.memberHint, "Editing the member updates every instance that uses it.");
assert.equal(instanceControlState.positionTitle, "POSITION");
assert.deepEqual(instanceControlState.positionRows, [
  { key: "stage", label: "stage (col)", value: 2 },
  { key: "slot", label: "slot (row)", value: 1 },
]);
const reskinnedInstanceControlState = controller.graphInstanceControlState({
  inst: graphProjectionInstances[1],
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  graphView: {
    ...hydratedCatalogs.graphView,
    instanceEyebrow: "PLACEMENT",
    instanceIdLineTemplate: "{id} @ {col}/{row}",
    instanceMemberRoleTemplate: "ROLE {role}",
    instanceEditMemberLabel: "Open agent",
    instanceModelLabel: "model id",
    instanceSchemaLabel: "output",
    instanceToolsLabel: "tool access",
    instanceMemberHint: "Shared agent definition.",
    instancePositionTitle: "GRID",
    instancePositionStageLabel: "column",
    instancePositionSlotLabel: "row",
    instanceOutputTitleTemplate: "OUTPUT {schema}",
    instanceOutputRequiredLabel: "required",
    instanceOutputHint: "From schema.",
    instanceOutputOpenMemberLabel: "Open agent",
  },
});
assert.equal(reskinnedInstanceControlState.eyebrow, "PLACEMENT");
assert.equal(reskinnedInstanceControlState.idLine, "n_review @ 2/1");
assert.equal(reskinnedInstanceControlState.memberRoleLabel, "ROLE reviewer");
assert.deepEqual(reskinnedInstanceControlState.memberSummaryRows.map((row) => [row.key, row.label]), [
  ["model", "model id"],
  ["schema", "output"],
  ["tools", "tool access"],
]);
assert.equal(reskinnedInstanceControlState.memberHint, "Shared agent definition.");
assert.equal(reskinnedInstanceControlState.positionTitle, "GRID");
assert.deepEqual(reskinnedInstanceControlState.positionRows.map((row) => [row.key, row.label]), [
  ["stage", "column"],
  ["slot", "row"],
]);
assert.deepEqual(instanceControlState.forkSourceOptions.map((option) => [option.value, option.label]), [
  ["n_writer", "Writer · n_writer"],
]);
assert.equal(instanceControlState.firstForkSourceId, "n_writer");
const writerInstanceState = controller.graphInstanceControlState({
  inst: graphProjectionInstances[0],
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  graphView: hydratedCatalogs.graphView,
});
assert.equal(writerInstanceState.memberToolSummary, "4 · builtins, shell, git…");
assert.equal(writerInstanceState.memberSchemaLabel, "Draft");
assert.deepEqual(writerInstanceState.memberSummaryRows, [
  { key: "model", label: "model", value: "—" },
  { key: "schema", label: "schema", value: "Draft" },
  { key: "tools", label: "tools", value: "4 · builtins, shell, git…" },
]);
assert.equal(writerInstanceState.outputSchema.id, "Draft");
assert.deepEqual(writerInstanceState.outputFields, [{ id: "f1", name: "body", type: "string", required: true }]);
assert.equal(writerInstanceState.outputTitle, "MEMBER OUTPUT · Draft");
assert.deepEqual(writerInstanceState.outputFieldRows, [{ id: "f1", name: "body", type: "string", required: true, requiredLabel: "req" }]);
assert.equal(writerInstanceState.outputHint, "Defined on the member.");
assert.equal(writerInstanceState.outputOpenMemberLabel, "Open member →");
assert.deepEqual(controller.graphNodeCanvasState({
  inst: { id: "n_writer", memberId: "m_writer", launchMode: { kind: "Fork" } },
  members: graphProjectionMembers,
  density: "comfortable",
  toolCatalog: [
    { id: "builtins", tagClass: "" },
    { id: "shell", tagClass: "is-shell" },
    { id: "mcp:linear", tagClass: "is-write" },
    { id: "comms", tagClass: "" },
  ],
}), {
  hidden: false,
  isTerminal: false,
  isCompact: false,
  roleLabel: "writer",
  launchLabel: "fork",
  title: "Writer",
  subtitle: undefined,
  toolRows: [
    { id: "builtins", className: "tag" },
    { id: "shell", className: "tag is-shell" },
    { id: "git", className: "tag" },
    { id: "comms", className: "tag" },
  ],
  overflowLabel: "",
});
assert.deepEqual(controller.graphNodeCanvasState({
  inst: { id: "n_mcp", memberId: "m_mcp" },
  members: [{ id: "m_mcp", name: "MCP", role: "worker", tools: ["mcp:linear"] }],
  toolCatalog: [{ id: "mcp:linear", tagClass: "is-write" }],
}).toolRows, [
  { id: "mcp:linear", className: "tag is-write" },
]);
assert.deepEqual(controller.graphNodeCanvasState({
  inst: { id: "n_writer", memberId: "m_writer" },
  members: graphProjectionMembers,
  density: "compact",
}).toolRows.map((row) => row.id), ["builtins", "shell", "git"]);
assert.equal(controller.graphNodeCanvasState({
  inst: { id: "n_missing", memberId: "m_missing" },
  members: graphProjectionMembers,
}).hidden, true);
const graphGridState = controller.graphGridState({
  instances: [{ id: "n_a", col: 0, row: 1 }, { id: "n_b", col: 3, row: 2 }],
  gridBase: { cols: 3, rows: 2, padX: 100, padY: 80, cellW: 120, cellH: 90, gapX: 30, gapY: 20 },
});
assert.deepEqual(graphGridState, {
  grid: { cols: 5, rows: 4, padX: 100, padY: 80, cellW: 120, cellH: 90, gapX: 30, gapY: 20 },
  totalW: 920,
  totalH: 580,
});
assert.deepEqual(controller.graphCellXY(graphGridState.grid, 2, 3), { x: 400, y: 410 });
assert.deepEqual(controller.graphCellAt(graphGridState.grid, 350, 338), { col: 1, row: 2 });
assert.deepEqual(controller.graphCellAt(graphGridState.grid, -1, 338), null);
assert.deepEqual(controller.graphDragCellAt(graphGridState.grid, { x: 250, y: 260 }, { dx: 0, dy: 0 }), { col: 1, row: 2 });
assert.deepEqual(controller.graphNodeBox(graphGridState.grid, { col: 1, row: 2 }), { x: 210, y: 267, w: 200, h: 156 });
assert.deepEqual(controller.graphNodeBox(graphGridState.grid, { col: 1, row: 2, isGate: true }), { x: 232, y: 317, w: 156, h: 56 });
assert.deepEqual(controller.graphNodeBox(graphGridState.grid, { col: 1, row: 2, isSourceFile: true }), { x: 205, y: 316, w: 210, h: 58 });
assert.deepEqual(controller.graphPortOut(graphGridState.grid, { col: 1, row: 2 }), { x: 410, y: 345 });
assert.deepEqual(controller.graphPortIn(graphGridState.grid, { col: 1, row: 2 }), { x: 210, y: 345 });
assert.equal(controller.graphEdgePath({ x: 10, y: 20 }, { x: 110, y: 60 }), "M 10 20 C 60 20, 60 60, 110 60");
assert.deepEqual(controller.graphEdgeMidpoint({ x: 10, y: 20 }, { x: 110, y: 60 }), { x: 60, y: 34 });
assert.equal(
  controller.graphEdgePath({ x: 200, y: 40 }, { x: 100, y: 80 }),
  "M 200 40 C 260 40, 260 170, 200 170 L 100 170 C 40 170, 40 80, 100 80",
);
assert.deepEqual(controller.graphEdgeMidpoint({ x: 200, y: 40 }, { x: 100, y: 80 }), { x: 150, y: 170 });
assert.deepEqual(controller.graphCellCanvasRows({
  grid: { cols: 2, rows: 2, padX: 100, padY: 80, cellW: 120, cellH: 90, gapX: 30, gapY: 20 },
  instances: [{ id: "n_a", col: 0, row: 1 }],
  hoverCell: { col: 1, row: 0 },
}), [
  {
    key: "cell-0-0",
    col: 0,
    row: 0,
    occupied: false,
    addVisible: true,
    className: "cell",
    style: { left: 100, top: 80, width: 120, height: 90 },
  },
  {
    key: "cell-0-1",
    col: 0,
    row: 1,
    occupied: true,
    addVisible: false,
    className: "cell is-occupied",
    style: { left: 100, top: 190, width: 120, height: 90 },
  },
  {
    key: "cell-1-0",
    col: 1,
    row: 0,
    occupied: false,
    addVisible: true,
    className: "cell is-hover",
    style: { left: 250, top: 80, width: 120, height: 90 },
  },
  {
    key: "cell-1-1",
    col: 1,
    row: 1,
    occupied: false,
    addVisible: true,
    className: "cell",
    style: { left: 250, top: 190, width: 120, height: 90 },
  },
]);
assert.deepEqual(controller.graphGridHeaderCanvasRows({
  grid: { cols: 2, rows: 2, padX: 100, padY: 80, cellW: 120, cellH: 90, gapX: 30, gapY: 20 },
}), {
  columns: [
    { key: "col-0", label: "01", className: "grid-head grid-head--col", style: { left: 100, top: 28, width: 120 } },
    { key: "col-1", label: "02", className: "grid-head grid-head--col", style: { left: 250, top: 28, width: 120 } },
  ],
  rows: [
    { key: "row-0", label: "A", className: "grid-head grid-head--row", style: { left: 14, top: 117 } },
    { key: "row-1", label: "B", className: "grid-head grid-head--row", style: { left: 14, top: 227 } },
  ],
});
assert.deepEqual(controller.graphFrameCanvasState({
  frame: { id: "frame_branch_route", label: "BRANCH · route", colStart: 1, colEnd: 3 },
  grid: { padX: 100, padY: 80, cellW: 120, cellH: 90, gapX: 30, gapY: 20, rows: 3 },
}), {
  id: "frame_branch_route",
  label: "BRANCH · route",
  frameStyle: { left: 236, top: 62, width: 448, height: 346 },
  labelStyle: { left: 248, top: 52 },
});
const sourceFileAdornment = controller.graphSourceFileAdornment({
  instances: graphProjectionInstances,
  graphView: hydratedCatalogs.graphView,
});
assert.deepEqual(sourceFileAdornment, {
  id: "source_mob_toml",
  isSourceFile: true,
  isGraphAdornment: true,
  adornmentKind: "source_file",
  kind: "source",
  label: "mob.toml",
  col: 0,
  row: -1,
});
const sourceFileAdornmentState = controller.graphSourceFileAdornmentCanvasState({
  adornment: sourceFileAdornment,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(sourceFileAdornmentState.isSourceFile, true);
assert.equal(sourceFileAdornmentState.role, "button");
assert.equal(sourceFileAdornmentState.ariaLabel, "Open mob.toml read-only source editor");
assert.equal(sourceFileAdornmentState.sourceGlyph, "{ }");
assert.equal(sourceFileAdornmentState.sourceActivationHash, "#meta-graph-source");
assert.equal(sourceFileAdornmentState.sourceActivationSelector, ".node--source-file");
assert.equal(sourceFileAdornmentState.roleLabel, "source file");
assert.equal(sourceFileAdornmentState.title, "mob.toml");
assert.equal(controller.graphNodeCanvasState({
  inst: { id: "looks_like_source", label: "mob.toml", kind: "success", isTerminal: true },
  graphView: hydratedCatalogs.graphView,
}).isSourceFile, false);
assert.deepEqual(controller.graphCanvasInstances({
  instances: graphProjectionInstances,
  graphView: hydratedCatalogs.graphView,
}).map((instance) => instance.id), ["n_writer", "n_review", "n_done"]);
assert.deepEqual(controller.graphCanvasAdornments({
  instances: graphProjectionInstances,
  graphView: hydratedCatalogs.graphView,
}).map((adornment) => adornment.id), ["source_mob_toml"]);
const existingSourceInstances = [{ id: "source_mob_toml", label: "mob.toml", isTerminal: true }];
assert.deepEqual(controller.graphCanvasInstances({
  instances: existingSourceInstances,
  graphView: hydratedCatalogs.graphView,
}), []);
assert.deepEqual(controller.graphCanvasAdornments({
  instances: existingSourceInstances,
  graphView: hydratedCatalogs.graphView,
}).map((adornment) => adornment.id), ["source_mob_toml"]);
assert.deepEqual(controller.graphGateCanvasState({
  inst: { id: "join_1", gateKind: "join", collection: "quorum", quorum: { n: 2, m: 3 } },
  edges: [{ to: "join_1" }, { to: "join_1" }],
  contract: graphShapeContract,
  graphView: hydratedCatalogs.graphView,
}), { glyph: "⋈", sublabel: "barrier · 2/2", gateKind: "join" });
assert.deepEqual(controller.graphGateCanvasState({
  inst: { id: "join_1", gateKind: "join", collection: "quorum", quorum: { n: 2, m: 3 } },
  edges: [{ to: "join_1" }, { to: "join_1" }],
  contract: {
    mob_definition: {
      ...graphShapeContract.mob_definition,
      editor_graph_draft: {
        ...testEditorGraphDraft,
        join_quorum_label_prefix: "schema quorum: ",
      },
    },
  },
  graphView: hydratedCatalogs.graphView,
}), { glyph: "⋈", sublabel: "schema quorum: 2/2", gateKind: "join" });
assert.deepEqual(controller.graphGateCanvasState({
  inst: { id: "join_2", gateKind: "join", collection: "any" },
  edges: [],
  contract: {
    mob_definition: {
      ...graphShapeContract.mob_definition,
      editor_graph_draft: {
        ...testEditorGraphDraft,
        join_label_prefix: "schema join: ",
      },
    },
  },
  graphView: {
    ...hydratedCatalogs.graphView,
    gatePaletteRows: [{ id: "join", glyph: "J", label: "Join", meta: "schema glyph" }],
  },
}), { glyph: "J", sublabel: "schema join: any", gateKind: "join" });
assert.deepEqual(controller.graphGateCanvasState({
  inst: { id: "fork_1", gateKind: "fork", label: "forker" },
  edges: [],
  graphView: hydratedCatalogs.graphView,
}), { glyph: "‖", sublabel: "forker", gateKind: "fork" });
assert.deepEqual(controller.graphEdgeCanvasState({
  edge: { id: "e_cond", kind: "cond", label: "review" },
  to: { id: "n_review" },
  active: true,
  selected: false,
  edgeStyle: "icons",
  contract: graphShapeContract,
}), {
  kind: "cond",
  mode: "icons",
  labelText: "review",
  labelWidth: 48,
  iconGlyph: "?",
  labelFill: "var(--danger)",
  iconLabelClass: "edge-label is-cond",
  textLabelClass: "edge-label is-cond is-active",
  lineClass: "edge-line is-cond is-active",
  markerEnd: "url(#arr-acc)",
});
assert.deepEqual(controller.graphEdgeCanvasState({
  edge: { id: "e_fan", kind: "fanout", label: "" },
  to: { id: "n_done", isTerminal: true },
  edgeStyle: "colored",
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    edgeKindLabels: { ...hydratedCatalogs.graphView.edgeKindLabels, fanout: "schema fanout" },
  },
}).markerEnd, "url(#arr-acc)");
assert.equal(controller.graphEdgeCanvasState({
  edge: { id: "e_fan", kind: "fanout", label: "" },
  to: { id: "n_done", isTerminal: true },
  edgeStyle: "colored",
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    edgeKindLabels: { ...hydratedCatalogs.graphView.edgeKindLabels, fanout: "schema fanout" },
  },
}).labelText, "schema fanout");
assert.deepEqual(controller.graphEdgeCanvasState({
  edge: { id: "e_when", kind: "when", label: "route" },
  to: { id: "n_review" },
  contract: customProjectionContract,
}).lineClass, "edge-line is-cond");
assert.deepEqual(controller.graphEdgeCanvasState({
  edge: { id: "e_spread", kind: "spread", label: "" },
  to: { id: "n_review" },
  contract: customProjectionContract,
}).iconGlyph, "‖");
assert.deepEqual(controller.graphEdgeCanvasState({
  edge: { id: "e_term", kind: "next", label: "done" },
  to: { id: "n_done", isTerminal: true },
}), {
  kind: "next",
  mode: "text",
  labelText: "done",
  labelWidth: 36,
  iconGlyph: "■",
  labelFill: "var(--muted)",
  iconLabelClass: "edge-label",
  textLabelClass: "edge-label",
  lineClass: "edge-line is-term",
  markerEnd: "url(#arr-dim)",
});
const terminalControlState = controller.graphTerminalControlState({
  id: "n_done",
  isTerminal: true,
  label: "Done",
  col: 3,
  row: 1,
}, graphShapeContract, hydratedCatalogs.graphView);
assert.equal(terminalControlState.eyebrow, "TERMINAL · success");
assert.equal(terminalControlState.title, "Done");
assert.equal(terminalControlState.idLine, "n_done · cell (4,2)");
assert.equal(terminalControlState.deleteLabel, "DELETE");
assert.equal(terminalControlState.labelTitle, "LABEL");
assert.equal(terminalControlState.labelValue, "Done");
assert.equal(terminalControlState.kindTitle, "KIND");
assert.equal(terminalControlState.terminalKind, "success");
assert.equal(terminalControlState.selectedTerminalKind.label, "success — done");
assert.equal(terminalControlState.authoringLockedTitle, "Visual terminal node");
assert.equal(
  terminalControlState.authoringLockedHint,
  "Visual terminal nodes do not compile into deployable Meta mob.toml. Delete this node or replace it with a real member, branch, join, or loop flow shape."
);
assert.equal(terminalControlState.editable, false);
const reskinnedTerminalControlState = controller.graphTerminalControlState({
  id: "n_done",
  isTerminal: true,
  label: "Done",
  col: 3,
  row: 1,
}, graphShapeContract, {
  ...hydratedCatalogs.graphView,
  terminalEyebrowTemplate: "END {kind}",
  terminalIdLineTemplate: "{id} @ {col}/{row}",
});
assert.equal(reskinnedTerminalControlState.eyebrow, "END success");
assert.equal(reskinnedTerminalControlState.idLine, "n_done @ 4/2");
assert.deepEqual(terminalControlState.terminalKindOptions.map((option) => [option.value, option.label, option.disabled]), [
  ["success", "success — done", false],
  ["failed", "failed — blocked", false],
  ["human", "human — needs human", false],
]);
const unsupportedTerminalControlState = controller.graphTerminalControlState({
  id: "n_done",
  isTerminal: true,
  kind: "archived",
}, graphShapeContract, hydratedCatalogs.graphView);
assert.equal(unsupportedTerminalControlState.terminalKind, "archived");
assert.equal(unsupportedTerminalControlState.selectedTerminalKind.disabled, true);
assert.match(unsupportedTerminalControlState.selectedTerminalKind.reason, /mob_definition\.graph_terminal_kinds/);
const branchConditionRows = controller.graphBranchConditionRows({
  inst: { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
  edges: [
    {
      id: "e_cond",
      from: "branch_1",
      to: "n_review",
      kind: "cond",
      cond: { var: "steps.n_writer.body", op: "==", val: "ok" },
    },
    { id: "e_fallback", from: "branch_1", to: "n_done", kind: "next" },
  ],
  instances: [
    { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "n_writer", memberId: "m_writer", col: 0, row: 1 },
    { id: "n_review", memberId: "m_review", col: 1, row: 0 },
    { id: "n_done", isTerminal: true, label: "Done", col: 2, row: 0 },
  ],
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: graphShapeContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(branchConditionRows.length, 2);
assert.deepEqual({
  id: branchConditionRows[0].edge.id,
  modeValue: branchConditionRows[0].modeValue,
  isCondition: branchConditionRows[0].isCondition,
  conditionEdgeKind: branchConditionRows[0].conditionEdgeKind,
  targetLabel: branchConditionRows[0].targetLabel,
  ownerValue: branchConditionRows[0].ownerValue,
  fieldValue: branchConditionRows[0].fieldValue,
  operatorValue: branchConditionRows[0].operatorValue,
  hasConditionOptions: branchConditionRows[0].hasConditionOptions,
}, {
  id: "e_cond",
  modeValue: "cond",
  isCondition: true,
  conditionEdgeKind: "cond",
  targetLabel: "Reviewer",
  ownerValue: "n_writer",
  fieldValue: "body",
  operatorValue: "==",
  hasConditionOptions: true,
});
assert.deepEqual(branchConditionRows[0].ownerOptions.map((option) => [option.value, option.label]), [["n_writer", "Writer"]]);
assert.deepEqual(branchConditionRows[0].fieldOptions.map((option) => [option.value, option.label]), [["body", "body · string"]]);
const reskinnedBranchConditionRows = controller.graphBranchConditionRows({
  inst: { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
  edges: [{
    id: "e_missing",
    from: "branch_1",
    to: "missing_target",
    kind: "cond",
    cond: { var: "steps.n_writer.body", op: "==", val: "ok" },
  }],
  instances: [
    { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "n_writer", memberId: "m_writer", col: 0, row: 1 },
  ],
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    graphConditionTargetMissingLabel: "missing node",
    graphConditionOwnerOptionTemplate: "{id}: {name}",
    graphConditionFieldOptionTemplate: "{type}/{name}",
  },
});
assert.equal(reskinnedBranchConditionRows[0].targetLabel, "missing node");
assert.deepEqual(reskinnedBranchConditionRows[0].ownerOptions.map((option) => option.label), ["n_writer: Writer"]);
assert.deepEqual(reskinnedBranchConditionRows[0].fieldOptions.map((option) => option.label), ["string/body"]);
assert.deepEqual(branchConditionRows[0].modeOptions, [
  { value: "cond", label: "condition" },
  { value: "fallback", label: "fallback" },
]);
assert.equal(branchConditionRows[0].targetPrefix, "→");
assert.deepEqual(branchConditionRows[0].fieldPlaceholderOption, { value: "", label: "— field —" });
assert.equal(branchConditionRows[0].noConditionOptionsHint, "add input params or an upstream schema field for this condition");
assert.deepEqual(branchConditionRows[1].targetLabel, "Done");
assert.equal(branchConditionRows[1].modeValue, "fallback");
assert.equal(branchConditionRows[1].isCondition, false);
assert.deepEqual(controller.graphBranchConditionModePatch(branchConditionRows[1].edge, "cond", {
  conditionOptions: branchConditionRows[1].conditionOptions,
  firstOwnerId: branchConditionRows[1].firstOwnerId,
  defaultOperator: branchConditionRows[1].defaultOperator,
  contract: graphShapeContract,
}), {
  kind: "cond",
  cond: { var: "steps.n_writer.body", op: "==", val: "" },
  label: "steps.n_writer.body == \"\"",
});
assert.deepEqual(controller.graphBranchConditionModePatch(branchConditionRows[0].edge, "fallback", {
  contract: graphShapeContract,
}), {
  kind: "next",
  label: "fallback",
  cond: null,
});
const customGraphConditionContract = {
  mob_definition: {
    defaults: {
      graph_edge_kind: "straight",
      graph_condition_edge_kind: "when",
      condition_operator: "==",
    },
    graph_edge_kinds: ["straight", "when"],
    condition_operators: ["=="],
    editor_graph_draft: testEditorGraphDraft,
  },
};
const customBranchConditionRows = controller.graphBranchConditionRows({
  inst: { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
  edges: [{ id: "e_when", from: "branch_1", to: "n_review", kind: "when", cond: { var: "steps.n_writer.body", op: "==", val: "ok" } }],
  instances: [
    { id: "branch_1", isGate: true, gateKind: "branch", col: 0, row: 0 },
    { id: "n_writer", memberId: "m_writer", col: 0, row: 1 },
    { id: "n_review", memberId: "m_review", col: 1, row: 0 },
  ],
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: customGraphConditionContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(customBranchConditionRows[0].modeValue, "when");
assert.equal(customBranchConditionRows[0].isCondition, true);
assert.deepEqual(customBranchConditionRows[0].modeOptions, [
  { value: "when", label: "condition" },
  { value: "fallback", label: "fallback" },
]);
assert.deepEqual(controller.graphBranchConditionModePatch(customBranchConditionRows[0].edge, "fallback", {
  contract: customGraphConditionContract,
}), { kind: "straight", label: "fallback", cond: null });
assert.deepEqual(controller.graphBranchConditionModePatch({ id: "e_straight", from: "branch_1", to: "n_review", kind: "straight" }, "when", {
  conditionOptions: customBranchConditionRows[0].conditionOptions,
  firstOwnerId: customBranchConditionRows[0].firstOwnerId,
  defaultOperator: customBranchConditionRows[0].defaultOperator,
  contract: customGraphConditionContract,
}), {
  kind: "when",
  cond: { var: "steps.n_writer.body", op: "==", val: "" },
  label: "steps.n_writer.body == \"\"",
});
const edgeInspectorState = controller.graphEdgeInspectorState({
  edge: {
    id: "e_cond",
    from: "n_writer",
    to: "n_review",
    kind: "cond",
    cond: { var: "steps.n_writer.body", op: "==", val: "ok" },
  },
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: graphShapeContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(edgeInspectorState.title, "Writer → Reviewer");
assert.equal(edgeInspectorState.eyebrow, "EDGE · cond");
assert.equal(edgeInspectorState.idLine, "e_cond");
assert.equal(edgeInspectorState.fieldPlaceholder, "— field —");
const reskinnedEdgeInspectorState = controller.graphEdgeInspectorState({
  edge: { id: "e_cond", from: "n_writer", to: "n_review", kind: "cond" },
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [],
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    edgeEyebrowTemplate: "WIRE {kind}",
    edgeTitleTemplate: "{from} to {to}",
    edgeIdLineTemplate: "edge:{id}",
    edgeFieldNoSchemaPlaceholder: "no fields",
    graphConditionOwnerOptionTemplate: "{id}: {name}",
    graphConditionFieldOptionTemplate: "{type}/{name}",
  },
});
assert.equal(reskinnedEdgeInspectorState.eyebrow, "WIRE cond");
assert.equal(reskinnedEdgeInspectorState.title, "Writer to Reviewer");
assert.equal(reskinnedEdgeInspectorState.idLine, "edge:e_cond");
assert.equal(reskinnedEdgeInspectorState.fieldPlaceholder, "no fields");
assert.deepEqual(reskinnedEdgeInspectorState.ownerOptions.map((option) => option.label), []);
assert.deepEqual(controller.graphEdgeInspectorState({
  edge: { id: "e_cond", from: "n_writer", to: "n_review", kind: "cond", cond: { var: "steps.n_writer.body", op: "==", val: "ok" } },
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: graphShapeContract,
  graphView: {
    ...hydratedCatalogs.graphView,
    graphConditionOwnerOptionTemplate: "{id}: {name}",
    graphConditionFieldOptionTemplate: "{type}/{name}",
  },
}).fieldOptions.map((option) => option.label), ["string/body"]);
assert.equal(edgeInspectorState.deleteLabel, "DELETE");
assert.equal(edgeInspectorState.kindTitle, "KIND");
assert.equal(edgeInspectorState.labelTitle, "LABEL");
assert.equal(edgeInspectorState.conditionTitle, "CONDITION");
assert.equal(edgeInspectorState.noConditionOptionsHint, "Add an upstream agent with an output schema before configuring this edge.");
assert.deepEqual(edgeInspectorState.ownerPlaceholderOption, { value: "", label: "— member —" });
assert.equal(edgeInspectorState.fromTitle, "FROM");
assert.equal(edgeInspectorState.toTitle, "TO");
assert.equal(edgeInspectorState.edgeKind, "cond");
assert.equal(edgeInspectorState.isCondition, true);
assert.equal(edgeInspectorState.conditionEdgeKind, "cond");
assert.equal(edgeInspectorState.defaultOperator, "==");
assert.equal(edgeInspectorState.operatorValue, "==");
assert.equal(edgeInspectorState.ownerValue, "n_writer");
assert.equal(edgeInspectorState.fieldValue, "body");
assert.deepEqual(edgeInspectorState.ownerOptions.map((option) => [option.value, option.label]), [["n_writer", "Writer"]]);
assert.deepEqual(edgeInspectorState.fieldOptions.map((option) => [option.value, option.label]), [["body", "body · string"]]);
assert.deepEqual(edgeInspectorState.fromRows.map((row) => [row.label, row.value]), [
  ["instance", "n_writer"],
  ["member", "Writer"],
  ["schema", "Draft"],
]);
assert.deepEqual(edgeInspectorState.toRows.map((row) => [row.label, row.value]), [
  ["instance", "n_review"],
  ["member", "Reviewer"],
  ["schema", "—"],
]);
assert.deepEqual(edgeInspectorState.conditionPatch, { var: "steps.n_writer.body", op: "==", val: "ok" });
const defaultEdgeInspectorState = controller.graphEdgeInspectorState({
  edge: { id: "e_next", from: "n_writer", to: "n_done" },
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: graphShapeContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(defaultEdgeInspectorState.title, "Writer → —");
assert.equal(defaultEdgeInspectorState.edgeKind, "next");
assert.equal(defaultEdgeInspectorState.isCondition, false);
assert.equal(defaultEdgeInspectorState.selectedEdgeKind.value, "next");
assert.deepEqual(defaultEdgeInspectorState.toRows.map((row) => [row.label, row.value]), [
  ["instance", "n_done"],
  ["member", "(terminal)"],
  ["schema", "—"],
]);
const customEdgeInspectorState = controller.graphEdgeInspectorState({
  edge: { id: "e_when", from: "n_writer", to: "n_review", kind: "when", cond: { var: "steps.n_writer.body", op: "==", val: "ok" } },
  instances: graphProjectionInstances,
  members: graphProjectionMembers,
  schemas: [{ id: "Draft", fields: [{ id: "f1", name: "body", type: "string", required: true }] }],
  contract: customGraphConditionContract,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(customEdgeInspectorState.edgeKind, "when");
assert.equal(customEdgeInspectorState.isCondition, true);
assert.equal(customEdgeInspectorState.conditionEdgeKind, "when");
assert.deepEqual(controller.graphGateKindPatch(" fork ", graphShapeContract), {
  gateKind: "fork",
});
assert.deepEqual(controller.graphGateKindPatch(" unsupported_gate ", graphShapeContract), {});
assert.deepEqual(controller.graphInstanceLabelPatch(" join label\n"), {
  label: " join label\n",
});
assert.deepEqual(controller.graphEdgeLabelPatch(" fallback path "), {
  label: " fallback path ",
});
assert.deepEqual(controller.graphTerminalKindPatch(" success ", graphShapeContract), {
  kind: "success",
});
assert.deepEqual(controller.graphTerminalKindPatch(" waiting_room ", graphShapeContract), {});
assert.deepEqual(controller.graphJoinCollectionPatch({}, "quorum", {
  incomingCount: 2,
  firstMemberId: "m_joiner",
  contract: graphShapeContract,
}), {
  collection: "quorum",
  label: "join · quorum",
  quorum: { n: 2, m: 2 },
  controllerRole: "m_joiner",
});
assert.deepEqual(controller.graphJoinCollectionPatch({
  quorum: { n: 1 },
  controllerRole: "m_existing",
}, "any", {
  incomingCount: 3,
  firstMemberId: "m_joiner",
  contract: graphShapeContract,
}), {
  collection: "any",
  label: "join · any",
  quorum: null,
  controllerRole: "m_existing",
});
assert.deepEqual(controller.graphJoinCollectionPatch({ controllerRole: "m_existing" }, "all", {
  incomingCount: 3,
  firstMemberId: "m_joiner",
  contract: graphShapeContract,
}), {
  collection: "all",
  label: "join · all",
  quorum: null,
  controllerRole: "",
});
assert.deepEqual(controller.graphJoinCollectionPatch({}, "quorum", {
  incomingCount: 2,
  firstMemberId: "m_joiner",
  contract: {
    mob_definition: {
      ...graphShapeContract.mob_definition,
      editor_graph_draft: {
        ...testEditorGraphDraft,
        join_label_prefix: "schema join: ",
        parallel_missing_collection_label: "schema missing collection",
      },
    },
  },
}), {
  collection: "quorum",
  label: "schema join: quorum",
  quorum: { n: 2, m: 2 },
  controllerRole: "m_joiner",
});
assert.deepEqual(controller.graphJoinCollectionPatch({ controllerRole: "m_existing" }, "lottery", {
  incomingCount: 3,
  firstMemberId: "m_joiner",
  contract: graphShapeContract,
}), {});
assert.deepEqual(controller.graphJoinQuorumPatch({ quorum: { n: 2, m: 5 } }, "4", 3), {
  quorum: { n: 4, m: 3 },
});
assert.deepEqual(controller.graphJoinControllerRolePatch(" m_joiner ", [{ id: "m_joiner" }]), {
  controllerRole: "m_joiner",
});
assert.deepEqual(controller.graphJoinControllerRolePatch("", [{ id: "m_joiner" }]), {
  controllerRole: "",
});
assert.deepEqual(controller.graphJoinControllerRolePatch("m_deleted", [{ id: "m_joiner" }]), {});
assert.deepEqual(controller.graphForkDispatchPatch({}, "fan_out", graphShapeContract), {
  dispatch: "fan_out",
  label: "fan_out",
});
assert.deepEqual(controller.graphForkDispatchPatch({}, "broadcast_everywhere", graphShapeContract), {});

const launchControlContract = {
  mob_definition: {
    ...graphShapeContract.mob_definition,
    defaults: {
      ...graphShapeContract.mob_definition.defaults,
      fork_context: "full_history",
    },
    fork_contexts: ["full_history", "last_messages"],
  },
};
const blankLaunchState = controller.launchModeControlState({}, launchControlContract, TEST_LAUNCH_VIEW);
assert.equal(blankLaunchState.launchTitle, "Launch mode");
assert.equal(blankLaunchState.graphLaunchTitle, "LAUNCH MODE · this position");
assert.equal(blankLaunchState.resumeSessionLabel, "Bridge session");
assert.equal(blankLaunchState.resumeSessionPlaceholder, "session id");
assert.equal(blankLaunchState.forkSourceLabel, "Fork from");
assert.equal(blankLaunchState.forkContextLabel, "Fork context");
assert.equal(blankLaunchState.graphForkContextLabel, "Context");
assert.equal(blankLaunchState.launchKind, "Fresh");
assert.deepEqual(blankLaunchState.launchMode, { kind: "Fresh" });
assert.equal(blankLaunchState.forkContextValue, "full_history");
assert.deepEqual(controller.launchModeKindPatch({}, "Fork", launchControlContract, {
  firstForkSourceId: "plan_step",
}), { launchMode: { kind: "Fork", from: "plan_step", context: "full_history" } });
assert.deepEqual(controller.launchModeKindPatch({
  launchMode: { kind: "Fresh", budgetSplitPolicy: { kind: "Fixed", limit: 2048 } },
}, "Fork", launchControlContract, {
  firstForkSourceId: "plan_step",
}), { launchMode: { kind: "Fork", from: "plan_step", context: "full_history" } });
assert.deepEqual(controller.launchModeMergePatch({ launchMode: { kind: "Fresh" } }, {
  budgetSplitPolicy: { kind: "Fixed", limit: 2048 },
}), {});
assert.deepEqual(controller.launchModeMergePatch({ launchMode: { kind: "Fresh" } }, {
  budget_split_policy: { type: "fixed", value: 2048 },
}), {});
assert.deepEqual(controller.launchModeMergePatch({ launchMode: { kind: "Fresh" } }, {
  budget: { kind: "Fixed", limit: 2048 },
}), {});
assert.deepEqual(controller.launchModeKindPatch({
  launchMode: { kind: "Fresh" },
}, "Teleport", launchControlContract), {});
assert.deepEqual(controller.launchModeSessionPatch({
  launchMode: { kind: "Resume", sessionId: "old" },
}, "new", launchControlContract), { launchMode: { kind: "Resume", sessionId: "new" } });
assert.deepEqual(controller.launchModeForkSourcePatch({
  launchMode: { kind: "Fork", from: "old_step", context: "last_messages" },
}, "plan_step", launchControlContract, {
  sourceOptions: [{ value: "plan_step" }, { value: "review_step" }],
}), { launchMode: { kind: "Fork", from: "plan_step", context: "last_messages" } });
assert.deepEqual(controller.launchModeForkSourcePatch({
  launchMode: { kind: "Fork", from: "old_step", context: "last_messages" },
}, "", launchControlContract, {
  sourceOptions: [{ value: "plan_step" }],
}), { launchMode: { kind: "Fork", from: "", context: "last_messages" } });
assert.deepEqual(controller.launchModeForkSourcePatch({
  launchMode: { kind: "Fork", from: "old_step", context: "last_messages" },
}, "phantom_step", launchControlContract, {
  sourceOptions: [{ value: "plan_step" }],
}), {});
assert.deepEqual(controller.launchModeForkContextPatch({
  launchMode: { kind: "Fork", from: "plan_step", context: "LastMessages" },
}, "FullHistory", launchControlContract), { launchMode: { kind: "Fork", from: "plan_step", context: "full_history" } });
assert.deepEqual(controller.launchModeForkContextPatch({
  launchMode: { kind: "Fork", from: "plan_step", context: "last_messages" },
}, "entire_universe", launchControlContract), {});
const memberStepControlContract = {
  mob_definition: {
    ...launchControlContract.mob_definition,
    step_output_formats: ["json", "text"],
  },
};
const writeStep = {
  id: "write",
  type: "member",
  role: "m_writer",
  launchMode: { kind: "Fork" },
  dispatchMode: "fan_out",
  collection: "quorum",
  dependsMode: "any",
  outputFormat: "xml",
};
const memberStepControlState = controller.basicMemberStepControlState({
  step: writeStep,
  flow: {
    steps: [
      { id: "plan", type: "member", role: "m_planner" },
      {
        id: "branch",
        type: "branch",
        branches: [{ id: "br1", steps: [{ id: "review", type: "member", role: "m_review" }] }],
        fallback: [],
      },
      writeStep,
    ],
  },
  members: [
    { id: "m_planner", name: "Planner", role: "planner", model: "gpt-5.5" },
    { id: "m_review", name: "Reviewer", role: "reviewer", model: "gpt-5.5" },
    { id: "m_writer", name: "Writer", role: "writer", model: "gpt-5.5", schema: "Draft", tools: ["shell", "git"] },
  ],
  contract: memberStepControlContract,
  basicView: hydratedCatalogs.basicView,
  launchView: hydratedCatalogs.launchView,
});
assert.equal(memberStepControlState.member.name, "Writer");
assert.equal(memberStepControlState.panelTitle, "Writer");
assert.equal(memberStepControlState.panelSub, "writer · gpt-5.5");
assert.equal(memberStepControlState.memberFieldLabel, "Member (profile)");
assert.equal(memberStepControlState.memberPlaceholderLabel, "— select member —");
assert.equal(memberStepControlState.launchState.launchKind, "Fork");
assert.equal(memberStepControlState.firstLaunchSourceId, "plan");
assert.deepEqual(memberStepControlState.launchSourceOptions.map((option) => [option.value, option.label]), [
  ["plan", "Planner · plan"],
  ["review", "Reviewer · review"],
]);
assert.deepEqual(memberStepControlState.memberOptions.map((option) => [option.value, option.label]), [
  ["m_planner", "Planner · planner"],
  ["m_review", "Reviewer · reviewer"],
  ["m_writer", "Writer · writer"],
]);
assert.equal(memberStepControlState.instructionLabel, "message — instruction for this turn");
assert.equal(memberStepControlState.instructionPlaceholder, "e.g. Run the focused tests and report failures.");
assert.equal(memberStepControlState.dispatchLabel, "Dispatch mode");
assert.equal(memberStepControlState.dispatchValue, "fan_out");
assert.equal(memberStepControlState.collectionLabel, "Collection policy");
assert.equal(memberStepControlState.collectionValue, "quorum");
assert.equal(memberStepControlState.showQuorum, true);
assert.equal(memberStepControlState.quorumLabel, "Quorum");
assert.equal(memberStepControlState.quorumPlaceholder, "required");
assert.equal(memberStepControlState.timeoutLabel, "Timeout (ms)");
assert.equal(memberStepControlState.timeoutPlaceholder, "runtime default");
assert.equal(memberStepControlState.dependencyLabel, "depends_on mode");
assert.equal(memberStepControlState.dependencyValue, "any");
assert.equal(memberStepControlState.outputFormatLabel, "Output format");
assert.equal(memberStepControlState.outputValue, "xml");
assert.equal(memberStepControlState.allowedToolsLabel, "Allowed tools");
assert.equal(memberStepControlState.allowedToolsEmptyLabel, "Runtime profile default");
assert.equal(memberStepControlState.blockedToolsLabel, "Blocked tools");
assert.equal(memberStepControlState.blockedToolsEmptyLabel, "No step-level blocks");
assert.deepEqual(memberStepControlState.outputOptions.map((option) => [option.value, option.label, option.disabled]), [
  ["", "runtime default", false],
  ["json", "json — parse terminal output as JSON", false],
  ["text", "text — preserve terminal text", false],
  ["xml", "xml", true],
]);
assert.match(memberStepControlState.selectedOutput.reason, /step_output_formats/);
assert.deepEqual(memberStepControlState.schemaHint, {
  schema: "Draft",
  tools: ["shell", "git"],
  toolSummary: "shell, git",
  parts: [
    { key: "prefix", text: "Emits " },
    { key: "schema", text: "Draft", kind: "code" },
    { key: "tools", text: " · tools: shell, git" },
  ],
});

assert.deepEqual(controller.parseLegacyInputFields("route: enum(code|docs), retries: int\nnotes: string"), [
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code", "docs"] },
  { id: "p2", name: "retries", type: "int", required: true, description: "", enumValues: [] },
  { id: "p3", name: "notes", type: "string", required: true, description: "", enumValues: [] },
]);
assert.equal(controller.uniqueInputParamName([
  { id: "p1", name: "route" },
  { id: "p2", name: "route_2" },
], "route"), "route_3");
assert.equal(controller.uniqueInputParamName([{ id: "p1", name: "route" }], "9 route!", "p1"), "_9_route");
assert.equal(controller.inputParamSummary([
  { name: "route", type: "enum", required: true },
  { name: "notes", type: "", required: false },
], graphShapeContract), "route: enum, notes: string?");
assert.deepEqual(controller.inputParamOptions({
  steps: [{ type: "input", fields: "route: enum(code,docs)" }],
}, { ...hydratedCatalogs.basicView, inputParamSourceLabel: "Runtime input" }), [{
  stepId: "params",
  namespace: "params",
  label: "Runtime input",
  fields: [{ id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code", "docs"] }],
}]);
const inputControlState = controller.basicInputControlState({
  id: "input_1",
  type: "input",
  task: "Fix it.",
  inputParams: [
    { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code", "docs"] },
  ],
}, graphShapeContract, {
  ...hydratedCatalogs.basicView,
  inputPanelTitle: "Runtime input",
  inputParamsTitlePrefix: "RUNTIME FIELDS",
  inputAddParamLabel: "+ runtime field",
  inputParamHeaderLabels: {
    name: "FIELD",
    type: "KIND",
    required: "MUST",
    description: "NOTES",
    action: "",
  },
});
assert.equal(inputControlState.panelIcon, "▤");
assert.equal(inputControlState.panelTitle, "Runtime input");
assert.equal(inputControlState.panelSub, "The task this mob is run with — its ingress");
assert.equal(inputControlState.taskLabel, "Task");
assert.equal(inputControlState.taskPlaceholder, "e.g. Fix the issue described below.");
assert.equal(inputControlState.paramsTitle, "RUNTIME FIELDS · 1");
assert.equal(inputControlState.addParamLabel, "+ runtime field");
assert.deepEqual(inputControlState.headerRows.map((row) => [row.key, row.label, row.className]), [
  ["name", "FIELD", "sb-col sb-col--name"],
  ["type", "KIND", "sb-col sb-col--type"],
  ["required", "MUST", "sb-col sb-col--req"],
  ["description", "NOTES", "sb-col sb-col--desc"],
  ["actions", "", "sb-col sb-col--act"],
]);
assert.deepEqual(inputControlState.emptyParamsParts, [
  { key: "prefix", kind: "text", text: "No params yet. Add one before branching on " },
  { key: "ref", text: "params.*", kind: "code" },
  { key: "suffix", kind: "text", text: "." },
]);
assert.deepEqual(inputControlState.tips, [
  "Run with: rkat mob run <pack> --prompt \"<task>\" — or run_flow(input).",
  "Typed fields become the input schema the run is validated against.",
  "Event sources & schedules live outside the mobpack (e.g. fugue).",
]);
assert.equal(inputControlState.params[0].name, "route");
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code"] },
], "p1", { required: false }, graphShapeContract), {
  inputParams: [{ id: "p1", name: "route", type: "enum", required: false, description: "", enumValues: ["code"] }],
  fields: "route: enum?",
});
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code"] },
  { id: "p2", name: "route_2", type: "string", required: true, description: "", enumValues: [] },
], "p1", { name: "route_2" }, graphShapeContract), {
  inputParams: [
    { id: "p1", name: "route_2_2", type: "enum", required: true, description: "", enumValues: ["code"] },
    { id: "p2", name: "route_2", type: "string", required: true, description: "", enumValues: [] },
  ],
  fields: "route_2_2: enum, route_2: string",
});
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code"] },
], "p1", { name: "" }, graphShapeContract), {
  inputParams: [{ id: "p1", name: "param", type: "enum", required: true, description: "", enumValues: ["code"] }],
  fields: "param: enum",
});
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["code"] },
], "p1", { name: "" }, {
  mob_definition: {
    defaults: { schema_field_type: "enum" },
    editor_schema_field_types: ["enum"],
    editor_input_param_draft: {
      added_field: { name: "input_value", required: true, description: "", enumValues: [] },
    },
  },
}), {
  inputParams: [{ id: "p1", name: "input_value", type: "enum", required: true, description: "", enumValues: ["code"] }],
  fields: "input_value: enum",
});
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "string", required: true, description: "", enumValues: [] },
], "p1", { type: "object" }, graphShapeContract), {
  inputParams: [{ id: "p1", name: "route", type: "string", required: true, description: "", enumValues: [] }],
  fields: "route: string",
});
assert.deepEqual(controller.inputParamUpdatePatch([
  { id: "p1", name: "route", type: "string", required: true, description: "", enumValues: [] },
], "p1", { type: "enum" }, graphShapeContract), {
  inputParams: [{ id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: ["value"] }],
  fields: "route: enum",
});
assert.deepEqual(controller.inputParamRenamePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: [] },
  { id: "p2", name: "route_2", type: "string", required: true, description: "", enumValues: [] },
], "p1", "route_2", graphShapeContract), {
  name: "route_2_2",
  patch: {
    inputParams: [
      { id: "p1", name: "route_2_2", type: "enum", required: true, description: "", enumValues: [] },
      { id: "p2", name: "route_2", type: "string", required: true, description: "", enumValues: [] },
    ],
    fields: "route_2_2: enum, route_2: string",
  },
});
assert.deepEqual(controller.inputParamDeletePatch([
  { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: [] },
  { id: "p2", name: "notes", type: "string", required: false, description: "", enumValues: [] },
], "p1", graphShapeContract), {
  removed: { id: "p1", name: "route", type: "enum", required: true, description: "", enumValues: [] },
  patch: {
    inputParams: [{ id: "p2", name: "notes", type: "string", required: false, description: "", enumValues: [] }],
    fields: "notes: string?",
  },
});
assert.equal(controller.inputParamAddPatch, undefined);
assert.deepEqual(controller.inputParamAddErrorState({
  ok: false,
  error: "Meta schema is missing mob_definition.editor_input_param_draft",
}), {
  hasError: true,
  text: "Meta schema is missing mob_definition.editor_input_param_draft",
  rawError: "Meta schema is missing mob_definition.editor_input_param_draft",
});
assert.deepEqual(controller.inputParamAddErrorState({ ok: true }), {
  hasError: false,
  text: "",
  rawError: "",
});

const treeFlow = {
  name: "tree",
  steps: [
    { id: "input", type: "input" },
    {
      id: "branch",
      type: "branch",
      branches: [
        { id: "br1", steps: [{ id: "left", type: "member", role: "m_left" }] },
        { id: "br2", steps: [] },
      ],
      fallback: [{ id: "fallback_step", type: "member", role: "m_fallback" }],
    },
    {
      id: "loop",
      type: "repeat",
      steps: [{ id: "loop_step", type: "member", role: "m_loop" }],
    },
  ],
};
assert.equal(controller.flowStepUpdatePatch(treeFlow, "loop_step", {
  instruction: "Run loop body",
}).steps[2].steps[0].instruction, "Run loop body");
assert.deepEqual(controller.flowStepUpdatePatch(treeFlow, "loop_step", {
  id: "left",
}, { members: [{ id: "m_loop" }] }), treeFlow);
assert.deepEqual(controller.flowStepUpdatePatch(treeFlow, "loop_step", {
  role: "m_missing",
}, { members: [{ id: "m_loop" }] }), treeFlow);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "branch",
  parentId: "branch",
  branchId: "br2",
}, { id: "right", type: "member", role: "m_right" }).steps[1].branches[1].steps, [
  { id: "right", type: "member", role: "m_right" },
]);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "branch",
  parentId: "branch",
  branchId: "br2",
}, { id: "left", type: "member", role: "m_right" }, { members: [{ id: "m_right" }] }), treeFlow);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "branch",
  parentId: "branch",
  branchId: "br2",
}, { id: "right", type: "member", role: "m_missing" }, { members: [{ id: "m_right" }] }), treeFlow);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "branch",
  parentId: "branch",
  branchId: "fallback",
}, { id: "new_fallback", type: "member", role: "m_new" }).steps[1].fallback.map((step) => step.id), [
  "fallback_step",
  "new_fallback",
]);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "branch",
  parentId: "loop",
  branchId: "body",
  index: 0,
}, { id: "pre_loop", type: "member", role: "m_pre" }).steps[2].steps.map((step) => step.id), [
  "pre_loop",
  "loop_step",
]);
assert.deepEqual(controller.flowStepInsertPatch(treeFlow, {
  lane: "main",
  index: 1,
}, { id: "main_member", type: "member", role: "m_main" }).steps.map((step) => step.id), [
  "input",
  "main_member",
  "branch",
  "loop",
]);
assert.deepEqual(controller.flowStepInsertTransition(treeFlow, {
  lane: "main",
  index: 1,
}, { id: "main_member", type: "member", role: "m_main" }), {
  ok: true,
  error: "",
  flow: controller.flowStepInsertPatch(treeFlow, {
    lane: "main",
    index: 1,
  }, { id: "main_member", type: "member", role: "m_main" }),
  selection: "main_member",
  picker: { open: false },
});
assert.deepEqual(controller.flowStepInsertTransition(treeFlow, {
  lane: "main",
  index: 1,
}, { id: "left", type: "member", role: "m_main" }), {
  ok: false,
  error: "flow step id already exists",
  flow: treeFlow,
  selection: null,
  picker: { open: false },
});
assert.deepEqual(controller.flowStepDeletePatch(treeFlow, "left").steps[1].branches[0].steps, []);
assert.deepEqual(controller.flowStepDeletePatch(treeFlow, "fallback_step").steps[1].fallback, []);
assert.deepEqual(controller.flowStepDeletePatch(treeFlow, "loop_step").steps[2].steps, []);
assert.deepEqual(controller.flowStepDeleteTransition(treeFlow, "left"), {
  flow: controller.flowStepDeletePatch(treeFlow, "left"),
  selection: null,
  picker: { open: false },
});
assert.deepEqual(controller.basicStepPickerOpenTransition({ lane: "main", index: 2 }), {
  picker: { open: true, at: { lane: "main", index: 2 } },
});
assert.deepEqual(controller.basicStepPickerCloseTransition(), {
  picker: { open: false },
});
assert.deepEqual(controller.basicCanvasClearTransition(), {
  selection: null,
  picker: { open: false },
});
assert.deepEqual(controller.basicStepSelectionTransition("left"), {
  selection: "left",
  picker: { open: false },
});
assert.deepEqual(controller.basicStepSelectionTransition(" "), {
  selection: null,
  picker: { open: false },
});
const deletedRefFlow = controller.flowStepDeletePatch({
  name: "delete-refs",
  steps: [
    { id: "source", type: "member", role: "m_source" },
    {
      id: "router",
      type: "branch",
      branches: [{
        id: "br1",
        cond: { namespace: "steps", stepId: "source", field: "verdict", op: "==", val: "green" },
        condition: 'steps.source.verdict == "green"',
        steps: [{ id: "review", type: "member", role: "m_review", launchMode: { kind: "Fork", from: "source", context: "full_history" } }],
      }],
      fallback: [],
    },
    {
      id: "loop",
      type: "repeat",
      cond: { namespace: "steps", stepId: "source", field: "verdict", op: "==", val: "green" },
      until: 'steps.source.verdict == "green"',
      steps: [{ id: "loop_review", type: "member", role: "m_review" }],
    },
  ],
}, "source");
assert.deepEqual(deletedRefFlow.steps.map((step) => step.id), ["router", "loop"]);
assert.deepEqual(deletedRefFlow.steps[0].branches[0].cond, {});
assert.equal(deletedRefFlow.steps[0].branches[0].condition, "");
assert.deepEqual(deletedRefFlow.steps[0].branches[0].steps[0].launchMode, {
  kind: "Fresh",
});
assert.deepEqual(deletedRefFlow.steps[1].cond, {});
assert.equal(deletedRefFlow.steps[1].until, "");

assert.deepEqual(controller.flowStepTaskPatch("Fix the bug.\n"), { task: "Fix the bug.\n" });
assert.deepEqual(controller.flowStepInstructionPatch("Run tests.\n"), { instruction: "Run tests.\n" });
assert.deepEqual(controller.flowStepQuorumPatch("3"), { quorum: 3 });
assert.deepEqual(controller.flowStepQuorumPatch(""), { quorum: null });
assert.deepEqual(controller.flowStepQuorumPatch("0"), { quorum: null });
assert.deepEqual(controller.flowStepTimeoutPatch("120000"), { timeoutMs: 120000 });
assert.deepEqual(controller.flowStepTimeoutPatch(""), { timeoutMs: null });
assert.deepEqual(controller.flowStepTimeoutPatch("-1"), { timeoutMs: null });
assert.deepEqual(controller.flowStepMaxIterationsPatch("5"), { maxIterations: 5 });
assert.deepEqual(controller.flowStepMaxIterationsPatch(""), { maxIterations: null });
assert.deepEqual(controller.flowStepMaxIterationsPatch("1.5"), { maxIterations: null });
assert.deepEqual(controller.flowStepLoopIdPatch(" quality_loop "), { loopId: "quality_loop" });
assert.deepEqual(controller.flowStepLoopIdPatch(""), { loopId: "" });
assert.deepEqual(controller.flowStepRepeatConditionPatch({
  cond: { stepId: "reviewer", field: "verdict", op: "==", val: "red" },
}, { field: "status", val: "green" }), {
  cond: { stepId: "reviewer", field: "status", op: "==", val: "green" },
});
assert.deepEqual(controller.flowStepRepeatConditionPatch({}, { stepId: "reviewer", field: "" }), {
  cond: { stepId: "reviewer", field: "" },
});
assert.deepEqual(controller.basicConditionSourcePatch([
  { stepId: "params", namespace: "params" },
  { stepId: "reviewer", namespace: "steps" },
], "params", { includeNamespace: true }), {
  namespace: "params",
  stepId: "params",
  field: "",
});
assert.deepEqual(controller.basicConditionSourcePatch([
  { stepId: "reviewer" },
], "reviewer"), {
  stepId: "reviewer",
  field: "",
});
assert.deepEqual(controller.basicConditionSourcePatch([
  { stepId: "reviewer" },
], "ghost_step"), {});
assert.deepEqual(controller.basicConditionFieldPatch(" verdict ", [{
  value: "verdict",
  field: { name: "verdict" },
}]), { field: "verdict" });
assert.deepEqual(controller.basicConditionFieldPatch("ghost", [{
  value: "verdict",
  field: { name: "verdict" },
}]), {});
assert.deepEqual(controller.basicConditionOperatorPatch(" == ", graphShapeContract), { op: "==" });
assert.deepEqual(controller.basicConditionOperatorPatch("contains_everything", graphShapeContract), {});
assert.deepEqual(controller.basicConditionValuePatch("green"), { val: "green" });
const iterationInputContract = {
  mob_definition: {
    defaults: { repeat_iteration_input: "carry" },
    repeat_iteration_inputs: ["carry"],
  },
};
assert.deepEqual(controller.flowStepIterationInputPatch(" carry ", iterationInputContract), { iterationInput: "carry" });
assert.deepEqual(controller.flowStepIterationInputPatch("", iterationInputContract), { iterationInput: "" });
assert.deepEqual(controller.flowStepIterationInputPatch("previous", iterationInputContract), {});
assert.deepEqual(controller.flowStepControllerRolePatch(" m_reviewer ", members), { controllerRole: "m_reviewer" });
assert.deepEqual(controller.flowStepControllerRolePatch("", members), { controllerRole: "" });
assert.deepEqual(controller.flowStepControllerRolePatch("m_deleted", members), {});
assert.deepEqual(controller.flowStepMemberRolePatch(" m_reviewer ", members), { role: "m_reviewer" });
assert.deepEqual(controller.flowStepMemberRolePatch("m_coder", members), {});
assert.deepEqual(controller.flowStepDispatchModePatch(" one_to_one ", memberStepControlContract), { dispatchMode: "one_to_one" });
assert.deepEqual(controller.flowStepDispatchModePatch(" broadcast ", memberStepControlContract), {});
assert.deepEqual(controller.flowStepDispatchModePatch("", memberStepControlContract), { dispatchMode: "" });
assert.deepEqual(controller.flowStepParallelDispatchPatch(" fan_out ", memberStepControlContract), { dispatch: "fan_out" });
assert.deepEqual(controller.flowStepParallelDispatchPatch(" broadcast ", memberStepControlContract), {});
assert.deepEqual(controller.flowStepCollectionPatch(" quorum ", memberStepControlContract), { collection: "quorum" });
assert.deepEqual(controller.flowStepCollectionPatch(" lottery ", memberStepControlContract), {});
assert.deepEqual(controller.flowStepDependencyModePatch(" any ", memberStepControlContract), { dependsMode: "any" });
assert.deepEqual(controller.flowStepDependencyModePatch(" maybe ", memberStepControlContract), {});
assert.deepEqual(controller.flowStepOutputFormatPatch(" json ", memberStepControlContract), { outputFormat: "json" });
assert.deepEqual(controller.flowStepOutputFormatPatch(" xml ", memberStepControlContract), {});
assert.deepEqual(controller.flowStepOutputFormatPatch("", memberStepControlContract), { outputFormat: "" });
assert.deepEqual(controller.flowStepOutputFormatPatch(" yaml ", {
  mob_definition: {
    ...memberStepControlContract.mob_definition,
    step_output_formats: ["json", "yaml"],
  },
}), { outputFormat: "yaml" });
assert.deepEqual(controller.flowStepAllowedToolsPatch([" shell ", "", "git", "shell"], {
  member: { tools: ["shell", "builtins", "unknown"] },
  toolCatalog: [{ id: "shell" }, { id: "builtins" }, { id: "memory" }],
}), { allowedTools: ["shell"] });
assert.deepEqual(controller.flowStepAllowedToolsPatch(["shell"], {
  member: { tools: ["shell"] },
  toolCatalog: [],
}), { allowedTools: [] });
assert.deepEqual(controller.flowStepBlockedToolsPatch([" memory ", null, "shell", "phantom", "shell"], {
  toolCatalog: [{ id: "memory" }, { id: "shell" }],
}), { blockedTools: ["memory", "shell"] });

assert.deepEqual(controller.basicBranchConditionPatch({
  type: "branch",
  branches: [{ id: "br1", condition: "", steps: [] }],
}, "br1", {
  namespace: "params",
  stepId: "params",
  field: "route",
  op: "==",
  val: "green",
}, graphShapeContract), {
  branches: [{
    id: "br1",
    condition: 'params.route == "green"',
    cond: { namespace: "params", stepId: "params", field: "route", op: "==", val: "green" },
    steps: [],
  }],
});
assert.deepEqual(controller.basicBranchConditionPatch({
  type: "branch",
  branches: [{ id: "br1", condition: 'params.route == "red"', steps: [] }],
}, "br1", { field: "status" }, graphShapeContract).branches[0].cond, {
  namespace: "params",
  stepId: "params",
  field: "status",
  op: "==",
  val: "red",
});
const branchConditionControlState = controller.basicBranchConditionControlState({
  branch: { id: "br1", cond: { namespace: "params", stepId: "params", field: "route", op: "==", val: "docs" } },
  options: [
    {
      stepId: "params",
      namespace: "params",
      label: "Input params",
      fields: [{ id: "p_route", name: "route", type: "enum" }],
    },
    {
      stepId: "review_step",
      namespace: "steps",
      member: { id: "m_review", name: "Reviewer", schema: "ReviewArtifact" },
    },
  ],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
  contract: graphShapeContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(branchConditionControlState.cond.stepId, "params");
assert.equal(branchConditionControlState.selected.label, "Input params");
assert.equal(branchConditionControlState.field.name, "route");
assert.equal(branchConditionControlState.operatorValue, "==");
assert.equal(branchConditionControlState.previewLabel, 'Input params.route == "docs"');
assert.deepEqual(branchConditionControlState.sourceOptions.map((option) => [option.value, option.label]), [
  ["params", "Input params"],
  ["review_step", "Reviewer"],
]);
assert.deepEqual(branchConditionControlState.fieldOptions.map((option) => [option.value, option.label]), [
  ["route", "route · enum"],
]);
const schemaBranchConditionControlState = controller.basicBranchConditionControlState({
  branch: { id: "br1", cond: { namespace: "steps", stepId: "review_step", field: "verdict", op: "!=", val: "red" } },
  options: [{
    stepId: "review_step",
    namespace: "steps",
    member: { id: "m_review", name: "Reviewer", schema: "ReviewArtifact" },
  }],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
  contract: graphShapeContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(schemaBranchConditionControlState.field.name, "verdict");
assert.equal(schemaBranchConditionControlState.operatorValue, "!=");
assert.equal(schemaBranchConditionControlState.operatorOptions.find((option) => option.value === "!=").disabled, true);
assert.match(schemaBranchConditionControlState.operatorOptions.find((option) => option.value === "!=").reason, /condition_operators/);
assert.equal(schemaBranchConditionControlState.previewLabel, 'Reviewer.verdict != "red"');
const branchParallelControlFlow = {
  steps: [
    {
      id: "input_1",
      type: "input",
      inputParams: [{ id: "p_route", name: "route", type: "enum" }],
    },
    { id: "review_step", type: "member", role: "m_reviewer" },
    {
      id: "parallel_1",
      type: "parallel",
      controllerRole: "m_reviewer",
      collection: "quorum",
      dependsMode: "custom_dependency",
      branches: [{ id: "br1", steps: [] }],
    },
  ],
};
const branchParallelControlState = controller.basicBranchParallelControlState({
  step: branchParallelControlFlow.steps[2],
  flow: branchParallelControlFlow,
  members: [
    ...members,
    { id: "m_writer", name: "Writer", role: "writer", schema: "DraftArtifact" },
  ],
  contract: graphShapeContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(branchParallelControlState.isParallel, true);
assert.equal(branchParallelControlState.panelTitle, "Parallel");
assert.equal(branchParallelControlState.controllerLabel, "Join member");
assert.equal(branchParallelControlState.controllerPlaceholderLabel, "— direct Meta lanes —");
assert.equal(branchParallelControlState.controllerRole, "m_reviewer");
assert.deepEqual(branchParallelControlState.memberOptions.map((option) => [option.value, option.label]), [
  ["m_reviewer", "Reviewer · reviewer"],
  ["m_writer", "Writer · writer"],
]);
assert.equal(branchParallelControlState.branchConditionTitle, "Branch conditions");
assert.equal(branchParallelControlState.fallbackTitle, "Fallback");
assert.equal(branchParallelControlState.dispatchLabel, "Dispatch mode");
assert.equal(branchParallelControlState.dispatchValue, "fan_out");
assert.equal(branchParallelControlState.dispatchOptions[0].label, "fan_out — broadcast to every lane");
assert.equal(branchParallelControlState.collectionLabel, "Collection policy (fan_in)");
assert.equal(branchParallelControlState.collectionValue, "quorum");
assert.equal(branchParallelControlState.collectionOptions[2].label, "quorum — require N branches");
assert.equal(branchParallelControlState.showQuorum, true);
assert.equal(branchParallelControlState.quorumLabel, "Quorum (N)");
assert.equal(branchParallelControlState.quorumPlaceholder, "required");
assert.equal(branchParallelControlState.dependencyLabel, "depends_on mode");
assert.equal(branchParallelControlState.dependencyValue, "custom_dependency");
assert.equal(branchParallelControlState.dependencyOptions[1].label, "any — any upstream node");
assert.equal(branchParallelControlState.selectedDependency.disabled, true);
assert.equal(branchParallelControlState.selectedDependency.label, "custom_dependency — not in Meta dependency_modes");
assert.equal(branchParallelControlState.selectedDependency.reason, "Unsupported by the Meta dependency_modes contract.");
assert.match(branchParallelControlState.selectedDependency.reason, /dependency_modes/);
const basicBranchControlState = controller.basicBranchParallelControlState({
  step: { id: "branch_1", type: "branch", branches: [{ id: "br1", condition: "", steps: [] }] },
  flow: {
    steps: [
      branchParallelControlFlow.steps[0],
      branchParallelControlFlow.steps[1],
      { id: "branch_1", type: "branch", branches: [] },
    ],
  },
  members,
  contract: graphShapeContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(basicBranchControlState.isParallel, false);
assert.equal(basicBranchControlState.panelTitle, "Branch");
assert.equal(basicBranchControlState.controllerLabel, "Route member");
assert.equal(basicBranchControlState.controllerPlaceholderLabel, "— direct Meta lanes —");
assert.equal(basicBranchControlState.addBranchLabel, "+ Add branch");
assert.equal(basicBranchControlState.branchConditionTitle, "Branch conditions");
assert.equal(basicBranchControlState.fallbackTitle, "Fallback");
assert.deepEqual(basicBranchControlState.conditionOptions.map((option) => [option.stepId, option.label || option.member?.name]), [
  ["params", "Input params"],
  ["review_step", "Reviewer"],
]);
const repeatControlContract = {
  mob_definition: {
    ...graphShapeContract.mob_definition,
    defaults: {
      ...graphShapeContract.mob_definition.defaults,
      repeat_iteration_input: "carry",
    },
    repeat_iteration_inputs: ["carry"],
  },
};
const repeatControlState = controller.basicRepeatControlState({
  step: {
    id: "repeat_1",
    type: "repeat",
    cond: { stepId: "loop_review", field: "verdict", op: "==", val: "green" },
    iterationInput: "carry",
    steps: [{ id: "loop_review", type: "member", role: "m_review" }],
  },
  members: [{ id: "m_review", name: "Reviewer", schema: "ReviewArtifact" }],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
  contract: repeatControlContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(repeatControlState.hasBodyMembers, true);
assert.equal(repeatControlState.panelIcon, "↻");
assert.equal(repeatControlState.panelTitle, "Repeat until");
assert.equal(repeatControlState.panelSub, "Loop the body, then evaluate the condition after each iteration");
assert.equal(repeatControlState.loopIdLabel, "loop_id");
assert.equal(repeatControlState.loopIdPlaceholder, "quality_loop");
assert.equal(repeatControlState.conditionTitle, "Until condition");
assert.equal(repeatControlState.conditionIntro, "Evaluated on a body member's structured output after each pass. The loop exits when it holds.");
assert.equal(repeatControlState.emptyBodyHint, "Add a member step inside the loop first — the condition reads its output schema.");
assert.equal(repeatControlState.memberPlaceholderLabel, "— member —");
assert.equal(repeatControlState.previewLabel, "until");
assert.equal(repeatControlState.previewFallback, "…");
assert.equal(repeatControlState.iterationInputLabel, "Iteration input — what each pass receives");
assert.equal(repeatControlState.maxIterationsLabel, "max_iterations");
assert.equal(repeatControlState.maxIterationsPlaceholder, "required");
assert.deepEqual(repeatControlState.tips, [
  "The body is its own FrameSpec — add member steps inside the loop.",
  "The condition reads a member's typed output (e.g. reviewer.verdict == green).",
  "max_iterations bounds the loop so it always terminates.",
]);
assert.deepEqual(repeatControlState.bodyMemberOptions.map((option) => [option.value, option.label]), [
  ["loop_review", "Reviewer"],
]);
assert.deepEqual(repeatControlState.fieldOptions.map((option) => [option.value, option.label]), [
  ["verdict", "verdict · enum"],
]);
assert.equal(repeatControlState.condField.name, "verdict");
assert.equal(repeatControlState.operatorValue, "==");
assert.equal(repeatControlState.repeatUntilExpression, 'Reviewer.verdict == "green"');
assert.deepEqual(repeatControlState.iterationInputOptions.map((option) => [option.value, option.label, option.disabled]), [
  ["", "runtime default", false],
  ["carry", "Carry — last body step's output feeds the next pass", false],
]);
assert.equal(repeatControlState.selectedIterationInput.value, "carry");
const missingRepeatControlState = controller.basicRepeatControlState({
  step: { id: "repeat_2", type: "repeat", cond: { stepId: "missing", field: "verdict", op: "==", val: "green" }, iterationInput: "previous" },
  members: [{ id: "m_review", name: "Reviewer", schema: "ReviewArtifact" }],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
  contract: repeatControlContract,
  basicView: hydratedCatalogs.basicView,
});
assert.equal(missingRepeatControlState.hasBodyMembers, false);
assert.equal(missingRepeatControlState.fieldPlaceholder, "(no schema)");
assert.equal(missingRepeatControlState.selectedIterationInput.disabled, true);
assert.match(missingRepeatControlState.selectedIterationInput.reason, /repeat_iteration_inputs/);
// ── adaptive layer panel + block state (server contract text only) ──
const adaptiveStudio = {
  members: [
    { id: "m_planner", name: "Planner", role: "lead", model: "openai/gpt-5" },
    { id: "m_worker", name: "Worker", role: "builder", model: "anthropic/claude-sonnet" },
  ],
  schemas: [{ id: "ReviewArtifact", fields: [{ id: "f_verdict", name: "verdict", type: "enum" }] }],
};
const adaptiveState = controller.adaptiveStepState(adaptiveStudio, {
  id: "s_adaptive",
  type: "adaptive",
  flowmasterId: "m_planner",
  prompt: "  plan each layer  \nfrom prior results ",
  objectiveClass: " code-review ",
  resultSchema: "ReviewArtifact",
  profileTemplateIds: ["m_worker"],
  targetSurfaces: ["cli", "rpc"],
  allowInlineProfiles: true,
  limits: { maxDepth: 6 },
}, repeatControlContract, hydratedCatalogs.basicView);
assert.equal(adaptiveState.panelIcon, "❖");
assert.equal(adaptiveState.panelTitle, "Adaptive layer");
assert.equal(adaptiveState.panelSub, "A FlowMaster designs the next mob layer at runtime from prior results — dynamic width & depth. Each plan is validated, then run as a normal mob flow.");
assert.equal(adaptiveState.flowmasterLabel, "FlowMaster (planning agent)");
assert.equal(adaptiveState.flowmasterPlaceholderLabel, "— select member —");
assert.equal(adaptiveState.flowmasterId, "m_planner");
assert.deepEqual(adaptiveState.flowmasterOptions.map((option) => [option.value, option.label]), [
  ["m_planner", "Planner · openai/gpt-5"],
  ["m_worker", "Worker · anthropic/claude-sonnet"],
]);
assert.equal(adaptiveState.selectedFlowmaster.id, "m_planner");
assert.equal(adaptiveState.promptLabel, "Planning prompt");
assert.equal(adaptiveState.promptPlaceholder, "How should the FlowMaster decide each layer?");
// Free text passes through VERBATIM — leading/trailing/inner whitespace kept.
assert.equal(adaptiveState.promptValue, "  plan each layer  \nfrom prior results ");
assert.equal(adaptiveState.objectiveLabel, "Objective class");
assert.equal(adaptiveState.objectivePlaceholder, "code-review");
assert.equal(adaptiveState.objectiveValue, " code-review ");
assert.equal(adaptiveState.resultSchemaLabel, "LayerResult schema");
assert.equal(adaptiveState.resultSchemaPlaceholderLabel, "— LayerResult (untyped) —");
assert.equal(adaptiveState.resultSchemaValue, "ReviewArtifact");
assert.deepEqual(adaptiveState.schemaOptions.map((option) => [option.value, option.label]), [
  ["ReviewArtifact", "ReviewArtifact"],
]);
assert.equal(adaptiveState.selectedSchema.id, "ReviewArtifact");
assert.equal(adaptiveState.profilesTitle, "Profile templates the layer may spawn");
assert.equal(adaptiveState.profilesHint, "No members are baked in — the FlowMaster picks counts & roles per layer from these shapes.");
assert.equal(adaptiveState.profilesAgentsHint, "Edit member shapes in the Agents tab.");
assert.equal(adaptiveState.profilesEmptyHint, "No members yet — define some in the Agents tab.");
assert.equal(adaptiveState.hasMembers, true);
assert.deepEqual(adaptiveState.templateRows.map((row) => [row.id, row.label, row.role, row.model, row.on]), [
  ["m_planner", "Planner", "lead", "openai/gpt-5", false],
  ["m_worker", "Worker", "builder", "anthropic/claude-sonnet", true],
]);
assert.equal(adaptiveState.surfacesLabel, "Target surfaces");
assert.deepEqual(adaptiveState.surfaceToggles, [
  { id: "cli", label: "cli", on: true },
  { id: "rpc", label: "rpc", on: true },
]);
assert.equal(adaptiveState.inlineProfilesLabel, "Allow inline profiles");
assert.equal(adaptiveState.allowInlineProfiles, true);
assert.equal(adaptiveState.limitsTitle, "Limits");
assert.equal(adaptiveState.limitsAdvancedLabel, "Advanced limits");
// Limits merge: the step patch overlays the server contract defaults.
assert.deepEqual(adaptiveState.primaryLimitRows.map((row) => [row.key, row.label, row.value]), [
  ["maxDepth", "Max depth", 6],
  ["maxMembersPerLayer", "Max members / layer", 20],
  ["maxTotalSpawnedMembers", "Max total members", 60],
]);
assert.deepEqual(adaptiveState.advancedLimitRows.map((row) => [row.key, row.value]), [
  ["maxTotalDecisions", 12],
  ["maxRepairAttempts", 2],
  ["maxLayerFailures", 2],
  ["maxAttemptsPerLayer", 2],
  ["maxActiveMembers", 20],
  ["maxRetainedLayerMobs", 4],
  ["maxWallClockMs", 1800000],
  ["maxAggregateTokens", 2000000],
  ["maxAggregateToolCalls", 500],
]);
assert.equal(adaptiveState.limitRows.length, 12);
assert.deepEqual(adaptiveState.tips, [
  "Dynamic width: the layer is generated with the right number of members.",
  "Dynamic depth: the loop repeats until the FlowMaster returns Finish or limits hit.",
  "Every layer runs through MobMachine and emits a validated LayerResult.",
]);
assert.equal(adaptiveState.stepCardTitle, "Adaptive layer");
assert.deepEqual(adaptiveState.block, {
  headText: "ADAPTIVE LAYER · synthesized at runtime · max depth 6",
  flowmasterKicker: "FLOWMASTER · plan",
  flowmasterTitle: "Planner",
  flowmasterDesc: "Designs the next LayerPlan from prior results",
  planConnector: "LayerPlan ✓",
  layerKicker: "LAYERMOB · ≤ 20 members",
  chips: [{ id: "m_worker", label: "Worker", suffix: "×N" }],
  emptyProfilesLabel: "no profiles yet",
  collectConnector: "collector → ReviewArtifact",
  loopBackText: "↑ loop until Finish · or depth 6 / budget",
});
// Empty draft: defaults fall back to the server contract, fallback copy renders.
const emptyAdaptiveState = controller.adaptiveStepState({ members: [], schemas: [] }, {
  id: "s_adaptive_blank",
  type: "adaptive",
  flowmasterId: "",
  prompt: "",
  objectiveClass: "",
  resultSchema: "",
  profileTemplateIds: [],
  targetSurfaces: [],
  allowInlineProfiles: false,
  limits: {},
}, repeatControlContract, hydratedCatalogs.basicView);
assert.equal(emptyAdaptiveState.hasMembers, false);
assert.equal(emptyAdaptiveState.selectedFlowmaster, null);
assert.deepEqual(emptyAdaptiveState.surfaceToggles, [
  { id: "cli", label: "cli", on: false },
  { id: "rpc", label: "rpc", on: false },
]);
assert.deepEqual(emptyAdaptiveState.primaryLimitRows.map((row) => [row.key, row.value]), [
  ["maxDepth", 4],
  ["maxMembersPerLayer", 20],
  ["maxTotalSpawnedMembers", 60],
]);
assert.equal(emptyAdaptiveState.block.headText, "ADAPTIVE LAYER · synthesized at runtime · max depth 4");
assert.equal(emptyAdaptiveState.block.flowmasterTitle, "—");
assert.deepEqual(emptyAdaptiveState.block.chips, []);
assert.equal(emptyAdaptiveState.block.collectConnector, "collector → LayerResult");
assert.equal(emptyAdaptiveState.block.loopBackText, "↑ loop until Finish · or depth 4 / budget");
// Card-row state for adaptive steps comes from the contract card title.
assert.deepEqual(controller.basicStepCardState({
  step: { type: "adaptive", flowmasterId: "m_planner", objectiveClass: "code-review" },
  members: adaptiveStudio.members,
  contract: repeatControlContract,
  basicView: hydratedCatalogs.basicView,
}), {
  icon: "❖",
  iconTint: "member",
  title: "Adaptive layer",
  desc: "code-review",
  configured: true,
  isFlowCard: true,
});
assert.equal(controller.basicStepCardState({
  step: { type: "adaptive", flowmasterId: "", objectiveClass: "" },
  members: adaptiveStudio.members,
  contract: repeatControlContract,
  basicView: hydratedCatalogs.basicView,
}).configured, false);
// Member deletion reconciles adaptive refs but never drops the step
// (mirrors Meta's prune_adaptive_step_member_refs).
const prunedAdaptiveFlow = controller.reconcileFlowMemberSteps({
  name: "adaptive-prune-proof",
  steps: [
    { id: "keep_member", type: "member", role: "m_reviewer" },
    {
      id: "s_adaptive",
      type: "adaptive",
      flowmasterId: "m_deleted",
      prompt: " keep me verbatim ",
      objectiveClass: "code-review",
      resultSchema: "",
      profileTemplateIds: ["m_reviewer", "m_deleted"],
      targetSurfaces: ["cli"],
      allowInlineProfiles: false,
      limits: {},
    },
  ],
}, [members[0]]);
assert.deepEqual(prunedAdaptiveFlow.steps.map((step) => step.id), ["keep_member", "s_adaptive"]);
assert.equal(prunedAdaptiveFlow.steps[1].flowmasterId, "");
assert.deepEqual(prunedAdaptiveFlow.steps[1].profileTemplateIds, ["m_reviewer"]);
assert.equal(prunedAdaptiveFlow.steps[1].prompt, " keep me verbatim ");
// The graph instance's embedded adaptive payload reconciles identically so
// graph_derives_flow equivalence keeps holding.
const adaptiveGraphSync = controller.reconcileGraphMemberInstances({
  instances: [
    {
      id: "s_adaptive",
      kind: "adaptive",
      label: "adaptive layer",
      memberId: null,
      adaptive: {
        id: "s_adaptive",
        type: "adaptive",
        flowmasterId: "m_deleted",
        prompt: " keep me verbatim ",
        objectiveClass: "code-review",
        resultSchema: "",
        profileTemplateIds: ["m_reviewer", "m_deleted"],
        targetSurfaces: ["cli"],
        allowInlineProfiles: false,
        limits: {},
      },
    },
    { id: "review_inst", memberId: "m_reviewer" },
    { id: "deleted_inst", memberId: "m_deleted" },
  ],
  edges: [
    { id: "keep_adaptive_review", from: "s_adaptive", to: "review_inst", kind: "next" },
    { id: "drop_deleted", from: "review_inst", to: "deleted_inst", kind: "next" },
  ],
}, [members[0]]);
assert.deepEqual(adaptiveGraphSync.instances.map((instance) => instance.id), ["s_adaptive", "review_inst"]);
assert.equal(adaptiveGraphSync.instances[0].adaptive.flowmasterId, "");
assert.deepEqual(adaptiveGraphSync.instances[0].adaptive.profileTemplateIds, ["m_reviewer"]);
assert.equal(adaptiveGraphSync.instances[0].adaptive.prompt, " keep me verbatim ");
assert.deepEqual(adaptiveGraphSync.edges.map((edge) => edge.id), ["keep_adaptive_review"]);
// Adaptive graph instances surface the server-authoritative locked copy.
const adaptiveInstanceState = controller.graphInstanceControlState({
  inst: { id: "s_adaptive", kind: "adaptive", label: "adaptive layer", memberId: null, col: 1, row: 0 },
  instances: [],
  members: adaptiveStudio.members,
  schemas: adaptiveStudio.schemas,
  graphView: hydratedCatalogs.graphView,
});
assert.equal(adaptiveInstanceState.adaptiveLockedTitle, "Adaptive layer node");
assert.equal(adaptiveInstanceState.adaptiveLockedHint, "Adaptive layers are authored in Basic mode — FlowMaster, prompt, profile templates, and limits are edited there. Deleting this node deletes the adaptive step from the flow.");
assert.equal(adaptiveInstanceState.deleteLabel, "DELETE");
{
  const patch = controller.basicBranchAddPatch(
    { type: "branch", branches: [{ id: "br1", label: "Path 1", condition: "", steps: [] }] },
    { basicView: { ...hydratedCatalogs.basicView, branchConditionRowTitlePrefix: "Path" } }
  );
  assert.equal(patch.branches.length, 2);
  assert.equal(patch.branches[1].id, "br_1");
  assert.equal(patch.branches[1].label, "Path 2");
  assert.equal(patch.branches[1].condition, "");
  assert.deepEqual(patch.branches[1].steps, []);
}
{
  const patch = controller.basicBranchAddPatch(
    { type: "parallel", branches: [{ id: "br1", label: "Path 1", steps: [] }] },
    { basicView: { ...hydratedCatalogs.basicView, branchConditionRowTitlePrefix: "Path" } }
  );
  assert.equal(patch.branches.length, 2);
  assert.equal(patch.branches[1].id, "br_1");
  assert.equal(patch.branches[1].label, "Path 2");
  assert.equal(Object.prototype.hasOwnProperty.call(patch.branches[1], "condition"), false);
  assert.deepEqual(patch.branches[1].steps, []);
}
{
  const flow = {
    steps: [{
      id: "router",
      type: "branch",
      branches: [
        { id: "br_1", steps: [] },
        { id: "br_2", steps: [] },
      ],
      fallback: [],
    }],
  };
  const patch = controller.basicBranchAddPatch(flow.steps[0], { flow });
  assert.equal(patch.branches[2].id, "br_3");
}
{
  const flow = {
    steps: [{
      id: "fanout",
      type: "parallel",
      branches: [
        { id: "br_1", steps: [] },
        { id: "br_2", steps: [] },
      ],
    }],
  };
  const patch = controller.basicBranchAddPatch(flow.steps[0], { flow });
  assert.equal(patch.branches[2].id, "br_3");
  assert.equal(Object.prototype.hasOwnProperty.call(patch.branches[2], "condition"), false);
}

assert.equal(controller.contractDefaultValue({
  mob_definition: {
    defaults: { dispatch_mode: "fan_out" },
    dispatch_modes: ["one_to_one"],
  },
}, "dispatch_mode"), "");

assert.deepEqual(controller.agentNavigationProjection("m_left"), {
  view: "agents",
  addAt: null,
  selection: { kind: "agent", id: "m_left" },
});
assert.deepEqual(controller.agentNavigationProjection("  "), {
  view: "agents",
  addAt: null,
  selection: null,
});

assert.equal(controller.flowStepTemplate({ kind: "parallel" }, {
  mob_definition: {
    editor_flow_step_types: ["parallel"],
  },
}), null);

const parallelTemplate = controller.flowStepTemplate({ kind: "parallel" }, graphShapeContract, {
  basicView: { ...hydratedCatalogs.basicView, branchConditionRowTitlePrefix: "Lane" },
});
assert(parallelTemplate.id.startsWith("s_"));
assert.equal(parallelTemplate.type, "parallel");
assert.equal(parallelTemplate.dispatch, "fan_out");
assert.equal(parallelTemplate.collection, "all");
assert.equal(parallelTemplate.dependsMode, "all");
assert.equal(parallelTemplate.branches.length, 2);
assert.deepEqual(parallelTemplate.branches.map((branch) => branch.label), ["Lane 1", "Lane 2"]);

const memberTemplate = controller.flowStepTemplate({ kind: "member", id: "m_left" }, graphShapeContract);
assert(memberTemplate.id.startsWith("s_"));
assert.equal(memberTemplate.type, "member");
assert.equal(memberTemplate.role, "m_left");
assert.equal(memberTemplate.dependsMode, "all");

const collisionFlowTemplate = controller.flowStepTemplate({ kind: "parallel" }, graphShapeContract, {
  basicView: { ...hydratedCatalogs.basicView, branchConditionRowTitlePrefix: "Lane" },
  flow: {
    steps: [
      { id: "s_1", type: "member", role: "m_left" },
      {
        id: "s_2",
        type: "branch",
        branches: [
          { id: "br_1", steps: [] },
          { id: "br_2", steps: [] },
        ],
        fallback: [],
      },
    ],
  },
});
assert.equal(collisionFlowTemplate.id, "s_3");
assert.deepEqual(collisionFlowTemplate.branches.map((branch) => branch.id), ["br_3", "br_4"]);
assert.deepEqual(collisionFlowTemplate.branches.map((branch) => branch.label), ["Lane 1", "Lane 2"]);
assert.deepEqual(controller.flowStepInsertPatch({
  name: "collision-proof",
  steps: [
    { id: "s_1", type: "member", role: "m_left" },
    { id: "s_2", type: "member", role: "m_right" },
  ],
}, { lane: "main", index: 2 }, controller.flowStepTemplate({ kind: "member", id: "m_left" }, graphShapeContract, {
  flow: {
    steps: [
      { id: "s_1", type: "member", role: "m_left" },
      { id: "s_2", type: "member", role: "m_right" },
    ],
  },
}), { members: graphMembers }).steps.map((step) => step.id), ["s_1", "s_2", "s_3"]);

const registryRows = [
  { id: "f_existing", name: "Existing", version: "old", stage: "valid", validation: { ok: true }, revision: 7, draft_etag: "f_existing:7" },
  { id: "f_other", name: "Other", version: "old", stage: "valid", validation: { ok: true } },
];
const draftRows = controller.flowRegistryMarkDraftPatch(registryRows, "f_existing");
assert.notEqual(draftRows, registryRows);
assert.equal(draftRows[0].stage, "draft");
assert.equal(draftRows[0].validation, null);
assert.equal(draftRows[1], registryRows[1]);

const registryDocument = {
  name: "Planner Coder Reviewer",
  mob_id: "planner_coder_reviewer",
  schema_version: "1.0",
  flow: { name: "Mob Flow" },
};
const selectionRows = [
  { id: "f_existing", name: "Existing", version: "old", stage: "valid", validation: { ok: true }, document: registryDocument },
  { id: "f_draft", name: "Draft", version: "draft", stage: "draft" },
];
const documentSelection = controller.flowRegistrySelectionState(selectionRows, "f_existing");
assert.equal(documentSelection.found, true);
assert.equal(documentSelection.hasDocument, true);
assert.equal(documentSelection.row, selectionRows[0]);
assert.deepEqual(documentSelection.hydration.result, {
  document: registryDocument,
  validation: { ok: true },
});
assert.deepEqual(documentSelection.hydration.options, {
  id: "f_existing",
  flowRow: selectionRows[0],
  addToRegistry: false,
});
assert.equal(documentSelection.fallback, null);

// A draft listed as a summary (no document) opens through meta/mobpacks/get.
const fetchSelection = controller.flowRegistrySelectionState(selectionRows, "f_draft");
assert.equal(fetchSelection.found, true);
assert.equal(fetchSelection.hasDocument, false);
assert.equal(fetchSelection.hydration, null);
assert.equal(fetchSelection.fallback, null);
assert.deepEqual(fetchSelection.fetch, { id: "f_draft" });
assert.equal(fetchSelection.error, undefined);
assert.equal(controller.flowRegistryFallbackOpenTransition, undefined);
// The row get returns hydrates like any row with a document.
const fetchedRow = { id: "f_draft", name: "Draft", stage: "valid", validation: { ok: true }, document: registryDocument, revision: 3 };
const fetchedSelection = controller.flowRegistrySelectionState([fetchedRow], "f_draft");
assert.deepEqual(fetchedSelection.hydration.result, { document: registryDocument, validation: { ok: true } });
assert.deepEqual(fetchedSelection.hydration.options, { id: "f_draft", flowRow: fetchedRow, addToRegistry: false });
// A runtime projection without a document still fails closed.
const runtimeSelection = controller.flowRegistrySelectionState([
  { id: "f_runtime", name: "Runtime", stage: "valid", runtime_projection: true },
], "f_runtime");
assert.equal(runtimeSelection.error, "missing_registry_document");
assert.equal(runtimeSelection.fetch, undefined);

// The library lists drafts from summaries: rows without documents keep their
// labels, description and optimistic-concurrency fields.
const summaryRows = controller.flowRegistryRowsFromBackend([
  {
    id: "f_summary",
    name: "Summary Draft",
    version: "0.1.0",
    stage: "valid",
    trigger: "Meta authoring draft",
    source: "meta/mobpacks/save",
    description: "Plans and reviews",
    revision: 5,
    draft_revision: 5,
    etag: "f_summary:5",
    draft_etag: "f_summary:5",
    updated_at_unix_ms: 1000,
    can_undo: true,
    can_redo: false,
  },
]);
assert.equal(summaryRows.length, 1);
assert.equal(summaryRows[0].document, undefined);
assert.equal(summaryRows[0].validation, null);
assert.deepEqual(
  controller.flowRegistryDraftGuard(summaryRows[0]),
  { id: "f_summary", expected_revision: 5, expected_etag: "f_summary:5" },
  "summary rows must keep the draft guard",
);
const summaryView = controller.flowRegistryViewState(summaryRows, null, { nowUnixMs: 61000 });
assert.deepEqual(
  summaryView.sections[0].rows.map((row) => [row.id, row.name, row.description, row.stage, row.updated]),
  [["f_summary", "Summary Draft", "Plans and reviews", "valid", "1m ago"]],
);
assert.deepEqual(controller.flowRegistrySelectionState(summaryRows, "f_summary").fetch, { id: "f_summary" });

const missingSelection = controller.flowRegistrySelectionState(selectionRows, "missing");
assert.equal(missingSelection.found, false);
assert.equal(missingSelection.row, null);
assert.equal(missingSelection.hydration, null);
assert.equal(missingSelection.fallback, null);

// Breadcrumb quick-switcher: draft registry rows only (runtime projections
// stay library-only), current row flagged, view-all label from the deploy view.
const switcherState = controller.mobSwitcherState([
  { id: "f_existing", name: "Existing", stage: "valid", document: registryDocument },
  { id: "f_draft", name: "Draft", stage: "draft" },
  { id: "f_runtime", name: "Runtime", stage: "valid", runtime_projection: true, document: registryDocument },
  { name: "no id" },
], "f_draft", { deployView: TEST_DEPLOY_VIEW });
assert.deepEqual(switcherState, {
  rows: [
    { id: "f_existing", name: "Existing", stage: "valid", className: "crumb-switcher__item" },
    { id: "f_draft", name: "Draft", stage: "draft", className: "crumb-switcher__item is-current" },
  ],
  viewAllLabel: "view all mobs",
});
assert.deepEqual(controller.mobSwitcherState(null, "", {}), { rows: [], viewAllLabel: "" });

assert.equal(controller.flowImportedIdFromDocument({
  name: "Imported Quality Flow",
}, {}, []), "f_imported_quality_flow");
assert.equal(controller.flowImportedIdFromDocument({
  mob_id: "imported_quality_flow",
}, {}, [{ id: "f_imported_quality_flow" }]), "f_imported_quality_flow_2");
assert.equal(controller.flowImportedIdFromDocument({}, {
  source_name: "special.mobpack",
}, []), "f_specialmobpack");
assert.equal(controller.flowImportedIdFromDocument({}, {}, []), "");

const rememberedRows = controller.flowRegistryRememberDocumentPatch(registryRows, {
  currentFlowId: "f_existing",
  document: registryDocument,
  validation: { ok: false },
  stage: "draft",
});
assert.equal(rememberedRows[0].name, "Planner Coder Reviewer");
assert.equal(rememberedRows[0].version, "1.0");
assert.equal(rememberedRows[0].document, registryDocument);
assert.deepEqual(rememberedRows[0].validation, { ok: false });

const emptyPersistence = controller.flowRegistryDocumentPersistence({
  currentFlowId: "",
  document: registryDocument,
});
assert.equal(emptyPersistence.ok, false);
assert.equal(emptyPersistence.changed, false);
assert.equal(emptyPersistence.rowPatch, null);

const draftPersistence = controller.flowRegistryDocumentPersistence({
  currentFlowId: "f_existing",
  document: registryDocument,
  stage: "published",
  validation: null,
});
assert.equal(draftPersistence.ok, true);
assert.equal(draftPersistence.changed, true);
assert.equal(draftPersistence.signature, `f_existing\n${JSON.stringify(registryDocument)}`);
assert.equal(draftPersistence.rowPatch.stage, "draft");
assert.equal(draftPersistence.rowPatch.currentFlowId, "f_existing");
assert.equal(draftPersistence.rowPatch.document, registryDocument);
assert.equal(draftPersistence.rowPatch.validation, null);

const validatedPersistence = controller.flowRegistryDocumentPersistence({
  currentFlowId: "f_existing",
  document: registryDocument,
  stage: "published",
  validation: { ok: true },
});
assert.equal(validatedPersistence.rowPatch.stage, "published");
assert.deepEqual(validatedPersistence.rowPatch.validation, { ok: true });

const seededHydrationNoop = controller.flowRegistryDocumentPersistence({
  currentFlowId: "f_existing",
  document: registryDocument,
  stage: "valid",
  validation: null,
  previousSignature: validatedPersistence.signature,
  skipIfUnchanged: true,
});
assert.equal(seededHydrationNoop.ok, true);
assert.equal(seededHydrationNoop.changed, false);
assert.equal(seededHydrationNoop.rowPatch, null);

const unchangedPersistence = controller.flowRegistryDocumentPersistence({
  currentFlowId: "f_existing",
  document: registryDocument,
  previousSignature: draftPersistence.signature,
  skipIfUnchanged: true,
});
assert.equal(unchangedPersistence.ok, true);
assert.equal(unchangedPersistence.changed, false);
assert.equal(unchangedPersistence.signature, draftPersistence.signature);
assert.equal(unchangedPersistence.rowPatch, null);

const persistedRegistryProjection = controller.flowRegistryPersistDocumentProjection(registryRows, {
  currentFlowId: "f_existing",
  document: registryDocument,
  validation: { ok: true },
  stage: "published",
});
assert.equal(persistedRegistryProjection.ok, true);
assert.equal(persistedRegistryProjection.changed, true);
assert.equal(persistedRegistryProjection.rowPatch.expectedRevision, 7);
assert.equal(persistedRegistryProjection.rowPatch.expectedEtag, "f_existing:7");
assert.equal(persistedRegistryProjection.rows[0].stage, "published");
assert.deepEqual(persistedRegistryProjection.rows[0].validation, { ok: true });
assert.equal(persistedRegistryProjection.rows[0].document, registryDocument);
assert.equal(persistedRegistryProjection.rows[1], registryRows[1]);
const runtimeProjectionRows = [{
  id: "runtime_main",
  source: "meta/runtime/flow_projection",
  document_kind: "runtime_projection",
  runtime_projection: true,
  runtime_mob_id: "runtime_mob",
  runtime_flow_id: "main",
  document: registryDocument,
}];
assert.equal(controller.flowRegistryRowIsRuntimeProjection(runtimeProjectionRows[0]), true);
const runtimeProjectionPersistence = controller.flowRegistryPersistDocumentProjection(runtimeProjectionRows, {
  currentFlowId: "runtime_main",
  document: registryDocument,
  validation: { ok: true },
  stage: "valid",
});
assert.equal(runtimeProjectionPersistence.ok, false);
assert.equal(runtimeProjectionPersistence.changed, false);
assert.equal(runtimeProjectionPersistence.reason, "runtime_projection_read_only");
assert.equal(runtimeProjectionPersistence.rowPatch, null);
assert.equal(runtimeProjectionPersistence.rows, runtimeProjectionRows);
assert.deepEqual(controller.flowRegistryDraftGuard(registryRows[0], "fallback_id"), {
  id: "f_existing",
  expected_revision: 7,
  expected_etag: "f_existing:7",
});
assert.deepEqual(controller.flowRegistryDraftGuard({ id: "new_draft" }), {});

// Regression (library rows strip revision/etag): rows hydrated from the
// backend list must carry the optimistic-concurrency fields through, or the
// first autosave/apply/undo/deploy after opening a library draft runs
// UNGUARDED (expected_revision null) until a save response replaces the row.
const backendHydratedRows = controller.flowRegistryRowsFromBackend([
  {
    id: "f_backend",
    document: registryDocument,
    validation: { ok: true },
    stage: "valid",
    revision: 4,
    draft_revision: 4,
    draft_etag: "f_backend:4",
  },
]);
assert.equal(backendHydratedRows.length, 1);
assert.equal(backendHydratedRows[0].revision, 4);
assert.equal(backendHydratedRows[0].draft_revision, 4);
assert.equal(backendHydratedRows[0].draft_etag, "f_backend:4");
assert.deepEqual(
  controller.flowRegistryDraftGuard(backendHydratedRows[0]),
  { id: "f_backend", expected_revision: 4, expected_etag: "f_backend:4" },
  "backend-hydrated library rows must produce a guarded draft guard",
);

// Regression (save-conflict loop): the conflict handler refreshes the local
// row's revision/etag from the authoritative server row, preserving the local
// document, so the re-save carries a revision the server accepts instead of
// re-sending the stale guard forever.
const localEditedDocument = { ...registryDocument, name: "Locally Edited" };
const conflictedRows = [
  {
    id: "f_backend",
    document: localEditedDocument,
    validation: null,
    stage: "draft",
    revision: 4,
    draft_revision: 4,
    draft_etag: "f_backend:4",
  },
];
const refreshedRows = controller.flowRegistryRefreshGuardPatch(conflictedRows, "f_backend", {
  id: "f_backend",
  document: { name: "Server Wins" },
  revision: 9,
  draft_revision: 9,
  draft_etag: "f_backend:9",
});
assert.notEqual(refreshedRows, conflictedRows, "guard refresh must produce a new rows array");
assert.equal(refreshedRows[0].revision, 9, "revision must advance to the server's current");
assert.equal(refreshedRows[0].draft_etag, "f_backend:9");
assert.equal(
  refreshedRows[0].document,
  localEditedDocument,
  "guard refresh must preserve the in-progress local document, not adopt the server's",
);
assert.deepEqual(
  controller.flowRegistryDraftGuard(refreshedRows[0]),
  { id: "f_backend", expected_revision: 9, expected_etag: "f_backend:9" },
  "the re-save must now carry the refreshed (acceptable) revision",
);
// A server row with no concurrency fields leaves the local row untouched
// (same reference) rather than clobbering the guard.
assert.equal(
  controller.flowRegistryRefreshGuardPatch(conflictedRows, "f_backend", { id: "f_backend" }),
  conflictedRows,
);

const unchangedRegistryProjection = controller.flowRegistryPersistDocumentProjection(registryRows, {
  currentFlowId: "f_existing",
  document: registryDocument,
  previousSignature: draftPersistence.signature,
  skipIfUnchanged: true,
});
assert.equal(unchangedRegistryProjection.ok, true);
assert.equal(unchangedRegistryProjection.changed, false);
assert.equal(unchangedRegistryProjection.signature, draftPersistence.signature);
assert.equal(unchangedRegistryProjection.rows, registryRows);

const invalidRegistryProjection = controller.flowRegistryPersistDocumentProjection(registryRows, {
  currentFlowId: "",
  document: registryDocument,
});
assert.equal(invalidRegistryProjection.ok, false);
assert.equal(invalidRegistryProjection.changed, false);
assert.equal(invalidRegistryProjection.rows, registryRows);

const persistedOutcomeProjection = controller.flowRegistryPersistOutcomeProjection(registryRows, {
  currentFlowId: "f_existing",
  outcome: {
    document: registryDocument,
    validation: { ok: true },
    validationRows: [{ kind: "ok", head: "valid" }],
    stage: "valid",
  },
});
assert.equal(persistedOutcomeProjection.ok, true);
assert.equal(persistedOutcomeProjection.changed, true);
assert.equal(persistedOutcomeProjection.persistence.rowPatch.expectedRevision, 7);
assert.equal(persistedOutcomeProjection.persistence.rowPatch.expectedEtag, "f_existing:7");
assert.equal(persistedOutcomeProjection.stage, "valid");
assert.deepEqual(persistedOutcomeProjection.validationRows, [{ kind: "ok", head: "valid" }]);
assert.deepEqual(persistedOutcomeProjection.rows[0].validation, { ok: true });
assert.equal(persistedOutcomeProjection.rows[0].stage, "valid");

const invalidOutcomeProjection = controller.flowRegistryPersistOutcomeProjection(registryRows, {
  currentFlowId: "f_existing",
  outcome: { stage: "draft", validationRows: [{ kind: "crit" }] },
});
assert.equal(invalidOutcomeProjection.ok, false);
assert.equal(invalidOutcomeProjection.changed, false);
assert.equal(invalidOutcomeProjection.stage, "draft");
assert.deepEqual(invalidOutcomeProjection.validationRows, [{ kind: "crit" }]);
assert.equal(invalidOutcomeProjection.rows, registryRows);

const importedRow = controller.flowRegistryRowFromDocument({
  id: "f_imported",
  document: registryDocument,
  validation: { ok: true },
  sourceLabel: "upload.mobpack",
  source: "file:///tmp/upload.mobpack",
  fallbackVersion: "imported",
});
assert.equal(importedRow.name, "Planner Coder Reviewer");
assert.equal(importedRow.version, "1.0");
assert.equal(importedRow.stage, "valid");
assert.equal(importedRow.trigger, "upload.mobpack");
assert.equal(importedRow.source, "file:///tmp/upload.mobpack");

const storedGraphDocument = {
  name: "Stored Graph Import",
  mob_id: "stored_graph_import",
  schema_version: "1.0",
  members: [{ id: "reviewer", name: "Reviewer", role: "reviewer", profileBinding: "inline", runtimeMode: "turn_driven" }],
  schemas: [{ id: "Verdict", fields: [] }],
  flow: { name: "Stored Graph Import", steps: [{ id: "step_1", type: "member", role: "reviewer", task: "Review", instruction: "Review." }] },
  instances: [{ id: "kept_node", memberId: "reviewer", col: 4, row: 2 }],
  edges: [{ id: "kept_edge", from: "kept_node", to: "done", kind: "next" }],
  frames: [{ id: "kept_frame", kind: "Manual", col: 4, row: 2 }],
  skill_realms: [{ id: "imported", default: true, skills: [{ id: "mob.imported", label: "Imported", content: "Do imported work." }] }],
};
const hydratedStored = controller.hydrateMobpackDocumentState({
  document: storedGraphDocument,
  validation: {
    ok: true,
    display_rows: [{
      kind: "ok",
      glyph: "✓",
      head: "Imported mobpack validated",
      sub: "mob.toml",
      meta: "import.ok",
    }],
  },
  source_label: "upload.mobpack",
  source: "file:///tmp/stored.mobpack",
}, {
  id: "f_stored",
  deployDefaults: testDeploySettings(),
  mobDefaults: controller.mobDefaultsFromSchema(TEST_SCHEMA),
  contractSkillRealms: [{ id: "contract", skills: [{ id: "mob.contract", label: "Contract", content: "Contract skill." }] }],
});
assert.equal(hydratedStored.id, "f_stored");
assert.equal(hydratedStored.stage, "valid");
assert.equal(hydratedStored.flow.name, "Stored Graph Import");
assert.deepEqual(hydratedStored.members, storedGraphDocument.members);
assert.deepEqual(hydratedStored.schemas, storedGraphDocument.schemas);
assert.deepEqual(hydratedStored.graphProjection.instances, storedGraphDocument.instances);
assert.deepEqual(hydratedStored.graphProjection.edges, storedGraphDocument.edges);
assert.deepEqual(hydratedStored.graphProjection.frames, storedGraphDocument.frames);
assert.deepEqual(hydratedStored.skillRealms.map(realm => realm.id), ["imported", "contract"]);
assert.equal(hydratedStored.deploySettings.command, "rkat mob run");
assert.equal(hydratedStored.mobSettings.backendDefault, "session");
assert.equal(hydratedStored.registryRow.name, "Stored Graph Import");
assert.equal(hydratedStored.registryRow.source, "file:///tmp/stored.mobpack");
assert.equal(hydratedStored.validationRows[0].head, "Imported mobpack validated");
assert.equal(hydratedStored.validationRows[0].meta, "import.ok");

const apiGraphHydrated = controller.hydrateMobpackDocumentState({
  document: storedGraphDocument,
  graph_projection: {
    source: "meta/mobpacks/graph_projection",
    instances: [{ id: "api_step_1", memberId: "reviewer", col: 2, row: 1 }],
    edges: [{ id: "api_edge_1", from: "api_step_1", to: "done", kind: "next" }],
    frames: [{ id: "api_frame_1", kind: "Branch", col: 1, row: 1 }],
    validation: { ok: true },
  },
  validation: { ok: true },
}, {
  id: "f_api_graph",
  contract: graphProjectionTestContract,
});
assert.deepEqual(apiGraphHydrated.graphProjection.instances.map(instance => instance.id), ["api_step_1"]);
assert.deepEqual(apiGraphHydrated.graphProjection.edges.map(edge => edge.id), ["api_edge_1"]);
assert.deepEqual(apiGraphHydrated.graphProjection.frames.map(frame => frame.id), ["api_frame_1"]);
assert.equal(apiGraphHydrated.graphProjection.source, "meta/mobpacks/graph_projection");
assert.equal(controller.graphProjectionFromMetaResult(apiGraphHydrated.graphProjection).instances[0].id, "api_step_1");

const operationProjection = controller.authoringProjectionFromOperationResult({
  source: "meta/mobpacks/apply_operation",
  operation: "add_agent_definition",
  document: {
    ...storedGraphDocument,
    schemas: [{ id: "AddedVerdict", fields: [{ id: "f_verdict", name: "verdict", type: "string" }] }],
    skill_realms: [{ id: "meta/editor-inline", skills: [{ id: "mob.editor.review", label: "Review", source: "inline", content: "Review carefully." }] }],
    members: [
      ...storedGraphDocument.members,
      { id: "m_added", name: "Added", role: "added", profileBinding: "inline", runtimeMode: "turn_driven", skills: ["mob.editor.review"] },
    ],
  },
  selection: { kind: "agent", id: "m_added" },
  validation: { ok: true },
}, {
  deployDefaults: testDeploySettings(),
  mobDefaults: controller.mobDefaultsFromSchema(TEST_SCHEMA),
});
assert.equal(operationProjection.flow.name, "Stored Graph Import");
assert.equal(operationProjection.members.at(-1).id, "m_added");
assert.equal(operationProjection.schemas[0].id, "AddedVerdict");
assert.equal(operationProjection.skillRealms[0].skills[0].id, "mob.editor.review");
assert.deepEqual(operationProjection.instances, storedGraphDocument.instances);
assert.equal(operationProjection.deploySettings.command, "rkat mob run");
assert.equal(operationProjection.mobSettings.backendDefault, "session");

const importedHydrationId = controller.hydrateMobpackDocumentState({
  document: storedGraphDocument,
  validation: { ok: true },
}, {
  existingRows: [{ id: "f_stored_graph_import" }],
});
assert.equal(importedHydrationId.id, "f_stored_graph_import_2");
assert.equal(importedHydrationId.registryRow.id, "f_stored_graph_import_2");

const flowOnlyHydrated = controller.hydrateMobpackDocumentState({
  document: {
    name: "Flow Only Import",
    mob_id: "flow_only_import",
    members: storedGraphDocument.members,
    flow: storedGraphDocument.flow,
  },
}, {
  addToRegistry: false,
  openEditor: false,
});
assert.equal(flowOnlyHydrated.addToRegistry, false);
assert.equal(flowOnlyHydrated.openEditor, false);
assert.deepEqual(flowOnlyHydrated.graphProjection.instances, []);
assert.deepEqual(flowOnlyHydrated.graphProjection.edges, []);
assert.deepEqual(flowOnlyHydrated.graphProjection.frames, []);
assert.deepEqual(flowOnlyHydrated.validationRows, []);

const anonymousHydrated = controller.hydrateMobpackDocumentState({
  document: {
    members: storedGraphDocument.members,
    flow: { steps: [{ id: "step_1", type: "member", role: "reviewer", instruction: "Review." }] },
  },
}, {
  existingRows: [],
});
assert.equal(anonymousHydrated.id, "");
assert.equal(anonymousHydrated.registryRow, null);

const missingFlowHydrated = controller.hydrateMobpackDocumentState({
  document: { name: "No Flow Import", mob_id: "no_flow_import" },
}, {
  deployDefaults: testDeploySettings(),
  mobDefaults: controller.mobDefaultsFromSchema(TEST_SCHEMA),
  errorView: hydratedCatalogs.errorView,
});
assert.equal(missingFlowHydrated.ok, false);
assert.equal(missingFlowHydrated.error, "missing_editor_flow");
assert.equal(missingFlowHydrated.flow, null);
assert.equal(missingFlowHydrated.graphProjection, null);
assert.equal(missingFlowHydrated.addToRegistry, false);
assert.equal(missingFlowHydrated.openEditor, false);
assert.deepEqual(missingFlowHydrated.members, []);
assert.deepEqual(missingFlowHydrated.schemas, []);
assert.equal(missingFlowHydrated.deploySettings.maxTotalTokens, 64);
assert.equal(missingFlowHydrated.mobSettings.backendDefault, "session");
assert.equal(missingFlowHydrated.validationRows[0].kind, "crit");
assert.equal(missingFlowHydrated.validationRows[0].head, "Imported mobpack is missing a Meta editor flow");
assert.equal(missingFlowHydrated.validationRows[0].meta, "missing_editor_flow");

const appendedRows = controller.flowRegistryAppendRowPatch(registryRows, importedRow);
assert.equal(appendedRows.length, 3);
assert.equal(appendedRows[2], importedRow);

const replacedRows = controller.flowRegistryUpsertRowPatch(registryRows, { ...importedRow, id: "f_existing", name: "Replacement" });
assert.equal(replacedRows.length, 2);
assert.equal(replacedRows[0].name, "Replacement");
assert.equal(replacedRows[1], registryRows[1]);

const patchedDeploy = controller.deploySettingsPatch({
  surface: "cli",
  trustPolicy: "permissive",
  maxToolCalls: 2,
}, {
  trustPolicy: "strict",
  maxTotalTokens: 128,
  prompt: "Run the mob.",
});
assert.equal(patchedDeploy.surface, "cli");
assert.equal(patchedDeploy.trustPolicy, "strict");
assert.equal(patchedDeploy.maxToolCalls, 2);
assert.equal(patchedDeploy.maxTotalTokens, 128);
assert.equal(patchedDeploy.prompt, "Run the mob.");
const settingsContract = {
  deploy_settings: {
    command: "rkat mob run",
    surfaces: ["cli"],
    trust_policies: ["permissive"],
    realm_backends: ["jsonl"],
  },
  mob_definition: {
    profile_backends: ["session", "external"],
  },
};
const catalogedDeploy = controller.deploySettingsPatch({
  command: "rkat mob run",
  surface: "cli",
  trustPolicy: "permissive",
  realmBackend: "jsonl",
  model: "openai/gpt-5.5",
}, {
  command: "invalid deploy command",
  surface: "rpc",
  trustPolicy: "strict",
  realmBackend: "sqlite",
  model: "openai/ghost",
  prompt: "Still valid.",
}, {
  contract: settingsContract,
  modelCatalog: [{ id: "openai/gpt-5.5" }],
});
assert.equal(catalogedDeploy.command, "rkat mob run");
assert.equal(catalogedDeploy.surface, "cli");
assert.equal(catalogedDeploy.trustPolicy, "permissive");
assert.equal(catalogedDeploy.realmBackend, "jsonl");
assert.equal(catalogedDeploy.model, "openai/gpt-5.5");
assert.equal(catalogedDeploy.prompt, "Still valid.");
const validCatalogedDeploy = controller.deploySettingsPatch(catalogedDeploy, {
  model: "",
  realmBackend: "jsonl",
}, {
  contract: settingsContract,
  modelCatalog: [{ id: "openai/gpt-5.5" }],
});
assert.equal(validCatalogedDeploy.model, "");
assert.equal(validCatalogedDeploy.realmBackend, "jsonl");
assert.equal(
  controller.deploySettingsFieldPatch(catalogedDeploy, "model", "openai/ghost", {
    contract: settingsContract,
    modelCatalog: [{ id: "openai/gpt-5.5" }],
  }).model,
  "openai/gpt-5.5",
);
assert.equal(
  controller.deploySettingsFieldPatch(catalogedDeploy, "model", "", {
    contract: settingsContract,
    modelCatalog: [{ id: "openai/gpt-5.5" }],
  }).model,
  "",
);

const patchedMob = controller.mobSettingsPatch({
  orchestrator: "planner",
  roleWiring: [{ a: "planner", b: "coder" }],
}, {
  backendDefault: "external",
  roleWiring: [{ a: "reviewer", b: "planner" }, { a: "", b: "ignored" }],
  topology: { kind: "mesh" },
  arbitraryToml: { leaks: true },
});
assert.equal(patchedMob.orchestrator, "planner");
assert.equal(patchedMob.backendDefault, "external");
assert.deepEqual(patchedMob.roleWiring, [{ a: "reviewer", b: "planner" }]);
assert.equal(patchedMob.topology, undefined);
assert.equal(patchedMob.arbitraryToml, undefined);
const catalogedMob = controller.mobSettingsPatch({
  backendDefault: "session",
  orchestrator: "planner",
}, {
  backendDefault: "daemon",
  externalAddressBase: "http://127.0.0.1:9000",
}, {
  contract: settingsContract,
});
assert.equal(catalogedMob.backendDefault, "session");
assert.equal(catalogedMob.externalAddressBase, "http://127.0.0.1:9000");
assert.equal(controller.mobSettingsFieldPatch(catalogedMob, "backendDefault", "daemon", { contract: settingsContract }).backendDefault, "session");
assert.equal(controller.mobSettingsFieldPatch(catalogedMob, "backendDefault", "", { contract: settingsContract }).backendDefault, "");
assert.equal(controller.mobSettingsPatch(catalogedMob, { backendDefault: "" }, { contract: settingsContract }).backendDefault, "");
assert.equal(controller.mobSettingsPatch(catalogedMob, { backendDefault: "sidecar" }, {
  contract: { mob_definition: { profile_backends: ["session", "sidecar"] } },
}).backendDefault, "sidecar");

const roleRules = [{ a: "planner", b: "coder" }, { a: "coder", b: "reviewer" }];
assert.deepEqual(
  controller.mobRoleWiringEditorState(
    [{ a: " planner ", b: "coder" }, { a: "", b: "ignored" }],
    [{ value: "planner", label: "Planner" }, { value: "coder", label: "Coder" }],
    TEST_SETTINGS_VIEW,
  ),
  {
    label: "Role wiring",
    countLabel: "1",
    addLabel: "+ rule",
    addDisabled: false,
    options: [{ value: "planner", label: "Planner" }, { value: "coder", label: "Coder" }],
    wiring: [{ a: "planner", b: "coder" }],
  },
);
assert.equal(controller.mobRoleWiringEditorState([], [], TEST_SETTINGS_VIEW).addDisabled, true);
const roleOptions = [{ value: "planner" }, { value: "coder" }, { value: "reviewer" }];
assert.deepEqual(
  controller.mobRoleWiringUpdatePatch(roleRules, 1, { b: "planner" }, roleOptions),
  [{ a: "planner", b: "coder" }, { a: "coder", b: "planner" }],
);
assert.deepEqual(
  controller.mobRoleWiringSourcePatch(roleRules, 1, "reviewer", roleOptions),
  [{ a: "planner", b: "coder" }, { a: "reviewer", b: "reviewer" }],
);
assert.deepEqual(
  controller.mobRoleWiringTargetPatch(roleRules, 0, "reviewer", roleOptions),
  [{ a: "planner", b: "reviewer" }, { a: "coder", b: "reviewer" }],
);
assert.deepEqual(
  controller.mobRoleWiringUpdatePatch(roleRules, 1, { b: "ghost_profile" }, roleOptions),
  [{ a: "planner", b: "coder" }],
);
assert.deepEqual(
  controller.mobRoleWiringDeletePatch(roleRules, 0),
  [{ a: "coder", b: "reviewer" }],
);
assert.deepEqual(
  controller.mobRoleWiringAddPatch(roleRules, [{ value: "reviewer" }, { value: "planner" }]),
  [{ a: "planner", b: "coder" }, { a: "coder", b: "reviewer" }, { a: "reviewer", b: "planner" }],
);

assert.deepEqual(controller.advancedMobSettingsEditorState({ limits: { max_members: 3 } }, TEST_SETTINGS_VIEW), {
  label: "Advanced",
  text: '{\n  "limits": {\n    "max_members": 3\n  }\n}',
});
const advancedOk = controller.advancedMobSettingsDraftPatch('{"limits":{"max_members":3},"spawnPolicy":{"mode":"manual"}}', TEST_SETTINGS_VIEW);
assert.equal(advancedOk.ok, true);
assert.deepEqual(advancedOk.value, {
  topology: null,
  supervisor: null,
  limits: { max_members: 3 },
  spawnPolicy: { mode: "manual" },
  eventRouter: null,
});
assert.equal(controller.advancedMobSettingsDraftPatch("[]", TEST_SETTINGS_VIEW).error, "object required");
assert.equal(controller.advancedMobSettingsDraftPatch("{", TEST_SETTINGS_VIEW).ok, false);

const enumField = { type: "string", enumValues: [] };
assert.deepEqual(controller.schemaLikeFieldTypePatch(enumField, "enum", graphShapeContract), { type: "enum", enumValues: ["value"] });
assert.deepEqual(controller.schemaLikeFieldTypePatch({ type: "enum", enumValues: ["green"] }, "string", graphShapeContract), { type: "string", enumValues: [] });
assert.deepEqual(controller.schemaLikeFieldTypePatch({ type: "string", enumValues: [] }, "object", graphShapeContract), {});
const schemaTypeControlState = controller.schemaLikeFieldTypeControlState({}, graphShapeContract);
assert.equal(schemaTypeControlState.type, "string");
assert.equal(schemaTypeControlState.selectedType.label, "string");
assert.deepEqual(schemaTypeControlState.typeOptions.map((option) => [option.value, option.label, option.disabled]), [
  ["string", "string", false],
  ["enum", "enum — fixed choices", false],
]);
const unsupportedSchemaTypeControlState = controller.schemaLikeFieldTypeControlState({ type: "object" }, graphShapeContract);
assert.equal(unsupportedSchemaTypeControlState.type, "object");
assert.equal(unsupportedSchemaTypeControlState.selectedType.disabled, true);
assert.match(unsupportedSchemaTypeControlState.selectedType.reason, /mob_definition\.editor_schema_field_types/);
const schemaFieldRowState = controller.schemaFieldRowControlState(
  { type: "enum", enumValues: ["green"] },
  graphShapeContract,
  hydratedCatalogs.schemaView,
);
assert.equal(schemaFieldRowState.namePlaceholder, "field_name");
assert.equal(schemaFieldRowState.descriptionPlaceholder, "—");
assert.equal(schemaFieldRowState.removeTitle, "Remove field");
assert.equal(schemaFieldRowState.enumLabel, "VALUES");
assert.equal(schemaFieldRowState.enumAddLabel, "+ value");
assert.equal(schemaFieldRowState.enumAddValue, "value");
assert.deepEqual(schemaFieldRowState.enumValues, ["green"]);
assert.equal(schemaFieldRowState.typeState.type, "enum");
const inputParamFieldState = controller.inputParamFieldControlState(
  { type: "enum", enumValues: ["green"] },
  graphShapeContract,
  hydratedCatalogs.basicView,
);
assert.equal(inputParamFieldState.namePlaceholder, "param_name");
assert.equal(inputParamFieldState.descriptionPlaceholder, "—");
assert.equal(inputParamFieldState.removeTitle, "Remove param");
assert.equal(inputParamFieldState.enumLabel, "VALUES");
assert.equal(inputParamFieldState.enumAddLabel, "+ value");
assert.equal(inputParamFieldState.enumAddValue, "value");
assert.deepEqual(inputParamFieldState.enumValues, ["green"]);
assert.equal(inputParamFieldState.typeState.type, "enum");
assert.deepEqual(controller.schemaLikeFieldRequiredPatch(0), { required: false });
assert.deepEqual(controller.schemaLikeFieldRequiredPatch("yes"), { required: true });
assert.deepEqual(controller.schemaLikeFieldDescriptionPatch(null), { description: "" });
assert.deepEqual(controller.schemaLikeFieldDescriptionPatch("Review result"), { description: "Review result" });
assert.deepEqual(controller.enumValueDraftPatch({ enumValues: ["green"] }, 0, ""), { enumValues: [""] });
assert.deepEqual(controller.enumValueCommitPatch({ enumValues: ["green", "red"] }, 1, "green"), { enumValues: ["green", "green_2"] });
assert.deepEqual(controller.enumValueDeletePatch({ enumValues: ["green", "red"] }, 0), { enumValues: ["red"] });
assert.deepEqual(controller.enumValueAddPatch({ enumValues: ["value"] }, "value"), { enumValues: ["value", "value_2"] });

const projectionPlan = controller.authoringProjectionApplyPlan({
  flow: { id: "main", steps: [{ id: "input", type: "input" }] },
  members: [{ id: "reviewer", name: "Reviewer" }],
  instances: [{ id: "n_reviewer", memberId: "reviewer", col: 1, row: 0 }],
  edges: [],
  frames: [{ id: "frame_review", kind: "repeat", colStart: 1, colEnd: 1, rowStart: 0, rowEnd: 0 }],
  deploySettings: { surface: "cli" },
  mobSettings: { backendDefault: "session" },
}, {
  flow: { id: "main", steps: [] },
  studio: {
    members: [{ id: "reviewer", name: "Old reviewer" }],
    instances: [{ id: "n_reviewer", memberId: "reviewer", col: 0, row: 0 }],
    edges: [],
    frames: [],
  },
  deploySettings: { surface: "web" },
  mobSettings: { backendDefault: "rpc" },
  contract: graphShapeContract,
});
assert.equal(projectionPlan.ok, true);
assert.equal(projectionPlan.flow.changed, true);
assert.equal(projectionPlan.members.changed, true);
assert.equal(projectionPlan.graph.changed, true);
assert.equal(projectionPlan.frames.changed, true);
assert.equal(projectionPlan.deploySettings.changed, true);
assert.equal(projectionPlan.mobSettings.changed, true);
assert.equal(
  projectionPlan.graph.signature,
  controller.graphStructureSignature(projectionPlan.graph.instances, projectionPlan.graph.edges, {
    members: projectionPlan.members.value,
    contract: graphShapeContract,
  }),
);

const unchangedProjectionPlan = controller.authoringProjectionApplyPlan({
  flow: projectionPlan.flow.value,
  members: projectionPlan.members.value,
  instances: projectionPlan.graph.instances,
  edges: projectionPlan.graph.edges,
  frames: projectionPlan.frames.value,
  deploySettings: projectionPlan.deploySettings.value,
  mobSettings: projectionPlan.mobSettings.value,
}, {
  flow: projectionPlan.flow.value,
  studio: {
    members: projectionPlan.members.value,
    instances: projectionPlan.graph.instances,
    edges: projectionPlan.graph.edges,
    frames: projectionPlan.frames.value,
  },
  deploySettings: projectionPlan.deploySettings.value,
  mobSettings: projectionPlan.mobSettings.value,
  contract: graphShapeContract,
});
assert.equal(unchangedProjectionPlan.flow.changed, false);
assert.equal(unchangedProjectionPlan.members.changed, false);
assert.equal(unchangedProjectionPlan.graph.changed, false);
assert.equal(unchangedProjectionPlan.frames.changed, false);
assert.equal(unchangedProjectionPlan.deploySettings.changed, false);
assert.equal(unchangedProjectionPlan.mobSettings.changed, false);

const studioStateA = {
  members: [{ id: "m1" }],
  instances: [{ id: "i1" }],
  edges: [],
  frames: [],
  schemas: [],
  skillRealms: [{ id: "realm" }],
};
const studioStateB = {
  members: [{ id: "m2" }],
  instances: [{ id: "i2" }],
  edges: [{ id: "e1" }],
  frames: [{ id: "f1" }],
  schemas: [{ id: "s1" }],
  skillRealms: [],
};
const snapPatch = controller.studioHistorySnapshotPatch({
  history: Array.from({ length: 31 }, (_, index) => ({ members: [{ id: `m${index}` }] })),
  future: [studioStateB],
  state: studioStateA,
});
assert.equal(snapPatch.history.length, 31);
assert.equal(snapPatch.history[snapPatch.history.length - 1].members[0].id, "m1");
assert.deepEqual(snapPatch.future, []);

const undoPatch = controller.studioUndoPatch({ history: [studioStateA], future: [], state: studioStateB });
assert.deepEqual(undoPatch.state.members, [{ id: "m1" }]);
assert.equal(undoPatch.history.length, 0);
assert.deepEqual(undoPatch.future[0].members, [{ id: "m2" }]);
assert.equal(controller.studioUndoPatch({ history: [], future: [], state: studioStateB }), null);

const redoPatch = controller.studioRedoPatch({ history: [], future: [studioStateB], state: studioStateA });
assert.deepEqual(redoPatch.state.members, [{ id: "m2" }]);
assert.deepEqual(redoPatch.history[0].members, [{ id: "m1" }]);
assert.equal(redoPatch.future.length, 0);
assert.equal(controller.studioRedoPatch({ history: [], future: [], state: studioStateA }), null);

async function exerciseAuthoringOperationRunner() {
  const originalFetch = global.fetch;
  const rpcCalls = [];
  const projections = [];
  let markedDrafts = 0;
  let projectionSyncs = 0;
  let inFlight = 0;
  let maxInFlight = 0;

  global.fetch = async (url, options) => {
    inFlight += 1;
    maxInFlight = Math.max(maxInFlight, inFlight);
    await Promise.resolve();
    const request = JSON.parse(options.body);
    rpcCalls.push({ url, request });
    const operation = request.params.operation;
    const document = {
      ...storedGraphDocument,
      deploy: {
        ...(storedGraphDocument.deploy || {}),
        prompt: operation.value,
      },
    };
    inFlight -= 1;
    return {
      ok: true,
      json: async () => ({
        jsonrpc: "2.0",
        id: request.id,
        result: {
          source: "meta/mobpacks/apply_operation",
          operation: operation.type,
          document,
          selection: operation.selection || null,
          validation: { ok: true },
        },
      }),
    };
  };

  try {
    controller.configure({ rpcUrl: "/runner-rpc" });
    const runner = controller.createAuthoringOperationRunner({
      getAuthoringOperations: () => ({
        update_deploy_settings: { type: "update_deploy_settings" },
      }),
      getCurrentDocument: () => storedGraphDocument,
      getDraftGuard: () => ({ expected_revision: 7, expected_etag: "draft-etag" }),
      getCatalogSnapshot: () => ({ id: "catalog-snapshot-1" }),
      getCurrentRevision: () => 42,
      isRevisionCurrent: (revision) => revision === 42,
      getProjectionDefaults: () => ({
        deployDefaults: testDeploySettings(),
        mobDefaults: controller.mobDefaultsFromSchema(TEST_SCHEMA),
      }),
      beginProjectionSync: () => {
        projectionSyncs += 1;
      },
      applyProjection: (projection) => {
        projections.push(projection);
      },
      markDraft: () => {
        markedDrafts += 1;
      },
    });

    const [firstResult, secondResult] = await Promise.all([
      runner({ intent: "settings.updateDeployField", field: "prompt", value: "first prompt" }),
      runner({ intent: "settings.updateDeployField", field: "prompt", value: "second prompt" }),
    ]);

    assert.equal(firstResult.operation, "update_deploy_settings");
    assert.equal(secondResult.document.deploy.prompt, "second prompt");
    assert.equal(maxInFlight, 1);
    assert.equal(rpcCalls.length, 2);
    assert.equal(rpcCalls[0].url, "/runner-rpc");
    assert.equal(rpcCalls[0].request.method, "meta/mobpacks/apply_operation");
    assert.equal(rpcCalls[0].request.params.operation.type, "update_deploy_settings");
    assert.equal(rpcCalls[0].request.params.operation.field, "prompt");
    assert.equal(rpcCalls[0].request.params.expected_catalog_snapshot_id, "catalog-snapshot-1");
    assert.equal(rpcCalls[0].request.params.expected_revision, 7);
    assert.equal(rpcCalls[0].request.params.expected_etag, "draft-etag");
    assert.equal(projections.length, 2);
    assert.equal(projections[1].deploySettings.prompt, "second prompt");
    assert.equal(projectionSyncs, 2);
    assert.equal(markedDrafts, 2);

    const staleRunner = controller.createAuthoringOperationRunner({
      getAuthoringOperations: () => ({
        update_deploy_settings: { type: "update_deploy_settings" },
      }),
      getCurrentDocument: () => storedGraphDocument,
      getCatalogSnapshot: () => "catalog-snapshot-1",
      getCurrentRevision: () => 100,
      isRevisionCurrent: () => false,
      getStaleError: () => "stale authoring revision",
      applyProjection: (projection) => {
        projections.push(projection);
      },
      markDraft: () => {
        markedDrafts += 1;
      },
    });
    const staleResult = await staleRunner({ intent: "settings.updateDeployField", field: "prompt", value: "stale prompt" });
    assert.deepEqual(staleResult, { ok: false, error: "stale authoring revision" });
    assert.equal(projections.length, 2);
    assert.equal(markedDrafts, 2);

    let queuedRevision = 1;
    const queuedRpcCalls = [];
    let releaseFirstQueuedFetch = null;
    let firstQueuedFetchStarted = null;
    const firstQueuedFetchStartedPromise = new Promise((resolve) => {
      firstQueuedFetchStarted = resolve;
    });
    global.fetch = async (url, options) => {
      const request = JSON.parse(options.body);
      queuedRpcCalls.push({ url, request });
      if (request.params.operation.value === "first stale prompt") {
        firstQueuedFetchStarted();
        await new Promise((resolve) => {
          releaseFirstQueuedFetch = resolve;
        });
      }
      const document = {
        ...storedGraphDocument,
        deploy: {
          ...(storedGraphDocument.deploy || {}),
          prompt: request.params.operation.value,
        },
      };
      return {
        ok: true,
        json: async () => ({
          jsonrpc: "2.0",
          id: request.id,
          result: {
            source: "meta/mobpacks/apply_operation",
            operation: request.params.operation.type,
            document,
            selection: null,
            validation: { ok: true },
          },
        }),
      };
    };
    const queuedRunner = controller.createAuthoringOperationRunner({
      getAuthoringOperations: () => ({
        update_deploy_settings: { type: "update_deploy_settings" },
      }),
      getCurrentDocument: () => storedGraphDocument,
      getCurrentRevision: () => queuedRevision,
      isRevisionCurrent: (revision) => revision === queuedRevision,
      getStaleError: () => "stale authoring revision",
      getProjectionDefaults: () => ({
        deployDefaults: testDeploySettings(),
        mobDefaults: controller.mobDefaultsFromSchema(TEST_SCHEMA),
      }),
      applyProjection: (projection) => {
        projections.push(projection);
      },
      markDraft: () => {
        markedDrafts += 1;
      },
    });
    const firstQueuedResultPromise = queuedRunner({ intent: "settings.updateDeployField", field: "prompt", value: "first stale prompt" });
    await firstQueuedFetchStartedPromise;
    const secondQueuedResultPromise = queuedRunner({ intent: "settings.updateDeployField", field: "prompt", value: "second stale prompt" });
    queuedRevision = 2;
    releaseFirstQueuedFetch();
    assert.deepEqual(await firstQueuedResultPromise, { ok: false, error: "stale authoring revision" });
    assert.deepEqual(await secondQueuedResultPromise, { ok: false, error: "stale authoring revision" });
    assert.equal(queuedRpcCalls.length, 1);
    assert.equal(queuedRpcCalls[0].request.params.operation.value, "first stale prompt");
    assert.equal(projections.length, 2);
    assert.equal(markedDrafts, 2);
  } finally {
    global.fetch = originalFetch;
    controller.configure({ rpcUrl: "/flow-editor/rpc" });
  }
}

// Read-only renders (source preview, validation, deploy plan) carry the
// optimistic draft store guard of the row the UI last saw. When our own
// autosave has bumped the store but its response is not applied yet, the
// server refuses the render as a draft revision conflict; the authoring
// runner already retries that exact race once without the guard, and #398 was
// the source render lacking the same retry (the Graph-mode inline source panel
// stayed empty behind a "Source render failed" sheet). This pins the retry
// contract: fires only for a guard conflict AND only when a guard was sent,
// re-submits the same document with just the guard keys dropped, and never
// touches a real deploy.
async function exerciseReadOnlyRenderDraftGuardRetry() {
  const originalFetch = global.fetch;
  const document = { ...storedGraphDocument };
  const guard = { id: "f_race_mob", expected_revision: 1, expected_etag: "W/\"f_race_mob:1\"" };
  const conflictMessage = (method) => `${method} draft revision conflict for f_race_mob: expected 1, found 2`;
  let calls = [];
  const conflictOnceThenResult = (result) => {
    let refused = false;
    return async (url, options) => {
      const request = JSON.parse(options.body);
      calls.push(request);
      const hasGuard = request.params.expected_revision !== undefined || request.params.expected_etag !== undefined;
      if (hasGuard && !refused) {
        refused = true;
        return { ok: true, json: async () => ({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: conflictMessage(request.method) } }) };
      }
      return { ok: true, json: async () => ({ jsonrpc: "2.0", id: request.id, result }) };
    };
  };
  try {
    controller.configure({ rpcUrl: "/flow-editor/rpc" });

    // source: refused once with the guard, retried once without it, same document.
    calls = [];
    global.fetch = conflictOnceThenResult({ source: "meta/mobpacks/source", source_files: [] });
    assert.deepEqual(await controller.sourceDocument(document, guard), { source: "meta/mobpacks/source", source_files: [] });
    assert.equal(calls.length, 2, "one refusal, one retry");
    assert.equal(calls[0].method, "meta/mobpacks/source");
    assert.equal(calls[0].params.expected_revision, 1);
    assert.equal(calls[0].params.expected_etag, guard.expected_etag);
    assert.equal(calls[1].method, "meta/mobpacks/source");
    assert.equal(calls[1].params.expected_revision, undefined, "retry drops expected_revision");
    assert.equal(calls[1].params.expected_etag, undefined, "retry drops expected_etag");
    assert.equal(calls[1].params.id, "f_race_mob", "retry keeps the non-guard request options");
    assert.deepEqual(calls[1].params.document, document, "retry re-submits the same document");

    // validate and the deploy PLAN get the same retry.
    calls = [];
    global.fetch = conflictOnceThenResult({ ok: true, diagnostics: [] });
    assert.deepEqual(await controller.validateDocument(document, guard), { ok: true, diagnostics: [] });
    assert.equal(calls.length, 2);
    assert.equal(calls[1].params.rkat_validate, true, "validate keeps its own request fields on retry");
    assert.equal(calls[1].params.expected_revision, undefined);
    calls = [];
    global.fetch = conflictOnceThenResult({ plan: [] });
    assert.deepEqual(await controller.deployDocument(document, { execute: false, ...guard }), { plan: [] });
    assert.equal(calls.length, 2);
    assert.equal(calls[1].params.execute, false, "plan retry stays a plan");
    assert.equal(calls[1].params.expected_revision, undefined);

    // A real deploy is a write: the guard conflict surfaces, no retry.
    calls = [];
    global.fetch = conflictOnceThenResult({ deployed: true });
    await assert.rejects(
      () => controller.deployDocument(document, { execute: true, ...guard }),
      (error) => controller.isDraftGuardConflictError(error),
    );
    assert.equal(calls.length, 1, "deploy execute:true is never retried");

    // A conflict without any guard in the request is not ours to absorb.
    calls = [];
    global.fetch = async (url, options) => {
      const request = JSON.parse(options.body);
      calls.push(request);
      return { ok: true, json: async () => ({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: conflictMessage(request.method) } }) };
    };
    await assert.rejects(() => controller.sourceDocument(document, { id: "f_race_mob" }), (error) => controller.isDraftGuardConflictError(error));
    assert.equal(calls.length, 1, "no guard sent means nothing to drop, so no retry");

    // Any other refusal propagates unchanged after a single attempt.
    calls = [];
    global.fetch = async (url, options) => {
      const request = JSON.parse(options.body);
      calls.push(request);
      return { ok: true, json: async () => ({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: "cannot render source for invalid mobpack document" } }) };
    };
    await assert.rejects(() => controller.sourceDocument(document, guard), /cannot render source for invalid mobpack document/);
    assert.equal(calls.length, 1);

    // A second refusal after the unguarded retry surfaces as the conflict it is.
    calls = [];
    global.fetch = async (url, options) => {
      const request = JSON.parse(options.body);
      calls.push(request);
      return { ok: true, json: async () => ({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: conflictMessage(request.method) } }) };
    };
    await assert.rejects(() => controller.sourceDocument(document, guard), (error) => controller.isDraftGuardConflictError(error));
    assert.equal(calls.length, 2, "exactly one retry, never a loop");
  } finally {
    global.fetch = originalFetch;
    controller.configure({ rpcUrl: "/flow-editor/rpc" });
  }
}

exerciseAuthoringOperationRunner()
  .then(exerciseReadOnlyRenderDraftGuardRetry)
  .then(() => {
    console.log("controller projection metadata ok");
  })
  .catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
