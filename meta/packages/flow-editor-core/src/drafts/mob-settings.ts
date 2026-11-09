// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the drafts/mob-settings functions move byte-verbatim as plain
// JS, and flowStepTemplate's `options = {}` parameter default raises TS2339
// under .ts semantics. Source-contract pins this exact text, so suppression
// must live at file level, not in the moved bodies. Resolution/linkage stays
// guarded behaviorally: the projection suite and export-keys test load the
// bundle and exercise these functions, so a missed import or re-export
// still fails the gate as a ReferenceError.
//
// Mobpack drafts/settings helpers for the Flow Editor controller plane.
// Moved verbatim from the controller.js drafts-mob-settings range: editor
// draft contracts (schema/input/graph), flow-step templates and ID
// reservation, output-format options, mob settings normalization, and the
// role-wiring/advanced-settings editor states. The editor_schema_draft
// chain (editorSchemaDraftField/Contract, editorSchemaFieldNameFallback)
// was seeded here in S5 for schema/field-edit.ts and keeps its original
// intra-cluster position. graphInstanceIdSet left for shared/normalize.ts
// in S1; diagnosticsToRows/deployResultToRows stay in the residue until
// S13 (shell/outcomes). editorSchemaDraftField calls schema/field-edit's
// schemaFieldName, a runtime-only import cycle with no module-init
// cross-calls. Two facade-internal stragglers no bridge could reach moved
// to their design-destined homes early: collectFlowStepIds (flow/step-tree.ts)
// and basicBranchDefaultLabel (editors/basic-editor.ts).
import {
  contractDefaultValue,
  contractStringValues,
  simpleContractOptions,
} from "../contract/options";
import { slug } from "../domain/tool-skill-access";
import { basicBranchDefaultLabel } from "../editors/basic-editor";
import { collectFlowStepIds } from "../flow/step-tree";
import { schemaFieldName } from "../schema/field-edit";
import { EMPTY_MOB_SETTINGS } from "../shared/constants";
import {
  normalizeOptionalObject,
  normalizeOutputFormat,
  normalizeProfileBackend,
} from "../shared/normalize";
import { settingsViewForState } from "../views/view-config";

export function editorSchemaDraftField(rawField) {
  if (!rawField || typeof rawField !== "object") return null;
  const name = schemaFieldName(rawField.name, "");
  if (!name) return null;
  return {
    name,
    required: rawField.required === true,
    description: String(rawField.description || ""),
    enumValues: Array.isArray(rawField.enumValues)
      ? rawField.enumValues.map((value) => String(value || "").trim()).filter(Boolean)
      : [],
  };
}

export function editorSchemaDraftContract(contract) {
  const draft = contract?.mob_definition?.editor_schema_draft;
  if (!draft || typeof draft !== "object") return null;
  const schemaIdPrefix = String(draft.schema_id_prefix || "").trim();
  const schemaFieldType = contractDefaultValue(contract, "schema_field_type");
  const initialField = editorSchemaDraftField(draft.initial_field);
  const addedField = editorSchemaDraftField(draft.added_field);
  if (!schemaIdPrefix || !schemaFieldType || !initialField || !addedField) return null;
  return { schemaIdPrefix, schemaFieldType, initialField, addedField };
}

export function editorInputParamDraftContract(contract) {
  const draft = contract?.mob_definition?.editor_input_param_draft;
  if (!draft || typeof draft !== "object") return null;
  const schemaFieldType = contractDefaultValue(contract, "schema_field_type");
  const addedField = editorSchemaDraftField(draft.added_field);
  if (!schemaFieldType || !addedField) return null;
  return { schemaFieldType, addedField };
}

export function editorInputStepDraftContract(contract) {
  const draft = contract?.mob_definition?.editor_input_step_draft;
  const step = draft?.default_step;
  if (!step || typeof step !== "object") return null;
  const idPrefix = String(step.id || "").trim();
  if (!idPrefix) return null;
  return {
    idPrefix,
    task: String(step.task || ""),
    fields: String(step.fields || ""),
    inputParams: Array.isArray(step.inputParams) ? JSON.parse(JSON.stringify(step.inputParams)) : [],
  };
}

export function inputStepDraft(contract, flow) {
  const draft = editorInputStepDraftContract(contract);
  return {
    id: uniqueFlowStepId(draft?.idPrefix || "input", flow),
    type: "input",
    task: draft?.task || "",
    fields: draft?.fields || "",
    inputParams: Array.isArray(draft?.inputParams) ? JSON.parse(JSON.stringify(draft.inputParams)) : [],
  };
}

export function editorSchemaFieldNameFallback(contract) {
  const draft = editorSchemaDraftContract(contract);
  return draft?.addedField?.name || draft?.initialField?.name || "field";
}

export function editorInputParamNameFallback(contract) {
  return editorInputParamDraftContract(contract)?.addedField?.name || "param";
}

export function editorGraphDraftContract(contract) {
  const draft = contract?.mob_definition?.editor_graph_draft;
  if (!draft || typeof draft !== "object") return null;
  const parallelLaneLabels = Array.isArray(draft.parallel_lane_labels)
    ? draft.parallel_lane_labels.map((label) => String(label || "").trim()).filter(Boolean)
    : [];
  const out = {
    branchGateLabel: String(draft.branch_gate_label || "").trim(),
    branchConditionLaneLabel: String(draft.branch_condition_lane_label || "").trim(),
    branchFallbackLaneLabel: String(draft.branch_fallback_lane_label || "").trim(),
    branchJoinLabel: String(draft.branch_join_label || "").trim(),
    fallbackEdgeLabel: String(draft.fallback_edge_label || "").trim(),
    parallelLaneLabels,
    parallelEdgeLabel: String(draft.parallel_edge_label || "").trim(),
    reworkEdgeLabel: String(draft.rework_edge_label || "").trim(),
    terminalEdgeLabelPrefix: String(draft.terminal_edge_label_prefix || ""),
    joinLabelPrefix: String(draft.join_label_prefix || ""),
    joinQuorumLabelPrefix: String(draft.join_quorum_label_prefix || ""),
    branchFrameLabelPrefix: String(draft.branch_frame_label_prefix || ""),
    branchFrameSingularSuffix: String(draft.branch_frame_singular_suffix || ""),
    branchFramePluralSuffix: String(draft.branch_frame_plural_suffix || ""),
    parallelFrameLabelPrefix: String(draft.parallel_frame_label_prefix || ""),
    parallelFrameJoinInfix: String(draft.parallel_frame_join_infix || ""),
    parallelMissingDispatchLabel: String(draft.parallel_missing_dispatch_label || "").trim(),
    parallelMissingCollectionLabel: String(draft.parallel_missing_collection_label || "").trim(),
    repeatFrameLabelPrefix: String(draft.repeat_frame_label_prefix || ""),
    repeatMaxIterationsPrefix: String(draft.repeat_max_iterations_prefix || ""),
    repeatMissingMaxIterationsLabel: String(draft.repeat_missing_max_iterations_label || "").trim(),
    repeatEdgeUntilPrefix: String(draft.repeat_edge_until_prefix || ""),
    repeatEdgeUntilFallback: String(draft.repeat_edge_until_fallback || "").trim(),
  };
  if (!out.branchGateLabel || !out.branchConditionLaneLabel || !out.branchFallbackLaneLabel
    || !out.branchJoinLabel || !out.fallbackEdgeLabel || out.parallelLaneLabels.length < 2
    || !out.parallelEdgeLabel || !out.reworkEdgeLabel || !out.terminalEdgeLabelPrefix
    || !out.joinLabelPrefix || !out.joinQuorumLabelPrefix || !out.branchFrameLabelPrefix || !out.branchFrameSingularSuffix
    || !out.branchFramePluralSuffix || !out.parallelFrameLabelPrefix || !out.parallelFrameJoinInfix
    || !out.parallelMissingDispatchLabel || !out.parallelMissingCollectionLabel
    || !out.repeatFrameLabelPrefix || !out.repeatMaxIterationsPrefix
    || !out.repeatMissingMaxIterationsLabel || !out.repeatEdgeUntilPrefix
    || !out.repeatEdgeUntilFallback) {
    return null;
  }
  return out;
}

export function emptyGraphDraftContract() {
  return {
    branchGateLabel: "",
    branchConditionLaneLabel: "",
    branchFallbackLaneLabel: "",
    branchJoinLabel: "",
    fallbackEdgeLabel: "",
    parallelLaneLabels: [],
    parallelEdgeLabel: "",
    reworkEdgeLabel: "",
    terminalEdgeLabelPrefix: "",
    joinLabelPrefix: "",
    joinQuorumLabelPrefix: "",
    branchFrameLabelPrefix: "",
    branchFrameSingularSuffix: "",
    branchFramePluralSuffix: "",
    parallelFrameLabelPrefix: "",
    parallelFrameJoinInfix: "",
    parallelMissingDispatchLabel: "",
    parallelMissingCollectionLabel: "",
    repeatFrameLabelPrefix: "",
    repeatMaxIterationsPrefix: "",
    repeatMissingMaxIterationsLabel: "",
    repeatEdgeUntilPrefix: "",
    repeatEdgeUntilFallback: "",
  };
}

export function agentNavigationProjection(memberId = null) {
  const id = String(memberId || "").trim();
  return {
    view: "agents",
    addAt: null,
    selection: id ? { kind: "agent", id } : null,
  };
}

export function flowStepTemplate(pick, contract, options = {}) {
  const kind = String(pick?.kind || "").trim();
  const id = uniqueFlowStepId("s", options.flow);
  const branchIds = collectFlowBranchIds(options.flow?.steps || []);
  const dependencyMode = contractDefaultValue(contract, "dependency_mode");
  if (!dependencyMode) return null;
  const stepTypes = contractStringValues(contract?.mob_definition?.editor_flow_step_types);
  if (kind === "member") {
    return {
      id,
      type: "member",
      role: String(pick?.id || "").trim(),
      instruction: "",
      dependsMode: dependencyMode,
    };
  }
  if (!stepTypes.includes(kind)) return null;
  if (kind === "branch") {
    return {
      id,
      type: "branch",
      controllerRole: "",
      branches: [{ id: reserveFlowBranchId("br", branchIds), label: basicBranchDefaultLabel(1, options.basicView), condition: "", steps: [] }],
      fallback: [],
      dependsMode: dependencyMode,
    };
  }
  if (kind === "parallel") {
    const dispatch = contractDefaultValue(contract, "dispatch_mode");
    const collection = contractDefaultValue(contract, "collection_policy");
    if (!dispatch || !collection) return null;
    return {
      id,
      type: "parallel",
      controllerRole: "",
      dispatch,
      collection,
      branches: [
        { id: reserveFlowBranchId("br", branchIds), label: basicBranchDefaultLabel(1, options.basicView), steps: [] },
        { id: reserveFlowBranchId("br", branchIds), label: basicBranchDefaultLabel(2, options.basicView), steps: [] },
      ],
      dependsMode: dependencyMode,
    };
  }
  if (kind === "repeat") {
    return { id, type: "repeat", loopId: "", until: "", maxIterations: null, iterationInput: "", steps: [] };
  }
  return null;
}

export function uniqueFlowStepId(prefix, flow) {
  const stem = slug(prefix, "s");
  const base = `${stem}_1`;
  const used = collectFlowStepIds(flow?.steps || []);
  if (!used.has(base)) return base;
  let index = 2;
  while (used.has(`${stem}_${index}`)) index += 1;
  return `${stem}_${index}`;
}

export function reserveFlowBranchId(prefix, used) {
  const stem = slug(prefix, "br");
  const base = `${stem}_1`;
  const ids = used instanceof Set ? used : new Set();
  if (!ids.has(base)) {
    ids.add(base);
    return base;
  }
  let index = 2;
  while (ids.has(`${stem}_${index}`)) index += 1;
  const id = `${stem}_${index}`;
  ids.add(id);
  return id;
}

export function collectFlowBranchIds(steps, out = new Set()) {
  for (const step of steps || []) {
    if (step?.type === "branch" || step?.type === "parallel") {
      for (const branch of step.branches || []) {
        const id = String(branch?.id || "").trim();
        if (id) out.add(id);
        collectFlowBranchIds(branch?.steps || [], out);
      }
    }
    if (step?.type === "branch") collectFlowBranchIds(step.fallback || [], out);
    if (step?.type === "repeat") collectFlowBranchIds(step.steps || [], out);
  }
  return out;
}

export function outputFormatOptions(contract, currentFormat) {
  return simpleContractOptions(
    contract?.mob_definition?.step_output_formats,
    currentFormat || contractDefaultValue(contract, "step_output_format"),
    {
      json: "json — parse terminal output as JSON",
      text: "text — preserve terminal text",
    },
    "mob_definition.step_output_formats"
  );
}

export function outputFormatAllowed(contract, format) {
  const value = normalizeOutputFormat(format);
  if (!value) return true;
  const formats = Array.isArray(contract?.mob_definition?.step_output_formats)
    ? contract.mob_definition.step_output_formats.map(normalizeOutputFormat)
    : [];
  return formats.includes(value);
}

export function normalizeMobSettings(settings) {
  const source = settings && typeof settings === "object" ? settings : {};
  const advancedSource = source.advanced && typeof source.advanced === "object" ? source.advanced : {};
  const roleWiring = normalizeRoleWiring(source.roleWiring || source.role_wiring);
  return {
    ...EMPTY_MOB_SETTINGS,
    orchestrator: String(source.orchestrator || source.orchestratorProfile || source.orchestrator_profile || "").trim(),
    autoWireOrchestrator: !!(source.autoWireOrchestrator ?? source.auto_wire_orchestrator),
    roleWiring,
    backendDefault: normalizeProfileBackend(source.backendDefault || source.backend_default || source.backend?.default) || "",
    externalAddressBase: String(source.externalAddressBase || source.external_address_base || source.backend?.external?.address_base || "").trim(),
    advanced: {
      topology: normalizeOptionalObject(advancedSource.topology || source.topology),
      supervisor: normalizeOptionalObject(advancedSource.supervisor || source.supervisor),
      limits: normalizeOptionalObject(advancedSource.limits || source.limits),
      spawnPolicy: normalizeOptionalObject(advancedSource.spawnPolicy || advancedSource.spawn_policy || source.spawnPolicy || source.spawn_policy),
      eventRouter: normalizeOptionalObject(advancedSource.eventRouter || advancedSource.event_router || source.eventRouter || source.event_router),
    },
  };
}

export function normalizeRoleWiring(value) {
  if (!Array.isArray(value)) return [];
  return value
    .map((rule) => ({
      a: String(rule?.a || "").trim(),
      b: String(rule?.b || "").trim(),
    }))
    .filter((rule) => rule.a && rule.b);
}

export function mobRoleWiringEditorState(value, profileOptions, settingsView = null) {
  const view = settingsViewForState(settingsView);
  const options = Array.isArray(profileOptions) ? profileOptions : [];
  const wiring = normalizeRoleWiring(value);
  return {
    label: view.roleWiringLabel,
    countLabel: String(wiring.length),
    addLabel: view.roleWiringAddLabel,
    addDisabled: !options.length,
    options,
    wiring,
  };
}

export function roleWiringOptionValues(profileOptions) {
  return (Array.isArray(profileOptions) ? profileOptions : [])
    .map((option) => String(option?.value || option || "").trim())
    .filter(Boolean);
}

export function normalizeRoleWiringForOptions(wiring, profileOptions) {
  const allowed = new Set(roleWiringOptionValues(profileOptions));
  if (!allowed.size) return [];
  return normalizeRoleWiring(wiring).filter((rule) => allowed.has(rule.a) && allowed.has(rule.b));
}

export function mobRoleWiringUpdatePatch(wiring, index, patch, profileOptions) {
  const rules = normalizeRoleWiring(wiring);
  const ruleIndex = Number(index);
  if (!Number.isInteger(ruleIndex) || ruleIndex < 0 || ruleIndex >= rules.length) return rules;
  return normalizeRoleWiringForOptions(
    rules.map((rule, i) => i === ruleIndex ? { ...rule, ...(patch || {}) } : rule),
    profileOptions,
  );
}

export function mobRoleWiringSourcePatch(wiring, index, rawValue, profileOptions) {
  return mobRoleWiringUpdatePatch(wiring, index, { a: String(rawValue || "").trim() }, profileOptions);
}

export function mobRoleWiringTargetPatch(wiring, index, rawValue, profileOptions) {
  return mobRoleWiringUpdatePatch(wiring, index, { b: String(rawValue || "").trim() }, profileOptions);
}

export function mobRoleWiringDeletePatch(wiring, index) {
  const rules = normalizeRoleWiring(wiring);
  const ruleIndex = Number(index);
  if (!Number.isInteger(ruleIndex) || ruleIndex < 0 || ruleIndex >= rules.length) return rules;
  return rules.filter((_, i) => i !== ruleIndex);
}

export function mobRoleWiringAddPatch(wiring, profileOptions) {
  const rules = normalizeRoleWiring(wiring);
  const options = roleWiringOptionValues(profileOptions);
  if (!options.length) return rules;
  return normalizeRoleWiring([
    ...rules,
    { a: options[0], b: options[1] || options[0] },
  ]);
}

export function advancedMobSettingsEditorState(value, settingsView = null) {
  const view = settingsViewForState(settingsView);
  return {
    label: view.advancedLabel,
    text: JSON.stringify(value || {}, null, 2),
  };
}

export function advancedMobSettingsDraftPatch(text, settingsView = null) {
  const view = settingsViewForState(settingsView);
  try {
    const parsed = String(text || "").trim() ? JSON.parse(String(text)) : {};
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return { ok: false, error: view.advancedObjectRequiredError, value: null };
    }
    return { ok: true, error: "", value: normalizeMobSettings({ advanced: parsed }).advanced };
  } catch (err) {
    return { ok: false, error: err?.message || view.advancedInvalidJsonError, value: null };
  }
}

export function mobSettingsForUi(settings) {
  return normalizeMobSettings(settings);
}

export function mobDefaultsFromSchema(schema) {
  return mobSettingsForUi(schema?.mob_definition?.mob_settings?.defaults);
}
