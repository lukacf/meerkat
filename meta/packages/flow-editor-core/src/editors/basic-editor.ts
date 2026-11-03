// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the basic-editor functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (basicBranchConditionControlState,
// basicBranchParallelControlState, basicForkCanvasState, basicRepeatCanvasState,
// basicStepCardState, basicRepeatControlState, basicMemberStepControlState)
// raise TS2339 under .ts semantics. Source-contract pins this exact text, so
// suppression must live at file level, not in the moved bodies.
// Resolution/linkage stays guarded behaviorally: the projection suite and
// export-keys test load the bundle and exercise these functions, so a missed
// import or re-export still fails the gate as a ReferenceError.
//
// Basic editor control plane for the Flow Editor. Moved verbatim from the
// controller.js basic-editor range: legacy input-field parsing, input-param
// options and patches, basic condition text/option/patch helpers, branch and
// parallel control states, fork/repeat/step-card canvas states, and the
// member-step control state. basicEditorViewState is re-homed here from the
// residue's view-state range (extraction design S11), and
// basicBranchDefaultLabel (seeded here early in S7) returns to its original
// intra-cluster position. graphConditionRefForEdge/graphConditionOptions,
// which sat in this range, land in editors/graph-editor.ts per the design.
//
// SCC note: editors/basic-editor.ts and editors/graph-editor.ts were
// co-moved in S11 per the extraction design; with the condition-option
// re-homes the remaining cross-edges all point graph -> basic (runtime-only,
// no module-init cross-calls).
import {
  conditionOperatorOptions,
  contractDefaultValue,
  repeatIterationInputOptions,
} from "../contract/options";
import {
  collectFlowBranchIds,
  editorInputParamNameFallback,
  outputFormatOptions,
  reserveFlowBranchId,
} from "../drafts/mob-settings";
import {
  collectionPolicyOptions,
  dependencyModeOptions,
  dispatchModeOptions,
  launchModeControlState,
} from "../flow/launch-modes";
import { conditionValueLiteral, parseEditorConditionText } from "../flow/reconcile";
import { childLanes, collectFlowMemberSteps } from "../flow/step-tree";
import { normalizeSchemaLikeFieldPatch, uniqueInputParamName } from "../schema/field-edit";
import { normalizePositiveInteger, normalizeStringList } from "../shared/normalize";

export function basicEditorViewState(basicView) {
  const view = basicView && typeof basicView === "object" ? basicView : null;
  return {
    startLabel: String(view?.startLabel || ""),
    loopBadge: String(view?.loopBadge || ""),
    tipsTitle: String(view?.tipsTitle || ""),
    emptyPanelTitle: String(view?.emptyPanelTitle || ""),
    emptyPanelSubtitleParts: Array.isArray(view?.emptyPanelSubtitleParts)
      ? view.emptyPanelSubtitleParts
      : [],
    sourceToggleLabel: String(view?.sourceToggleLabel || ""),
    authoringOperationUnavailableError: String(view?.authoringOperationUnavailableError || ""),
    authoringOperationFallbackError: String(view?.authoringOperationFallbackError || ""),
    memberStepPanelTitleFallback: String(view?.memberStepPanelTitleFallback || ""),
    memberStepPanelSubFallback: String(view?.memberStepPanelSubFallback || ""),
    memberStepMemberLabel: String(view?.memberStepMemberLabel || ""),
    memberStepMemberPlaceholder: String(view?.memberStepMemberPlaceholder || ""),
    memberStepRuntimeDefaultLabel: String(view?.memberStepRuntimeDefaultLabel || ""),
    memberStepInstructionLabel: String(view?.memberStepInstructionLabel || ""),
    memberStepInstructionPlaceholder: String(view?.memberStepInstructionPlaceholder || ""),
    memberStepDispatchLabel: String(view?.memberStepDispatchLabel || ""),
    memberStepCollectionLabel: String(view?.memberStepCollectionLabel || ""),
    memberStepQuorumLabel: String(view?.memberStepQuorumLabel || ""),
    memberStepQuorumPlaceholder: String(view?.memberStepQuorumPlaceholder || ""),
    memberStepTimeoutLabel: String(view?.memberStepTimeoutLabel || ""),
    memberStepDependencyLabel: String(view?.memberStepDependencyLabel || ""),
    memberStepOutputFormatLabel: String(view?.memberStepOutputFormatLabel || ""),
    memberStepAllowedToolsLabel: String(view?.memberStepAllowedToolsLabel || ""),
    memberStepAllowedToolsEmptyLabel: String(view?.memberStepAllowedToolsEmptyLabel || ""),
    memberStepBlockedToolsLabel: String(view?.memberStepBlockedToolsLabel || ""),
    memberStepBlockedToolsEmptyLabel: String(view?.memberStepBlockedToolsEmptyLabel || ""),
    memberStepSchemaHintPrefix: String(view?.memberStepSchemaHintPrefix || ""),
    memberStepSchemaHintToolsPrefix: String(view?.memberStepSchemaHintToolsPrefix || ""),
    memberStepSchemaHintEmptyToolsLabel: String(view?.memberStepSchemaHintEmptyToolsLabel || ""),
    toolScopeNotInCatalogReason: String(view?.toolScopeNotInCatalogReason || ""),
    toolScopeNotEnabledReason: String(view?.toolScopeNotEnabledReason || ""),
    toolScopeToolDescriptionFallback: String(view?.toolScopeToolDescriptionFallback || ""),
    toolScopeRemoveLabel: String(view?.toolScopeRemoveLabel || ""),
    toolScopeSelectMemberPlaceholder: String(view?.toolScopeSelectMemberPlaceholder || ""),
    toolScopeBlockCatalogPlaceholder: String(view?.toolScopeBlockCatalogPlaceholder || ""),
    toolScopeAddProfilePlaceholder: String(view?.toolScopeAddProfilePlaceholder || ""),
    inputPanelIcon: String(view?.inputPanelIcon || ""),
    inputPanelTitle: String(view?.inputPanelTitle || ""),
    inputPanelSub: String(view?.inputPanelSub || ""),
    inputTaskLabel: String(view?.inputTaskLabel || ""),
    inputTaskPlaceholder: String(view?.inputTaskPlaceholder || ""),
    inputParamsTitlePrefix: String(view?.inputParamsTitlePrefix || ""),
    inputAddParamLabel: String(view?.inputAddParamLabel || ""),
    inputParamSourceLabel: String(view?.inputParamSourceLabel || ""),
    inputParamHeaderLabels: {
      name: String(view?.inputParamHeaderLabels?.name || ""),
      type: String(view?.inputParamHeaderLabels?.type || ""),
      required: String(view?.inputParamHeaderLabels?.required || ""),
      description: String(view?.inputParamHeaderLabels?.description || ""),
      action: String(view?.inputParamHeaderLabels?.action || ""),
    },
    inputParamNamePlaceholder: String(view?.inputParamNamePlaceholder || ""),
    inputParamDescriptionPlaceholder: String(view?.inputParamDescriptionPlaceholder || ""),
    inputParamRemoveTitle: String(view?.inputParamRemoveTitle || ""),
    inputParamEnumLabel: String(view?.inputParamEnumLabel || ""),
    inputParamEnumAddLabel: String(view?.inputParamEnumAddLabel || ""),
    inputParamEnumAddValue: String(view?.inputParamEnumAddValue || ""),
    inputEmptyParamsParts: Array.isArray(view?.inputEmptyParamsParts) ? view.inputEmptyParamsParts : [],
    inputTips: Array.isArray(view?.inputTips) ? view.inputTips : [],
    branchPanelTitle: String(view?.branchPanelTitle || ""),
    branchPanelSub: String(view?.branchPanelSub || ""),
    parallelPanelTitle: String(view?.parallelPanelTitle || ""),
    parallelPanelSub: String(view?.parallelPanelSub || ""),
    branchRouteMemberLabel: String(view?.branchRouteMemberLabel || ""),
    parallelJoinMemberLabel: String(view?.parallelJoinMemberLabel || ""),
    branchControllerPlaceholderLabel: String(view?.branchControllerPlaceholderLabel || ""),
    branchEmptyControllerHint: String(view?.branchEmptyControllerHint || ""),
    branchConditionTitle: String(view?.branchConditionTitle || ""),
    branchConditionIntro: String(view?.branchConditionIntro || ""),
    branchConditionRowTitlePrefix: String(view?.branchConditionRowTitlePrefix || ""),
    branchConditionEmptyHint: String(view?.branchConditionEmptyHint || ""),
    branchConditionSourcePlaceholder: String(view?.branchConditionSourcePlaceholder || ""),
    branchConditionFieldPlaceholder: String(view?.branchConditionFieldPlaceholder || ""),
    branchConditionNoSchemaLabel: String(view?.branchConditionNoSchemaLabel || ""),
    branchConditionPreviewPrefix: String(view?.branchConditionPreviewPrefix || ""),
    branchConditionPreviewFallback: String(view?.branchConditionPreviewFallback || ""),
    branchFallbackTitle: String(view?.branchFallbackTitle || ""),
    branchFallbackHint: String(view?.branchFallbackHint || ""),
    addBranchLabel: String(view?.addBranchLabel || ""),
    addParallelBranchLabel: String(view?.addParallelBranchLabel || ""),
    parallelDispatchLabel: String(view?.parallelDispatchLabel || ""),
    parallelCollectionLabel: String(view?.parallelCollectionLabel || ""),
    parallelQuorumLabel: String(view?.parallelQuorumLabel || ""),
    parallelQuorumPlaceholder: String(view?.parallelQuorumPlaceholder || ""),
    branchDependencyLabel: String(view?.branchDependencyLabel || ""),
    repeatPanelTitle: String(view?.repeatPanelTitle || ""),
    repeatPanelSub: String(view?.repeatPanelSub || ""),
    repeatLoopIdLabel: String(view?.repeatLoopIdLabel || ""),
    repeatLoopIdPlaceholder: String(view?.repeatLoopIdPlaceholder || ""),
    repeatConditionTitle: String(view?.repeatConditionTitle || ""),
    repeatConditionIntro: String(view?.repeatConditionIntro || ""),
    repeatEmptyBodyHint: String(view?.repeatEmptyBodyHint || ""),
    repeatMemberPlaceholderLabel: String(view?.repeatMemberPlaceholderLabel || ""),
    repeatConditionFieldPlaceholder: String(view?.repeatConditionFieldPlaceholder || ""),
    repeatConditionNoSchemaLabel: String(view?.repeatConditionNoSchemaLabel || ""),
    repeatPreviewLabel: String(view?.repeatPreviewLabel || ""),
    repeatPreviewFallback: String(view?.repeatPreviewFallback || ""),
    repeatIterationInputLabel: String(view?.repeatIterationInputLabel || ""),
    repeatMaxIterationsLabel: String(view?.repeatMaxIterationsLabel || ""),
    repeatMaxIterationsPlaceholder: String(view?.repeatMaxIterationsPlaceholder || ""),
    repeatTips: Array.isArray(view?.repeatTips) ? view.repeatTips : [],
    repeatCanvasWhileLabel: String(view?.repeatCanvasWhileLabel || ""),
    repeatCanvasNotLabel: String(view?.repeatCanvasNotLabel || ""),
    repeatCanvasMissingMaxIterationsLabel: String(view?.repeatCanvasMissingMaxIterationsLabel || ""),
    repeatCanvasMaxIterationsPrefix: String(view?.repeatCanvasMaxIterationsPrefix || ""),
    repeatCanvasLoopBackPrefix: String(view?.repeatCanvasLoopBackPrefix || ""),
    repeatCanvasExitPrefix: String(view?.repeatCanvasExitPrefix || ""),
    repeatCanvasExitFallback: String(view?.repeatCanvasExitFallback || ""),
    repeatIterationRuntimeDefaultLabel: String(view?.repeatIterationRuntimeDefaultLabel || ""),
    repeatIterationCarryLabel: String(view?.repeatIterationCarryLabel || ""),
    repeatIterationReuseUnsupportedLabel: String(view?.repeatIterationReuseUnsupportedLabel || ""),
    repeatIterationFeedsUnsupportedPrefix: String(view?.repeatIterationFeedsUnsupportedPrefix || ""),
    repeatIterationUnsupportedPrefix: String(view?.repeatIterationUnsupportedPrefix || ""),
    addStepTitle: String(view?.addStepTitle || ""),
    inputStepCardTitle: String(view?.inputStepCardTitle || ""),
    inputStepCardDescFallback: String(view?.inputStepCardDescFallback || ""),
    branchStepCardTitle: String(view?.branchStepCardTitle || ""),
    branchStepCardDesc: String(view?.branchStepCardDesc || ""),
    parallelStepCardTitle: String(view?.parallelStepCardTitle || ""),
    parallelStepCardDescPrefix: String(view?.parallelStepCardDescPrefix || ""),
    parallelStepCardCollectionFallback: String(view?.parallelStepCardCollectionFallback || ""),
    repeatStepCardTitle: String(view?.repeatStepCardTitle || ""),
    repeatStepCardDescPrefix: String(view?.repeatStepCardDescPrefix || ""),
    repeatStepCardDescFallback: String(view?.repeatStepCardDescFallback || ""),
    memberStepCardTitleFallback: String(view?.memberStepCardTitleFallback || ""),
    pickerKickoffTitle: String(view?.pickerKickoffTitle || ""),
    pickerKickoffSub: String(view?.pickerKickoffSub || ""),
    pickerKickoffHint: String(view?.pickerKickoffHint || ""),
    pickerTitle: String(view?.pickerTitle || ""),
    pickerSub: String(view?.pickerSub || ""),
    pickerSearchIcon: String(view?.pickerSearchIcon || ""),
    pickerSearchPlaceholder: String(view?.pickerSearchPlaceholder || ""),
    pickerMembersLabel: String(view?.pickerMembersLabel || ""),
    pickerFlowLabel: String(view?.pickerFlowLabel || ""),
    pickerEmptyMembersHint: String(view?.pickerEmptyMembersHint || ""),
    pickerNewBadgeLabel: String(view?.pickerNewBadgeLabel || ""),
    adaptiveDefaults: view?.adaptiveDefaults && typeof view.adaptiveDefaults === "object"
      ? view.adaptiveDefaults
      : null,
    adaptiveLimitRows: Array.isArray(view?.adaptiveLimitRows) ? view.adaptiveLimitRows : [],
    adaptiveTargetSurfaces: Array.isArray(view?.adaptiveTargetSurfaces) ? view.adaptiveTargetSurfaces : [],
    adaptivePanelTitle: String(view?.adaptivePanelTitle || ""),
    adaptivePanelSub: String(view?.adaptivePanelSub || ""),
    adaptiveFlowmasterLabel: String(view?.adaptiveFlowmasterLabel || ""),
    adaptiveFlowmasterPlaceholderLabel: String(view?.adaptiveFlowmasterPlaceholderLabel || ""),
    adaptivePromptLabel: String(view?.adaptivePromptLabel || ""),
    adaptivePromptPlaceholder: String(view?.adaptivePromptPlaceholder || ""),
    adaptiveObjectiveLabel: String(view?.adaptiveObjectiveLabel || ""),
    adaptiveObjectivePlaceholder: String(view?.adaptiveObjectivePlaceholder || ""),
    adaptiveResultSchemaLabel: String(view?.adaptiveResultSchemaLabel || ""),
    adaptiveResultSchemaPlaceholderLabel: String(view?.adaptiveResultSchemaPlaceholderLabel || ""),
    adaptiveProfilesTitle: String(view?.adaptiveProfilesTitle || ""),
    adaptiveProfilesHint: String(view?.adaptiveProfilesHint || ""),
    adaptiveProfilesAgentsHint: String(view?.adaptiveProfilesAgentsHint || ""),
    adaptiveProfilesEmptyHint: String(view?.adaptiveProfilesEmptyHint || ""),
    adaptiveSurfacesLabel: String(view?.adaptiveSurfacesLabel || ""),
    adaptiveInlineProfilesLabel: String(view?.adaptiveInlineProfilesLabel || ""),
    adaptiveLimitsTitle: String(view?.adaptiveLimitsTitle || ""),
    adaptiveLimitsAdvancedLabel: String(view?.adaptiveLimitsAdvancedLabel || ""),
    adaptiveTips: Array.isArray(view?.adaptiveTips) ? view.adaptiveTips : [],
    adaptiveStepCardTitle: String(view?.adaptiveStepCardTitle || ""),
    adaptiveBlockHeadPrefix: String(view?.adaptiveBlockHeadPrefix || ""),
    adaptiveBlockFlowmasterKicker: String(view?.adaptiveBlockFlowmasterKicker || ""),
    adaptiveBlockFlowmasterTitleFallback: String(view?.adaptiveBlockFlowmasterTitleFallback || ""),
    adaptiveBlockFlowmasterDesc: String(view?.adaptiveBlockFlowmasterDesc || ""),
    adaptiveBlockPlanConnector: String(view?.adaptiveBlockPlanConnector || ""),
    adaptiveBlockLayerKickerPrefix: String(view?.adaptiveBlockLayerKickerPrefix || ""),
    adaptiveBlockLayerKickerSuffix: String(view?.adaptiveBlockLayerKickerSuffix || ""),
    adaptiveBlockChipSuffix: String(view?.adaptiveBlockChipSuffix || ""),
    adaptiveBlockEmptyProfilesLabel: String(view?.adaptiveBlockEmptyProfilesLabel || ""),
    adaptiveBlockCollectConnectorPrefix: String(view?.adaptiveBlockCollectConnectorPrefix || ""),
    adaptiveBlockCollectFallback: String(view?.adaptiveBlockCollectFallback || ""),
    adaptiveBlockLoopBackPrefix: String(view?.adaptiveBlockLoopBackPrefix || ""),
    adaptiveBlockLoopBackSuffix: String(view?.adaptiveBlockLoopBackSuffix || ""),
    flowPrimitiveRows: Array.isArray(view?.flowPrimitiveRows) ? view.flowPrimitiveRows : [],
  };
}

export function parseLegacyInputFields(text) {
  return String(text || "")
    .split(/\n/)
    .flatMap(splitLegacyInputFieldLine)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line, index) => {
      const match = /^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(enum\(([^)]+)\)|[A-Za-z0-9_[\]]+)/.exec(line);
      if (!match) return null;
      const enumValues = match[3]
        ? match[3].split("|").join(",").split(",").map((value) => value.trim()).filter(Boolean)
        : [];
      return {
        id: `p${index + 1}`,
        name: match[1],
        type: enumValues.length ? "enum" : match[2],
        required: true,
        description: "",
        enumValues,
      };
    })
    .filter(Boolean);
}

export function splitLegacyInputFieldLine(line) {
  const out = [];
  let start = 0;
  let depth = 0;
  const raw = String(line || "");
  for (let index = 0; index < raw.length; index += 1) {
    const char = raw[index];
    if (char === "(") depth += 1;
    if (char === ")" && depth > 0) depth -= 1;
    if (char === "," && depth === 0) {
      out.push(raw.slice(start, index));
      start = index + 1;
    }
  }
  out.push(raw.slice(start));
  return out;
}

export function inputParamsForStep(step) {
  if (Array.isArray(step?.inputParams)) return step.inputParams;
  return parseLegacyInputFields(step?.fields);
}

export function inputParamSummary(params, contract) {
  const defaultType = contractDefaultValue(contract, "schema_field_type");
  return (params || [])
    .map((param) => `${param.name}: ${param.type || defaultType}${param.required ? "" : "?"}`)
    .join(", ");
}

export function inputParamOptions(flow, basicView = null) {
  const input = (flow?.steps || []).find((step) => step.type === "input");
  const fields = inputParamsForStep(input);
  if (!fields.length) return [];
  const view = basicEditorViewState(basicView);
  return [{
    stepId: "params",
    namespace: "params",
    label: view.inputParamSourceLabel,
    fields,
  }];
}

export function basicInputControlState(step, contract, basicView = null) {
  const params = inputParamsForStep(step);
  const view = basicEditorViewState(basicView);
  return {
    panelIcon: view.inputPanelIcon,
    panelTitle: view.inputPanelTitle,
    panelSub: view.inputPanelSub,
    taskLabel: view.inputTaskLabel,
    taskPlaceholder: view.inputTaskPlaceholder,
    params,
    paramsTitle: `${view.inputParamsTitlePrefix} · ${params.length}`,
    addParamLabel: view.inputAddParamLabel,
    headerRows: [
      { key: "name", label: view.inputParamHeaderLabels.name, className: "sb-col sb-col--name" },
      { key: "type", label: view.inputParamHeaderLabels.type, className: "sb-col sb-col--type" },
      { key: "required", label: view.inputParamHeaderLabels.required, className: "sb-col sb-col--req" },
      { key: "description", label: view.inputParamHeaderLabels.description, className: "sb-col sb-col--desc" },
      { key: "actions", label: view.inputParamHeaderLabels.action, className: "sb-col sb-col--act" },
    ],
    emptyParamsParts: view.inputEmptyParamsParts,
    tips: view.inputTips,
  };
}

export function basicConditionOptions(flow, targetId, members, basicView = null) {
  return [
    ...inputParamOptions(flow, basicView),
    ...memberConditionOptionsBefore(flow?.steps || [], targetId, members).out,
  ];
}

export function memberConditionOptionsBefore(steps, targetId, members, out = []) {
  const memberById = new Map((Array.isArray(members) ? members : [])
    .filter((member) => member?.id)
    .map((member) => [member.id, member]));
  return memberConditionOptionsBeforeWithMap(steps, targetId, memberById, out);
}

export function memberConditionOptionsBeforeWithMap(steps, targetId, memberById, out = []) {
  const target = String(targetId || "");
  for (const step of steps || []) {
    if (step?.id === target) return { found: true, out };
    if (step?.type === "member" && step.role) {
      const member = memberById.get(step.role);
      if (member) {
        out.push({
          stepId: step.id,
          namespace: "steps",
          member,
          label: member.name || member.role || member.id || step.id,
          fields: [],
        });
      }
    }
    for (const lane of childLanes(step)) {
      const result = memberConditionOptionsBeforeWithMap(lane.steps, target, memberById, [...out]);
      if (result.found) return result;
    }
  }
  return { found: false, out };
}

export function parseGraphConditionVar(value) {
  const text = String(value || "").trim();
  const params = /^params\.([A-Za-z0-9_.-]+)$/.exec(text);
  if (params) return { instanceId: "params", field: params[1], namespace: "params" };
  const match = /^steps\.([A-Za-z0-9_.-]+)\.([A-Za-z0-9_.-]+)$/.exec(text)
    || /^([A-Za-z0-9_.-]+)\.([A-Za-z0-9_.-]+)$/.exec(text);
  if (!match) return { instanceId: "", field: "", namespace: "" };
  return { instanceId: match[1], field: match[2], namespace: "steps" };
}

export function inputParamUpdatePatch(params, id, patch, contract) {
  const source = Array.isArray(params) ? params : [];
  const current = source.find((param) => param?.id === id) || null;
  if (!current) return { inputParams: source, fields: inputParamSummary(source, contract) };
  const normalized = normalizeSchemaLikeFieldPatch(current, patch, contract);
  if (Object.prototype.hasOwnProperty.call(normalized, "name")) {
    normalized.name = uniqueInputParamName(source, normalized.name, id, editorInputParamNameFallback(contract));
  }
  const next = source.map((param) => param?.id === id ? { ...param, ...normalized } : param);
  return { inputParams: next, fields: inputParamSummary(next, contract) };
}

export function inputParamDeletePatch(params, id, contract) {
  const removed = (params || []).find((param) => param?.id === id) || null;
  const next = (params || []).filter((param) => param?.id !== id);
  return { removed, patch: { inputParams: next, fields: inputParamSummary(next, contract) } };
}

export function inputParamRenamePatch(params, id, rawName, contract) {
  const nextName = uniqueInputParamName(params, rawName, id, editorInputParamNameFallback(contract));
  const next = (params || []).map((param) => param?.id === id ? { ...param, name: nextName } : param);
  return { name: nextName, patch: { inputParams: next, fields: inputParamSummary(next, contract) } };
}

export function basicConditionFromText(text) {
  return parseEditorConditionText(text);
}

export function basicConditionText(cond, options = {}) {
  if (!cond || !cond.stepId || !cond.field) return "";
  const op = cond.op || cond.operator || options.defaultOperator || "";
  if (!op) return "";
  if (cond.namespace === "params" || cond.stepId === "params") {
    return `params.${cond.field} ${op} ${conditionValueLiteral(cond.val ?? cond.value ?? "")}`;
  }
  return `steps.${cond.stepId}.${cond.field} ${op} ${conditionValueLiteral(cond.val ?? cond.value ?? "")}`;
}

export function basicBranchConditionPatch(step, branchId, patch = {}, contract) {
  const branches = (step?.branches || []).map((branch) => {
    if (branch?.id !== branchId) return branch;
    const defaultOperator = contractDefaultValue(contract, "condition_operator");
    const cond = {
      ...(branch.cond || basicConditionFromText(branch.condition) || {}),
      ...patch,
    };
    return {
      ...branch,
      cond,
      condition: basicConditionText(cond, { defaultOperator }),
    };
  });
  return { branches };
}

export function basicBranchAddPatch(step, options = {}) {
  const branches = Array.isArray(step?.branches) ? step.branches : [];
  const branchIds = collectFlowBranchIds(options.flow?.steps || []);
  for (const branch of branches) {
    const id = String(branch?.id || "").trim();
    if (id) branchIds.add(id);
  }
  const nextBranch = {
    id: reserveFlowBranchId("br", branchIds),
    label: basicBranchDefaultLabel(branches.length + 1, options.basicView),
    steps: [],
  };
  if (step?.type !== "parallel") nextBranch.condition = "";
  return { branches: [...branches, nextBranch] };
}

export function basicBranchDefaultLabel(index, basicView = null) {
  const view = basicEditorViewState(basicView);
  const prefix = view.branchConditionRowTitlePrefix;
  return [prefix, String(index || 1)].filter(Boolean).join(" ");
}

export function basicConditionLabel(cond, options = [], config = {}) {
  if (!cond || !cond.stepId || !cond.field) return String(config.previewFallback || "");
  const option = (Array.isArray(options) ? options : []).find((candidate) => candidate.stepId === cond.stepId);
  const label = option?.label || option?.member?.name || cond.stepId;
  const op = cond.op || cond.operator || config.defaultOperator || "";
  return `${label}.${cond.field} ${op} ${conditionValueLiteral(cond.val ?? cond.value ?? "")}`;
}

export function basicBranchConditionControlState({ branch, options = [], schemas = [], contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const sourceOptions = Array.isArray(options) ? options : [];
  const sourceSchemas = Array.isArray(schemas) ? schemas : [];
  const defaultOperator = contractDefaultValue(contract, "condition_operator");
  const cond = branch?.cond || basicConditionFromText(branch?.condition) || {};
  const selected = sourceOptions.find((option) => option.stepId === cond.stepId) || null;
  const fields = selected?.namespace === "params"
    ? selected.fields || []
    : ((sourceSchemas.find((candidate) => candidate.id === selected?.member?.schema)?.fields) || []);
  const field = fields.find((candidate) => candidate.name === cond.field) || null;
  const operatorValue = cond.op || defaultOperator;
  return {
    cond,
    selected,
    fields,
    field,
    sourceOptions: sourceOptions.map((option) => ({
      value: option.stepId,
      label: option.label || option.member?.name || option.stepId,
      option,
    })),
    fieldOptions: fields.map((candidate) => ({
      value: candidate.name,
      label: `${candidate.name} · ${candidate.type}`,
      field: candidate,
    })),
    rowTitle: `${view.branchConditionRowTitlePrefix} ${Number.isFinite(Number(branch?.index)) ? Number(branch.index) + 1 : ""}`.trim(),
    emptyHint: view.branchConditionEmptyHint,
    sourcePlaceholder: view.branchConditionSourcePlaceholder,
    fieldPlaceholder: fields.length ? view.branchConditionFieldPlaceholder : view.branchConditionNoSchemaLabel,
    defaultOperator,
    operatorValue,
    operatorOptions: conditionOperatorOptions(contract, operatorValue),
    previewPrefix: view.branchConditionPreviewPrefix,
    previewLabel: basicConditionLabel(cond, sourceOptions, {
      defaultOperator,
      previewFallback: view.branchConditionPreviewFallback,
    }),
    hasConditionOptions: sourceOptions.length > 0,
  };
}

export function basicBranchParallelControlState({ step, flow, members = [], contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const sourceMembers = Array.isArray(members) ? members : [];
  const isParallel = step?.type === "parallel";
  const controllerRole = step?.controllerRole || "";
  const defaultDispatch = contractDefaultValue(contract, "dispatch_mode");
  const dispatchValue = step?.dispatch || defaultDispatch;
  const defaultCollection = contractDefaultValue(contract, "collection_policy");
  const collectionValue = step?.collection || defaultCollection;
  const defaultDependency = contractDefaultValue(contract, "dependency_mode");
  const dependencyValue = step?.dependsMode || defaultDependency;
  const dispatchOptions = dispatchModeOptions(contract, dispatchValue);
  const collectionOptions = collectionPolicyOptions(contract, collectionValue);
  const dependencyOptions = dependencyModeOptions(contract, dependencyValue);
  return {
    isParallel,
    panelIcon: isParallel ? "‖" : "⑂",
    panelTitle: isParallel ? view.parallelPanelTitle : view.branchPanelTitle,
    panelSub: isParallel ? view.parallelPanelSub : view.branchPanelSub,
    controllerLabel: isParallel ? view.parallelJoinMemberLabel : view.branchRouteMemberLabel,
    controllerPlaceholderLabel: view.branchControllerPlaceholderLabel,
    controllerRole,
    memberOptions: sourceMembers.map((member) => ({
      value: member.id,
      label: `${member.name || member.role || member.id} · ${member.role || "profile"}`,
      member,
    })),
    emptyControllerHint: view.branchEmptyControllerHint,
    conditionOptions: basicConditionOptions(flow, step?.id, sourceMembers, basicView),
    branchConditionTitle: view.branchConditionTitle,
    branchConditionIntro: view.branchConditionIntro,
    fallbackTitle: view.branchFallbackTitle,
    fallbackHint: view.branchFallbackHint,
    addBranchLabel: isParallel ? view.addParallelBranchLabel : view.addBranchLabel,
    dispatchLabel: view.parallelDispatchLabel,
    dispatchValue,
    dispatchOptions,
    selectedDispatch: dispatchOptions.find((option) => option.value === dispatchValue) || null,
    collectionLabel: view.parallelCollectionLabel,
    collectionValue,
    collectionOptions,
    selectedCollection: collectionOptions.find((option) => option.value === collectionValue) || null,
    showQuorum: collectionValue === "quorum",
    quorumLabel: view.parallelQuorumLabel,
    quorumPlaceholder: view.parallelQuorumPlaceholder,
    dependencyLabel: view.branchDependencyLabel,
    dependencyValue,
    dependencyOptions,
    selectedDependency: dependencyOptions.find((option) => option.value === dependencyValue) || null,
  };
}

export function basicForkCanvasState({ step, contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const isParallel = step?.type === "parallel";
  const collection = step?.collection || contractDefaultValue(contract, "collection_policy");
  const branches = Array.isArray(step?.branches) ? step.branches : [];
  const lanes = [
    ...branches.map((branch) => ({ id: branch.id, label: branch.label, steps: branch.steps || [] })),
    ...(isParallel ? [] : [{ id: "fallback", label: view.branchFallbackTitle, steps: step?.fallback || [] }]),
  ];
  return {
    isParallel,
    className: "bld-fork" + (isParallel ? " bld-fork--parallel" : ""),
    lanes,
    showRail: lanes.length > 1,
    showJoin: isParallel,
    joinLabel: `⋈ join · ${collection || "—"}`,
  };
}

export function basicRepeatIterationLabel(step, members = [], basicView = null) {
  const view = basicEditorViewState(basicView);
  const iterationInput = typeof step?.iterationInput === "string" ? step.iterationInput.trim() : "";
  if (!iterationInput) return view.repeatIterationRuntimeDefaultLabel;
  if (iterationInput === "carry") return view.repeatIterationCarryLabel;
  if (iterationInput === "reuse") return view.repeatIterationReuseUnsupportedLabel;
  const bodyStep = (Array.isArray(step?.steps) ? step.steps : []).find((candidate) => candidate?.id === iterationInput);
  const member = (Array.isArray(members) ? members : []).find((candidate) => candidate?.id === bodyStep?.role);
  return member
    ? `${view.repeatIterationFeedsUnsupportedPrefix}${member.name}'s output`
    : `${view.repeatIterationUnsupportedPrefix}${iterationInput}`;
}

export function basicRepeatCanvasState({ step, members = [], contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const defaultOperator = contractDefaultValue(contract, "condition_operator");
  const repeatUntilExpression = basicRepeatUntilExpression(step, members, { defaultOperator });
  return {
    repeatUntilExpression,
    whileLabel: view.repeatCanvasWhileLabel,
    notLabel: view.repeatCanvasNotLabel,
    conditionLabel: repeatUntilExpression || view.repeatPreviewFallback,
    maxIterationsLabel: step?.maxIterations
      ? `${view.repeatCanvasMaxIterationsPrefix}${step.maxIterations}`
      : view.repeatCanvasMissingMaxIterationsLabel,
    loopBackLabel: `${view.repeatCanvasLoopBackPrefix}${basicRepeatIterationLabel(step, members, basicView)}`,
    exitLabel: `${view.repeatCanvasExitPrefix}${repeatUntilExpression || view.repeatCanvasExitFallback}`,
  };
}

export function basicStepCardState({ step, members = [], contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const sourceMembers = Array.isArray(members) ? members : [];
  const member = step?.role ? sourceMembers.find((candidate) => candidate?.id === step.role) || null : null;
  if (step?.type === "input") {
    return {
      icon: "▤",
      iconTint: "member",
      title: view.inputStepCardTitle,
      desc: step?.task ? step.task : view.inputStepCardDescFallback,
      configured: true,
      isFlowCard: false,
    };
  }
  if (step?.type === "branch") {
    return {
      icon: "⑂",
      iconTint: "member",
      title: view.branchStepCardTitle,
      desc: view.branchStepCardDesc,
      configured: true,
      isFlowCard: true,
    };
  }
  if (step?.type === "parallel") {
    const collection = step?.collection || contractDefaultValue(contract, "collection_policy") || view.parallelStepCardCollectionFallback;
    return {
      icon: "‖",
      iconTint: "member",
      title: view.parallelStepCardTitle,
      desc: `${view.parallelStepCardDescPrefix}${collection}`,
      configured: true,
      isFlowCard: true,
    };
  }
  if (step?.type === "repeat") {
    const defaultOperator = contractDefaultValue(contract, "condition_operator");
    const repeatUntilExpression = basicRepeatUntilExpression(step, sourceMembers, { defaultOperator });
    return {
      icon: "↻",
      iconTint: "member",
      title: view.repeatStepCardTitle,
      desc: repeatUntilExpression
        ? `${view.repeatStepCardDescPrefix}${repeatUntilExpression}`
        : view.repeatStepCardDescFallback,
      configured: true,
      isFlowCard: true,
    };
  }
  if (step?.type === "adaptive") {
    return {
      icon: "❖",
      iconTint: "member",
      title: view.adaptiveStepCardTitle,
      desc: typeof step?.objectiveClass === "string" ? step.objectiveClass : "",
      configured: !!String(step?.flowmasterId || "").trim(),
      isFlowCard: true,
    };
  }
  return {
    icon: "◆",
    iconTint: "accent",
    title: member ? member.name : view.memberStepCardTitleFallback,
    desc: step?.instruction || (member ? `${member.role} · ${member.model}` : ""),
    configured: !!step?.role,
    isFlowCard: false,
  };
}

export function basicRepeatControlState({ step, members = [], schemas = [], contract, basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const sourceMembers = Array.isArray(members) ? members : [];
  const sourceSchemas = Array.isArray(schemas) ? schemas : [];
  const memberById = new Map(sourceMembers.map((member) => [member.id, member]));
  const bodyMembers = (Array.isArray(step?.steps) ? step.steps : [])
    .filter((candidate) => candidate?.type === "member" && candidate.role)
    .map((candidate) => ({
      stepId: candidate.id,
      member: memberById.get(candidate.role) || null,
    }))
    .filter((candidate) => candidate.member);
  const cond = step?.cond || {};
  const condMember = bodyMembers.find((candidate) => candidate.stepId === cond.stepId)?.member || null;
  const condSchema = condMember
    ? sourceSchemas.find((schema) => schema.id === condMember.schema) || null
    : null;
  const fields = condSchema?.fields || [];
  const condField = fields.find((field) => field.name === cond.field) || null;
  const defaultOperator = contractDefaultValue(contract, "condition_operator");
  const operatorValue = cond.op || defaultOperator;
  const repeatUntilExpression = basicRepeatUntilExpression(step, sourceMembers, { defaultOperator });
  const iterationInputValue = typeof step?.iterationInput === "string" ? step.iterationInput : "";
  const iterationInputOptions = [
    { value: "", label: view.repeatIterationRuntimeDefaultLabel, disabled: false, reason: "" },
    ...repeatIterationInputOptions(contract, iterationInputValue),
  ];
  return {
    panelIcon: "↻",
    panelTitle: view.repeatPanelTitle,
    panelSub: view.repeatPanelSub,
    loopIdLabel: view.repeatLoopIdLabel,
    loopIdPlaceholder: view.repeatLoopIdPlaceholder,
    conditionTitle: view.repeatConditionTitle,
    conditionIntro: view.repeatConditionIntro,
    emptyBodyHint: view.repeatEmptyBodyHint,
    memberPlaceholderLabel: view.repeatMemberPlaceholderLabel,
    previewLabel: view.repeatPreviewLabel,
    previewFallback: view.repeatPreviewFallback,
    iterationInputLabel: view.repeatIterationInputLabel,
    maxIterationsLabel: view.repeatMaxIterationsLabel,
    maxIterationsPlaceholder: view.repeatMaxIterationsPlaceholder,
    tips: view.repeatTips,
    bodyMembers,
    bodyMemberOptions: bodyMembers.map((candidate) => ({
      value: candidate.stepId,
      label: candidate.member.name,
      bodyMember: candidate,
    })),
    hasBodyMembers: bodyMembers.length > 0,
    cond,
    condMember,
    condSchema,
    fields,
    condField,
    fieldOptions: fields.map((field) => ({
      value: field.name,
      label: `${field.name} · ${field.type}`,
      field,
    })),
    fieldPlaceholder: condSchema ? view.repeatConditionFieldPlaceholder : view.repeatConditionNoSchemaLabel,
    defaultOperator,
    operatorValue,
    operatorOptions: conditionOperatorOptions(contract, operatorValue),
    repeatUntilExpression,
    iterationInputValue,
    iterationInputOptions,
    selectedIterationInput: iterationInputOptions.find((option) => option.value === iterationInputValue) || null,
  };
}

export function basicMemberStepControlState({ step, flow, members = [], contract, basicView = null, launchView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const sourceMembers = Array.isArray(members) ? members : [];
  const memberById = new Map(sourceMembers.map((member) => [member.id, member]));
  const member = step?.role ? memberById.get(step.role) || null : null;
  const launchState = launchModeControlState(step, contract, launchView);
  const launchSources = collectFlowMemberSteps(flow?.steps || [])
    .filter((candidate) => candidate.id !== step?.id && candidate.role);
  const launchSourceOptions = launchSources.map((source) => {
    const sourceMember = memberById.get(source.role) || null;
    return {
      value: source.id,
      label: `${sourceMember?.name || source.role} · ${source.id}`,
      step: source,
      member: sourceMember,
    };
  });
  const runtimeDefault = { value: "", label: view.memberStepRuntimeDefaultLabel, disabled: false, reason: "" };
  const dispatchValue = typeof step?.dispatchMode === "string" ? step.dispatchMode : "";
  const collectionValue = typeof step?.collection === "string" ? step.collection : "";
  const dependencyValue = typeof step?.dependsMode === "string" ? step.dependsMode : "";
  const outputValue = typeof step?.outputFormat === "string" ? step.outputFormat : "";
  const dispatchOptions = [runtimeDefault, ...dispatchModeOptions(contract, dispatchValue)];
  const collectionOptions = [runtimeDefault, ...collectionPolicyOptions(contract, collectionValue)];
  const dependencyOptions = [runtimeDefault, ...dependencyModeOptions(contract, dependencyValue)];
  const outputOptions = [runtimeDefault, ...outputFormatOptions(contract, outputValue)];
  return {
    member,
    panelTitle: member ? member.name : view.memberStepPanelTitleFallback,
    panelSub: member ? `${member.role} · ${member.model}` : view.memberStepPanelSubFallback,
    memberFieldLabel: view.memberStepMemberLabel,
    memberPlaceholderLabel: view.memberStepMemberPlaceholder,
    memberOptions: sourceMembers.map((candidate) => ({
      value: candidate.id,
      label: `${candidate.name} · ${candidate.role}`,
      member: candidate,
    })),
    launchState,
    launchSources,
    launchSourceOptions,
    firstLaunchSourceId: launchSourceOptions[0]?.value || "",
    instructionLabel: view.memberStepInstructionLabel,
    instructionPlaceholder: view.memberStepInstructionPlaceholder,
    dispatchLabel: view.memberStepDispatchLabel,
    dispatchValue,
    dispatchOptions,
    selectedDispatch: dispatchOptions.find((option) => option.value === dispatchValue) || null,
    collectionLabel: view.memberStepCollectionLabel,
    collectionValue,
    collectionOptions,
    selectedCollection: collectionOptions.find((option) => option.value === collectionValue) || null,
    quorumLabel: view.memberStepQuorumLabel,
    quorumPlaceholder: view.memberStepQuorumPlaceholder,
    timeoutLabel: view.memberStepTimeoutLabel,
    timeoutPlaceholder: view.memberStepRuntimeDefaultLabel,
    dependencyLabel: view.memberStepDependencyLabel,
    dependencyValue,
    dependencyOptions,
    selectedDependency: dependencyOptions.find((option) => option.value === dependencyValue) || null,
    outputFormatLabel: view.memberStepOutputFormatLabel,
    outputValue,
    outputOptions,
    selectedOutput: outputOptions.find((option) => option.value === outputValue) || null,
    showQuorum: collectionValue === "quorum",
    allowedToolsLabel: view.memberStepAllowedToolsLabel,
    allowedToolsEmptyLabel: view.memberStepAllowedToolsEmptyLabel,
    blockedToolsLabel: view.memberStepBlockedToolsLabel,
    blockedToolsEmptyLabel: view.memberStepBlockedToolsEmptyLabel,
    schemaHint: member?.schema
      ? (() => {
        const tools = normalizeStringList(member.tools);
        const toolSummary = tools.join(", ") || view.memberStepSchemaHintEmptyToolsLabel;
        return {
          schema: member.schema,
          tools,
          toolSummary,
          parts: [
            { key: "prefix", text: view.memberStepSchemaHintPrefix },
            { key: "schema", text: member.schema, kind: "code" },
            { key: "tools", text: `${view.memberStepSchemaHintToolsPrefix}${toolSummary}` },
          ],
        };
      })()
      : null,
  };
}

// Control state for the adaptive-layer step: everything the Basic panel and
// the AdaptiveBody block render. Every label/placeholder/prefix comes from
// the server view contract (editor_basic_view adaptive_* keys); prompt and
// objectiveClass values pass through verbatim — never trimmed — so the
// EchoTextArea per-keystroke echo round-trips byte-identical text.
export function adaptiveStepState(studio, step, contract, basicView = null) {
  const view = basicEditorViewState(basicView);
  const members = Array.isArray(studio?.members) ? studio.members : [];
  const schemas = Array.isArray(studio?.schemas) ? studio.schemas : [];
  const defaults = view.adaptiveDefaults && typeof view.adaptiveDefaults === "object"
    ? view.adaptiveDefaults
    : null;
  const defaultLimits = defaults?.limits && typeof defaults.limits === "object" ? defaults.limits : {};
  const stepLimits = step?.limits && typeof step.limits === "object" && !Array.isArray(step.limits)
    ? step.limits
    : {};
  const limitValue = (key) => normalizePositiveInteger(stepLimits[key]) ?? normalizePositiveInteger(defaultLimits[key]);
  const limitRows = view.adaptiveLimitRows.map((row) => ({
    key: row.key,
    tomlKey: row.tomlKey,
    label: row.label,
    primary: Boolean(row.primary),
    value: limitValue(row.key),
  }));
  const flowmasterId = String(step?.flowmasterId || "");
  const selectedFlowmaster = members.find((member) => member?.id === flowmasterId) || null;
  const profileTemplateIds = Array.isArray(step?.profileTemplateIds)
    ? step.profileTemplateIds.map((id) => String(id || "")).filter(Boolean)
    : [];
  const templateMembers = profileTemplateIds
    .map((id) => members.find((member) => member?.id === id) || null)
    .filter(Boolean);
  const targetSurfaces = Array.isArray(step?.targetSurfaces)
    ? step.targetSurfaces.map((surface) => String(surface || "")).filter(Boolean)
    : [];
  const resultSchemaValue = String(step?.resultSchema || "");
  const maxDepth = limitValue("maxDepth");
  const maxMembersPerLayer = limitValue("maxMembersPerLayer");
  return {
    panelIcon: "❖",
    panelTitle: view.adaptivePanelTitle,
    panelSub: view.adaptivePanelSub,
    flowmasterLabel: view.adaptiveFlowmasterLabel,
    flowmasterPlaceholderLabel: view.adaptiveFlowmasterPlaceholderLabel,
    flowmasterId,
    flowmasterOptions: members.map((member) => ({
      value: member.id,
      label: `${member.name || member.role || member.id} · ${member.model || member.role || member.id}`,
      member,
    })),
    selectedFlowmaster,
    promptLabel: view.adaptivePromptLabel,
    promptPlaceholder: view.adaptivePromptPlaceholder,
    promptValue: typeof step?.prompt === "string" ? step.prompt : "",
    objectiveLabel: view.adaptiveObjectiveLabel,
    objectivePlaceholder: view.adaptiveObjectivePlaceholder,
    objectiveValue: typeof step?.objectiveClass === "string" ? step.objectiveClass : "",
    resultSchemaLabel: view.adaptiveResultSchemaLabel,
    resultSchemaPlaceholderLabel: view.adaptiveResultSchemaPlaceholderLabel,
    resultSchemaValue,
    schemaOptions: schemas.map((schema) => ({ value: schema.id, label: schema.id, schema })),
    selectedSchema: schemas.find((schema) => schema?.id === resultSchemaValue) || null,
    profilesTitle: view.adaptiveProfilesTitle,
    profilesHint: view.adaptiveProfilesHint,
    profilesAgentsHint: view.adaptiveProfilesAgentsHint,
    profilesEmptyHint: view.adaptiveProfilesEmptyHint,
    profileTemplateIds,
    hasMembers: members.length > 0,
    templateRows: members.map((member) => ({
      id: member.id,
      label: member.name || member.role || member.id,
      role: member.role || "",
      model: member.model || "",
      on: profileTemplateIds.includes(member.id),
      member,
    })),
    surfacesLabel: view.adaptiveSurfacesLabel,
    targetSurfaces,
    surfaceToggles: view.adaptiveTargetSurfaces.map((surface) => ({
      id: surface,
      label: surface,
      on: targetSurfaces.includes(surface),
    })),
    inlineProfilesLabel: view.adaptiveInlineProfilesLabel,
    allowInlineProfiles: step?.allowInlineProfiles === true,
    limitsTitle: view.adaptiveLimitsTitle,
    limitsAdvancedLabel: view.adaptiveLimitsAdvancedLabel,
    limitRows,
    primaryLimitRows: limitRows.filter((row) => row.primary),
    advancedLimitRows: limitRows.filter((row) => !row.primary),
    tips: view.adaptiveTips,
    stepCardTitle: view.adaptiveStepCardTitle,
    block: {
      headText: `${view.adaptiveBlockHeadPrefix}${maxDepth ?? ""}`,
      flowmasterKicker: view.adaptiveBlockFlowmasterKicker,
      flowmasterTitle: selectedFlowmaster
        ? (selectedFlowmaster.name || selectedFlowmaster.role || selectedFlowmaster.id)
        : view.adaptiveBlockFlowmasterTitleFallback,
      flowmasterDesc: view.adaptiveBlockFlowmasterDesc,
      planConnector: view.adaptiveBlockPlanConnector,
      layerKicker: `${view.adaptiveBlockLayerKickerPrefix}${maxMembersPerLayer ?? ""}${view.adaptiveBlockLayerKickerSuffix}`,
      chips: templateMembers.map((member) => ({
        id: member.id,
        label: member.name || member.role || member.id,
        suffix: view.adaptiveBlockChipSuffix,
      })),
      emptyProfilesLabel: view.adaptiveBlockEmptyProfilesLabel,
      collectConnector: `${view.adaptiveBlockCollectConnectorPrefix}${resultSchemaValue || view.adaptiveBlockCollectFallback}`,
      loopBackText: `${view.adaptiveBlockLoopBackPrefix}${maxDepth ?? ""}${view.adaptiveBlockLoopBackSuffix}`,
    },
  };
}

export function basicRepeatUntilExpression(step, members = [], options = {}) {
  const cond = step?.cond;
  if (!cond || !cond.stepId || !cond.field) return step?.until || "";
  const bodyStep = (Array.isArray(step.steps) ? step.steps : []).find((candidate) => candidate?.id === cond.stepId);
  const member = (Array.isArray(members) ? members : []).find((candidate) => candidate?.id === bodyStep?.role);
  if (!member) return step?.until || "";
  const op = cond.op || cond.operator || options.defaultOperator || "";
  if (!op) return step?.until || "";
  return `${member.name || member.role || member.id}.${cond.field} ${op} ${conditionValueLiteral(cond.val ?? cond.value ?? "")}`;
}
