// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the contract-option functions move byte-verbatim as plain JS,
// and their destructured `= {}` parameter defaults (tweaksControlState,
// graphAddNodeMenuState, basicStepPickerState) and heterogeneous option-row
// literals raise TS2339 under .ts semantics. Source-contract pins this
// exact text, so suppression must live at file level, not in the moved
// bodies. Resolution/linkage stays guarded behaviorally: the projection
// suite and export-keys test load the bundle and exercise these functions,
// so a missed import or re-export still fails the gate as a ReferenceError.
//
// Contract option helpers for the Flow Editor controller plane. Moved
// verbatim from the controller.js contract-options range: runtime-mode and
// deploy-surface compatibility, simpleContractOptions and the option-builder
// family, tweaksControlState, graphAddNodeMenuState, basicStepPickerState,
// and the contractDefault* chain. contractStringValues was seeded here in
// S5 (needed by schema/field-edit.ts) and returns to its original
// intra-cluster position with the rest of the range in S6.
//
// SCC note: contract/options.ts and flow/launch-modes.ts form a
// runtime-only import cycle (no module-init cross-calls), co-moved in S6
// per the extraction design. The straggler edges that went through the
// lazy _residue-bridge (basicEditorViewState, graphCanvasViewState,
// graphCellXY) became relative imports when the editors modules landed in
// S11 — runtime-only cycles with the editors' contract/options imports,
// no module-init cross-calls.
import { profileName } from "../domain/tool-skill-access";
import { basicEditorViewState } from "../editors/basic-editor";
import { graphCanvasViewState, graphCellXY } from "../editors/graph-editor";
import {
  canonicalLaunchModeKind,
  launchOptionLabel,
  launchUnsupportedReason,
  normalizeForkContext,
} from "../flow/launch-modes";
import {
  launchViewForState,
  roleAccentStyle,
  settingsViewForState,
  viewStringMapFromSchema,
} from "../views/view-config";

export function runtimeModeOptions(contract, deploySettings, currentMode) {
  const modes = Array.isArray(contract?.mob_definition?.runtime_modes) && contract.mob_definition.runtime_modes.length
    ? contract.mob_definition.runtime_modes.map(String)
    : [];
  const current = String(currentMode || "");
  if (current && !modes.includes(current)) modes.push(current);
  const surface = String(deploySettings?.surface || contract?.deploy_settings?.defaults?.surface || "");
  const labels = viewStringMapFromSchema(contract?.mob_definition?.runtime_mode_labels);
  return modes.map((mode) => {
    const surfaceBlocked = !runtimeModeDeploySurfaceAllowed(contract, surface, mode);
    return {
      value: mode,
      label: labels[mode] || `${mode}`,
      disabled: surfaceBlocked,
      reason: surfaceBlocked ? runtimeModeDeploySurfaceReason(contract, surface, mode) : "",
    };
  });
}

export function deployRuntimeCompatibility(contract, surface) {
  const compatibility = contract?.mob_definition?.deploy_runtime_mode_compatibility;
  if (!compatibility || typeof compatibility !== "object") return null;
  const surfaceKey = String(surface || contract?.deploy_settings?.defaults?.surface || "").trim();
  const surfaceContract = compatibility[surfaceKey];
  return surfaceContract && typeof surfaceContract === "object" ? surfaceContract : null;
}

export function deploySurfaceRuntimeModes(contract, surface) {
  return contractStringValues(deployRuntimeCompatibility(contract, surface)?.allowed);
}

export function runtimeModeDeploySurfaceAllowed(contract, surface, mode) {
  const value = String(mode || "").trim();
  if (!value) return true;
  const allowed = deploySurfaceRuntimeModes(contract, surface);
  return allowed.length ? allowed.includes(value) : true;
}

export function runtimeModeDeploySurfaceReason(contract, surface, mode) {
  const value = String(mode || "").trim();
  const blocked = deployRuntimeCompatibility(contract, surface)?.blocked;
  const reason = blocked && typeof blocked === "object" ? blocked[value] : "";
  return String(reason || "Unsupported by this Meta deploy surface.");
}

export function firstDeploySurfaceRuntimeMode(contract, surface) {
  return deploySurfaceRuntimeModes(contract, surface)[0] || "";
}

export function simpleContractOptions(values, currentValue, labels, contractName, display = {}) {
  const contractValues = Array.isArray(values)
    ? values.map((value) => String(value || "").trim()).filter(Boolean)
    : [];
  const options = contractValues.length ? [...contractValues] : [];
  const current = String(currentValue || "").trim();
  if (current && !options.includes(current)) options.push(current);
  const unsupportedLabelSeparator = String(display.unsupportedLabelSeparator || "");
  const unsupportedReasonPrefix = String(display.unsupportedReasonPrefix || "Unsupported ");
  const unsupportedReasonSuffix = String(display.unsupportedReasonSuffix || "");
  return options.map((value) => {
    const known = contractValues.includes(value);
    const label = labels?.[value] || (known || !unsupportedLabelSeparator ? value : `${value}${unsupportedLabelSeparator}${contractName}`);
    return {
      value,
      label,
      disabled: !known,
      reason: known ? "" : `${unsupportedReasonPrefix}${contractName}${unsupportedReasonSuffix}`,
    };
  });
}

export function profileBindingRestriction(contract, binding) {
  const restrictions = contract?.mob_definition?.profile_binding_restrictions;
  const value = restrictions && typeof restrictions === "object" ? restrictions[binding] : null;
  return value && typeof value === "object" ? value : {};
}

export function deploySurfaceOptions(contract, currentSurface, settingsView = null) {
  const view = settingsViewForState(settingsView);
  return simpleContractOptions(
    contract?.deploy_settings?.surfaces,
    currentSurface || "",
    view.deploySurfaceLabels,
    view.deploySurfaceContractLabel,
    view
  );
}

export function trustPolicyOptions(contract, currentPolicy, settingsView = null) {
  const view = settingsViewForState(settingsView);
  return simpleContractOptions(
    contract?.deploy_settings?.trust_policies,
    currentPolicy || "",
    view.trustPolicyLabels,
    view.trustPolicyContractLabel,
    view
  );
}

export function realmBackendOptions(contract, currentBackend, settingsView = null) {
  const view = settingsViewForState(settingsView);
  return simpleContractOptions(
    contract?.deploy_settings?.realm_backends,
    currentBackend || "",
    view.realmBackendLabels,
    view.realmBackendContractLabel,
    view
  );
}

export function profileBackendOptions(contract, currentBackend, includeDefault, defaultLabel = "") {
  const options = simpleContractOptions(
    contract?.mob_definition?.profile_backends,
    currentBackend || "",
    { session: "session", external: "external" },
    "mob_definition.profile_backends"
  );
  if (!includeDefault) return options;
  return [{ value: "", label: String(defaultLabel || ""), disabled: false, reason: "" }, ...options.filter(option => option.value)];
}

export function profileBindingOptions(contract, currentBinding) {
  return simpleContractOptions(
    contract?.mob_definition?.profile_binding,
    currentBinding || "",
    {
      inline: "inline — define profile in this mobpack",
      realm_profile: "realm_profile",
    },
    "mob_definition.profile_binding"
  ).map((option) => {
    const restriction = profileBindingRestriction(contract, option.value);
    const deployable = restriction.deployable;
    return {
      ...option,
      label: String(restriction.label || option.label || option.value),
      disabled: option.disabled || deployable === false,
      reason: String(restriction.reason || option.reason || ""),
    };
  });
}

export function mobBackendDefaultOptions(contract, currentBackend) {
  return simpleContractOptions(
    contract?.mob_definition?.profile_backends,
    currentBackend || "",
    { session: "session", external: "external" },
    "mob_definition.mob_settings.backendDefault"
  );
}

export function tweaksControlState({
  deploySettings = {},
  mobSettings = {},
  members = [],
  modelCatalog = [],
  contract = null,
  settingsView = null,
} = {}) {
  const view = settingsViewForState(settingsView);
  const profileOptions = [
    { value: "", label: view.profileNoneLabel },
    ...(Array.isArray(members) ? members : []).map((member) => {
      const profile = profileName(member);
      return { value: profile, label: profile };
    }),
  ];
  const modelOptions = [
    { value: "", label: view.modelDefaultLabel },
    ...(Array.isArray(modelCatalog) ? modelCatalog : []).map((model) => ({
      value: model.id,
      label: `${model.label || model.id}${view.optionSeparator}${model.vendor || view.modelVendorFallback}`,
    })),
  ];
  return {
    canvasTitle: view.canvasTitle,
    edgeStyleLabel: view.edgeStyleLabel,
    edgeStyleOptions: view.edgeStyleOptions,
    densityLabel: view.densityLabel,
    densityOptions: view.densityOptions,
    themeTitle: view.themeTitle,
    themeModeLabel: view.themeModeLabel,
    themeModeOptions: view.themeModeOptions,
    mobTitle: view.mobTitle,
    orchestratorLabel: view.orchestratorLabel,
    autoWireLabel: view.autoWireLabel,
    autoWireOptions: view.autoWireOptions,
    roleWiringLabel: view.roleWiringLabel,
    roleWiringAddLabel: view.roleWiringAddLabel,
    defaultBackendLabel: view.defaultBackendLabel,
    externalBaseLabel: view.externalBaseLabel,
    externalBasePlaceholder: view.externalBasePlaceholder,
    advancedLabel: view.advancedLabel,
    advancedObjectRequiredError: view.advancedObjectRequiredError,
    advancedInvalidJsonError: view.advancedInvalidJsonError,
    deployTitle: view.deployTitle,
    surfaceLabel: view.surfaceLabel,
    trustLabel: view.trustLabel,
    modelLabel: view.modelLabel,
    durationLabel: view.durationLabel,
    durationPlaceholder: view.durationPlaceholder,
    toolCallsLabel: view.toolCallsLabel,
    toolCallsMin: view.toolCallsMin,
    toolCallsMax: view.toolCallsMax,
    tokensLabel: view.tokensLabel,
    tokensMin: view.tokensMin,
    tokensMax: view.tokensMax,
    realmLabel: view.realmLabel,
    realmOptions: view.realmOptions,
    realmIdLabel: view.realmIdLabel,
    realmIdPlaceholder: view.realmIdPlaceholder,
    backendLabel: view.backendLabel,
    promptLabel: view.promptLabel,
    promptPlaceholder: view.promptPlaceholder,
    commandLabel: view.commandLabel,
    commandFallback: view.commandFallback,
    inspectorTitle: view.inspectorTitle,
    inspectorLayoutLabel: view.inspectorLayoutLabel,
    inspectorLayoutOptions: view.inspectorLayoutOptions,
    profileOptions,
    profileChoices: profileOptions.filter((option) => option.value),
    mobBackendOptions: mobBackendDefaultOptions(contract, mobSettings.backendDefault || ""),
    surfaceOptions: deploySurfaceOptions(contract, deploySettings.surface || "", view),
    trustOptions: trustPolicyOptions(contract, deploySettings.trustPolicy || "", view),
    realmBackendOptions: realmBackendOptions(contract, deploySettings.realmBackend || "", view),
    modelOptions,
  };
}

export function schemaFieldTypeOptions(contract, currentType) {
  return simpleContractOptions(
    contract?.mob_definition?.editor_schema_field_types,
    currentType || contractDefaultValue(contract, "schema_field_type"),
    {
      string: "string",
      "string[]": "string[] — list",
      number: "number",
      float: "float",
      int: "int",
      integer: "integer",
      boolean: "boolean",
      bool: "bool",
      enum: "enum — fixed choices",
      bytes: "bytes — binary blob",
      object: "object — nested",
    },
    "mob_definition.editor_schema_field_types"
  );
}

export function conditionOperatorOptions(contract, currentOperator) {
  return simpleContractOptions(
    contract?.mob_definition?.condition_operators,
    currentOperator || contractDefaultValue(contract, "condition_operator"),
    { "==": "==", ">": ">", "<": "<" },
    "mob_definition.condition_operators"
  );
}

export function forkContextOptions(contract, currentContext, launchView = null) {
  const view = launchViewForState(launchView);
  const contractValues = Array.isArray(contract?.mob_definition?.fork_contexts)
    ? contract.mob_definition.fork_contexts.map((value) => normalizeForkContext(value)).filter(Boolean)
    : [];
  const options = contractValues.length ? [...contractValues] : [];
  const currentSource = currentContext || contractDefaultValue(contract, "fork_context");
  const current = currentSource ? normalizeForkContext(currentSource) : "";
  if (current && !options.includes(current)) options.push(current);
  return options.map((value) => {
    const supported = contractValues.includes(value);
    return {
      value,
      label: launchOptionLabel(view.forkContextLabels, value, view, view.forkContextsContractLabel),
      disabled: !supported,
      reason: supported ? "" : launchUnsupportedReason(view, view.forkContextsContractLabel),
    };
  });
}

export function graphGateKindOptions(contract, currentKind, graphView = null) {
  const view = graphCanvasViewState(graphView);
  return simpleContractOptions(
    contract?.mob_definition?.graph_gate_kinds,
    currentKind || contractDefaultValue(contract, "graph_gate_kind"),
    view.gateKindLabels,
    "mob_definition.graph_gate_kinds"
  );
}

export function graphTerminalKindOptions(contract, currentKind, graphView = null) {
  const view = graphCanvasViewState(graphView);
  return simpleContractOptions(
    contract?.mob_definition?.graph_terminal_kinds,
    currentKind || contractDefaultValue(contract, "graph_terminal_kind"),
    view.terminalKindLabels,
    "mob_definition.graph_terminal_kinds"
  );
}

export function graphFrameKindOptions(contract, currentKind, graphView = null) {
  const view = graphCanvasViewState(graphView);
  return simpleContractOptions(
    contract?.mob_definition?.graph_frame_kinds,
    currentKind || contractDefaultValue(contract, "graph_frame_kind"),
    view.frameKindLabels,
    "mob_definition.graph_frame_kinds"
  );
}

export function graphEdgeKindOptions(contract, currentKind, graphView = null) {
  const view = graphCanvasViewState(graphView);
  return simpleContractOptions(
    contract?.mob_definition?.graph_edge_kinds,
    currentKind || contractDefaultValue(contract, "graph_edge_kind"),
    view.edgeKindLabels,
    "mob_definition.graph_edge_kinds"
  );
}

export function repeatIterationInputOptions(contract, currentMode) {
  return simpleContractOptions(
    contract?.mob_definition?.repeat_iteration_inputs,
    currentMode || contractDefaultValue(contract, "repeat_iteration_input"),
    {
      carry: "Carry — last body step's output feeds the next pass",
    },
    "mob_definition.repeat_iteration_inputs"
  );
}

export function editorFlowPrimitiveOptions(contract, basicView = null) {
  const view = basicEditorViewState(basicView);
  const stepTypes = Array.isArray(contract?.mob_definition?.editor_flow_step_types) && contract.mob_definition.editor_flow_step_types.length
    ? contract.mob_definition.editor_flow_step_types.map(String)
    : [];
  const metadata = Object.fromEntries((view.flowPrimitiveRows || []).map((row) => [row.id, row]));
  const supportedRows = stepTypes
    .filter((type) => metadata[type])
    .map((type) => metadata[type]);
  return supportedRows;
}

export function graphControlNodes(contract, graphView = null) {
  const view = graphCanvasViewState(graphView);
  const metadata = Object.fromEntries((view.gatePaletteRows || []).map((row) => [row.id, row]));
  const paletteKinds = Array.isArray(contract?.mob_definition?.graph_palette_gate_kinds)
    ? contract.mob_definition.graph_palette_gate_kinds.map(String)
    : [];
  return graphGateKindOptions(contract, "")
    .filter((option) => !option.disabled && paletteKinds.includes(option.value) && metadata[option.value])
    .map((option) => ({
      id: option.value,
      gateKind: option.value,
      glyph: metadata[option.value].glyph,
      label: metadata[option.value].label,
      meta: metadata[option.value].meta,
    }));
}

export function graphAddNodeMenuState({ members = [], contract = null, query = "", graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const q = String(query || "");
  const ql = q.trim().toLowerCase();
  const memberRows = (Array.isArray(members) ? members : [])
    .filter((member) => {
      if (!ql) return true;
      return [
        member?.name,
        member?.role,
        member?.model,
      ].map((part) => String(part || "")).join(" ").toLowerCase().includes(ql);
    })
    .map((member) => ({
      id: String(member.id || ""),
      role: String(member.role || ""),
      name: String(member.name || ""),
      model: String(member.model || ""),
      dotStyle: roleAccentStyle(member.role),
      pick: { kind: "memberInstance", memberId: member.id },
    }))
    .filter((row) => row.id);
  const controls = graphControlNodes(contract, graphView);
  const controlRows = controls
    .filter((node) => {
      if (!ql) return true;
      return [
        node?.label,
        node?.meta,
        node?.gateKind,
      ].map((part) => String(part || "")).join(" ").toLowerCase().includes(ql);
    })
    .map((node) => ({
      id: String(node.id || ""),
      gateKind: String(node.gateKind || ""),
      glyph: String(node.glyph || ""),
      label: String(node.label || ""),
      meta: String(node.meta || ""),
      pick: { kind: "gate", gateKind: node.gateKind },
    }))
    .filter((row) => row.id);
  const terminalRows = [];
  return {
    searchIcon: view.addNodeSearchIcon,
    searchPlaceholder: view.addNodeSearchPlaceholder,
    closeLabel: view.addNodeCloseLabel,
    closeTitle: view.addNodeCloseTitle,
    agentsLabel: view.addNodeAgentsLabel,
    controlsLabel: view.addNodeControlsLabel,
    terminalsLabel: view.addNodeTerminalsLabel,
    emptyLabel: `${view.addNodeEmptyPrefix}${q}${view.addNodeEmptySuffix}`,
    jumpLabel: view.addNodeJumpLabel,
    memberRows,
    controlRows,
    terminalRows,
    hasMembers: memberRows.length > 0,
    hasControls: controlRows.length > 0,
    hasTerminals: terminalRows.length > 0,
    isEmpty: memberRows.length === 0 && controlRows.length === 0 && terminalRows.length === 0,
  };
}

export function graphAddMenuOpenProjection({ col, row, grid } = {}) {
  const cell = graphCellXY(grid, col, row);
  return {
    addAt: {
      col,
      row,
      x: cell.x + Number(grid?.cellW || 0) * 0.5 - 130,
      y: 90,
    },
  };
}

export function graphAddMenuCloseProjection() {
  return { addAt: null };
}

export function basicStepPickerState({ members = [], contract = null, query = "", isKickoff = false, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  if (isKickoff) {
    return {
      mode: "kickoff",
      title: view.pickerKickoffTitle,
      sub: view.pickerKickoffSub,
      kickoffHint: view.pickerKickoffHint,
    };
  }
  const q = String(query || "");
  const ql = q.trim().toLowerCase();
  const memberRows = (Array.isArray(members) ? members : [])
    .filter((member) => {
      if (!ql) return true;
      return [
        member?.name,
        member?.role,
      ].map((part) => String(part || "")).join(" ").toLowerCase().includes(ql);
    })
    .map((member) => ({
      id: String(member.id || ""),
      name: String(member.name || ""),
      role: String(member.role || ""),
      model: String(member.model || ""),
      schema: String(member.schema || ""),
      icon: "◆",
      iconTint: "accent",
      sub: [
        member?.role,
        member?.model,
        member?.schema,
      ].map((part) => String(part || "").trim()).filter(Boolean).join(" · "),
      pick: { kind: "member", id: member.id },
    }))
    .filter((row) => row.id);
  const primitiveRows = editorFlowPrimitiveOptions(contract, basicView)
    .filter((primitive) => {
      if (!ql) return true;
      return [
        primitive?.label,
        primitive?.sub,
      ].map((part) => String(part || "")).join(" ").toLowerCase().includes(ql);
    })
    .map((primitive) => ({
      id: String(primitive.id || ""),
      glyph: String(primitive.glyph || ""),
      tint: String(primitive.tint || ""),
      label: String(primitive.label || ""),
      sub: String(primitive.sub || ""),
      isNew: Boolean(primitive.isNew),
      disabled: Boolean(primitive.disabled),
      disabledReason: String(primitive.disabledReason || ""),
      pick: primitive.disabled ? null : { kind: primitive.id },
    }))
    .filter((row) => row.id);
  return {
    mode: "picker",
    title: view.pickerTitle,
    sub: view.pickerSub,
    searchIcon: view.pickerSearchIcon,
    searchPlaceholder: view.pickerSearchPlaceholder,
    membersLabel: view.pickerMembersLabel,
    flowLabel: view.pickerFlowLabel,
    emptyMembersHint: view.pickerEmptyMembersHint,
    newBadgeLabel: view.pickerNewBadgeLabel,
    memberRows,
    primitiveRows,
    hasConfiguredMembers: Array.isArray(members) && members.length > 0,
  };
}

export function firstSupportedOption(options, preferred = []) {
  const list = Array.isArray(options) ? options : [];
  for (const value of preferred) {
    const option = list.find((candidate) => candidate.value === value && !candidate.disabled);
    if (option) return option.value;
  }
  return list.find((option) => !option.disabled)?.value || "";
}

export function contractStringValues(values) {
  return Array.isArray(values)
    ? values.map((value) => String(value || "").trim()).filter(Boolean)
    : [];
}

export function firstContractValue(values, preferred = []) {
  const list = contractStringValues(values);
  for (const value of preferred) {
    if (list.includes(value)) return value;
  }
  return list[0] || "";
}

export function contractDefaultRaw(contract, name) {
  return String(contract?.mob_definition?.defaults?.[name] || "").trim();
}

export function contractDefaultFromList(contract, name, values, normalizer) {
  const raw = contractDefaultRaw(contract, name);
  if (!raw) return "";
  const normalized = normalizer ? normalizer(raw) : raw;
  const allowed = new Set(contractStringValues(values).map((value) => normalizer ? normalizer(value) : value));
  return allowed.has(normalized) ? normalized : "";
}

export function contractDefaultValue(contract, name) {
  const mob = contract?.mob_definition || {};
  switch (name) {
    case "launch_mode":
      return contractDefaultFromList(contract, "launch_mode", mob.launch_modes, canonicalLaunchModeKind);
    case "dispatch_mode":
      return contractDefaultFromList(contract, "dispatch_mode", mob.dispatch_modes);
    case "collection_policy":
      return contractDefaultFromList(contract, "collection_policy", mob.collection_policies);
    case "dependency_mode":
      return contractDefaultFromList(contract, "dependency_mode", mob.dependency_modes);
    case "condition_operator":
      return contractDefaultFromList(contract, "condition_operator", mob.condition_operators);
    case "fork_context":
      return contractDefaultFromList(contract, "fork_context", mob.fork_contexts, normalizeForkContext);
    case "graph_gate_kind":
      return contractDefaultFromList(contract, "graph_gate_kind", mob.graph_gate_kinds);
    case "graph_edge_kind":
      return contractDefaultFromList(contract, "graph_edge_kind", mob.graph_edge_kinds);
    case "graph_condition_edge_kind":
      return contractDefaultFromList(contract, "graph_condition_edge_kind", mob.graph_edge_kinds);
    case "graph_fanout_edge_kind":
      return contractDefaultFromList(contract, "graph_fanout_edge_kind", mob.graph_edge_kinds);
    case "graph_terminal_kind":
      return contractDefaultFromList(contract, "graph_terminal_kind", mob.graph_terminal_kinds);
    case "schema_field_type":
      return contractDefaultFromList(contract, "schema_field_type", mob.editor_schema_field_types);
    case "branch_param_type":
      return contractDefaultFromList(contract, "branch_param_type", mob.editor_schema_field_types);
    case "repeat_iteration_input":
      return contractDefaultFromList(contract, "repeat_iteration_input", mob.repeat_iteration_inputs);
    case "step_output_format":
      return contractDefaultFromList(contract, "step_output_format", mob.step_output_formats);
    case "runtime_mode":
      return contractDefaultFromList(contract, "runtime_mode", mob.runtime_modes);
    default:
      return "";
  }
}
