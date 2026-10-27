// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the graph-editor functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (graphSelectionState,
// graphTemplateInspectorState, graphInstanceControlState, graphGridState,
// the canvas-state family, graphGateControlState, graphBranchConditionRows,
// graphEdgeInspectorState, graphJoinCollectionPatch) raise TS2339 under .ts
// semantics. Source-contract pins this exact text, so suppression must live
// at file level, not in the moved bodies. Resolution/linkage stays guarded
// behaviorally: the projection suite and export-keys test load the bundle
// and exercise these functions, so a missed import or re-export still fails
// the gate as a ReferenceError.
//
// Graph editor control plane for the Flow Editor. Moved verbatim from the
// controller.js graph-editor range: edge condition/kind/fallback patches,
// selection state, template/instance inspectors, grid geometry
// (graphGridState..graphDragCellAt), cell/node/frame/adornment/gate/edge
// canvas states, gate/terminal/edge inspector control states, and the
// gate/terminal/join/fork patch family. graphCanvasViewState is re-homed
// here from the residue's view-state range, and graphConditionRefForEdge/
// graphConditionOptions from the basic-editor range (extraction design S11).
// The four misfiled reconcile fns that sat in this range moved to
// flow/reconcile.ts in S8; GRAPH_NODE_W/H moved to shared/constants.ts in
// S1. graphProjectionEdgeKinds, still residue-bound by range, was seeded
// into its design-destined document/build-projection.ts home (S16) because
// graphEdgeCanvasState needs it and it is facade-internal, out of the lazy
// residue-bridge's reach.
//
// SCC note: editors/graph-editor.ts and editors/basic-editor.ts were
// co-moved in S11 per the extraction design — the inputParamsForStep/
// parseGraphConditionVar imports below are runtime-only cycle edges with
// basic-editor's contract/options imports (no module-init cross-calls).
import {
  conditionOperatorOptions,
  contractDefaultValue,
  contractStringValues,
  graphEdgeKindOptions,
  graphGateKindOptions,
  graphTerminalKindOptions,
} from "../contract/options";
import { graphProjectionEdgeKinds } from "../document/build-projection";
import { editorGraphDraftContract, emptyGraphDraftContract } from "../drafts/mob-settings";
import {
  collectionPolicyAllowed,
  collectionPolicyOptions,
  dispatchModeAllowed,
  dispatchModeOptions,
  normalizedEdgeCondition,
} from "../flow/launch-modes";
import { conditionTextForPath } from "../flow/reconcile";
import {
  basicConditionOperatorPatch,
  basicConditionValuePatch,
  memberRoleAllowed,
} from "../flow/step-tree";
import { GRAPH_NODE_H, GRAPH_NODE_W } from "../shared/constants";
import { normalizeStringList } from "../shared/normalize";
import { graphTemplateViewForState } from "../views/view-config";
import { inputParamsForStep, parseGraphConditionVar } from "./basic-editor";

export function graphCanvasViewState(graphView) {
  const view = graphView && typeof graphView === "object" ? graphView : null;
  return {
    zoomOutTitle: String(view?.zoomOutTitle || ""),
    fitTitle: String(view?.fitTitle || ""),
    zoomInTitle: String(view?.zoomInTitle || ""),
    portDragTitle: String(view?.portDragTitle || ""),
    addNodeSearchIcon: String(view?.addNodeSearchIcon || ""),
    addNodeSearchPlaceholder: String(view?.addNodeSearchPlaceholder || ""),
    addNodeCloseLabel: String(view?.addNodeCloseLabel || ""),
    addNodeCloseTitle: String(view?.addNodeCloseTitle || ""),
    addNodeAgentsLabel: String(view?.addNodeAgentsLabel || ""),
    addNodeControlsLabel: String(view?.addNodeControlsLabel || ""),
    addNodeTerminalsLabel: String(view?.addNodeTerminalsLabel || ""),
    addNodeEmptyPrefix: String(view?.addNodeEmptyPrefix || ""),
    addNodeEmptySuffix: String(view?.addNodeEmptySuffix || ""),
    addNodeJumpLabel: String(view?.addNodeJumpLabel || ""),
    authoringOperationUnavailableError: String(view?.authoringOperationUnavailableError || ""),
    authoringOperationFallbackError: String(view?.authoringOperationFallbackError || ""),
    gatePaletteRows: Array.isArray(view?.gatePaletteRows) ? view.gatePaletteRows : [],
    terminalPaletteRows: Array.isArray(view?.terminalPaletteRows) ? view.terminalPaletteRows : [],
    gateKindLabels: view?.gateKindLabels && typeof view.gateKindLabels === "object" ? view.gateKindLabels : {},
    terminalKindLabels: view?.terminalKindLabels && typeof view.terminalKindLabels === "object" ? view.terminalKindLabels : {},
    frameKindLabels: view?.frameKindLabels && typeof view.frameKindLabels === "object" ? view.frameKindLabels : {},
    edgeKindLabels: view?.edgeKindLabels && typeof view.edgeKindLabels === "object" ? view.edgeKindLabels : {},
    inspectorDeleteLabel: String(view?.inspectorDeleteLabel || ""),
    inspectorLabelTitle: String(view?.inspectorLabelTitle || ""),
    inspectorKindTitle: String(view?.inspectorKindTitle || ""),
    inspectorRuntimeDefaultLabel: String(view?.inspectorRuntimeDefaultLabel || ""),
    instanceEyebrow: String(view?.instanceEyebrow || ""),
    instanceIdLineTemplate: String(view?.instanceIdLineTemplate || ""),
    instanceMemberRoleTemplate: String(view?.instanceMemberRoleTemplate || ""),
    instanceEditMemberLabel: String(view?.instanceEditMemberLabel || ""),
    instanceModelLabel: String(view?.instanceModelLabel || ""),
    instanceSchemaLabel: String(view?.instanceSchemaLabel || ""),
    instanceToolsLabel: String(view?.instanceToolsLabel || ""),
    instanceMemberHint: String(view?.instanceMemberHint || ""),
    instancePositionTitle: String(view?.instancePositionTitle || ""),
    instancePositionStageLabel: String(view?.instancePositionStageLabel || ""),
    instancePositionSlotLabel: String(view?.instancePositionSlotLabel || ""),
    instanceOutputTitleTemplate: String(view?.instanceOutputTitleTemplate || ""),
    instanceOutputRequiredLabel: String(view?.instanceOutputRequiredLabel || ""),
    instanceOutputHint: String(view?.instanceOutputHint || ""),
    instanceOutputOpenMemberLabel: String(view?.instanceOutputOpenMemberLabel || ""),
    gateEyebrowTemplate: String(view?.gateEyebrowTemplate || ""),
    gateIdLineTemplate: String(view?.gateIdLineTemplate || ""),
    gateQuorumIncomingTemplate: String(view?.gateQuorumIncomingTemplate || ""),
    gateMemberOptionTemplate: String(view?.gateMemberOptionTemplate || ""),
    terminalEyebrowTemplate: String(view?.terminalEyebrowTemplate || ""),
    terminalIdLineTemplate: String(view?.terminalIdLineTemplate || ""),
    terminalAuthoringLockedTitle: String(view?.terminalAuthoringLockedTitle || ""),
    terminalAuthoringLockedHint: String(view?.terminalAuthoringLockedHint || ""),
    adaptiveAuthoringLockedTitle: String(view?.adaptiveAuthoringLockedTitle || ""),
    adaptiveAuthoringLockedHint: String(view?.adaptiveAuthoringLockedHint || ""),
    edgeEyebrowTemplate: String(view?.edgeEyebrowTemplate || ""),
    edgeTitleTemplate: String(view?.edgeTitleTemplate || ""),
    edgeIdLineTemplate: String(view?.edgeIdLineTemplate || ""),
    edgeFieldPlaceholder: String(view?.edgeFieldPlaceholder || ""),
    edgeFieldNoSchemaPlaceholder: String(view?.edgeFieldNoSchemaPlaceholder || ""),
    gateCollectionTitle: String(view?.gateCollectionTitle || ""),
    gateJoinMemberLabel: String(view?.gateJoinMemberLabel || ""),
    gateJoinMemberPlaceholder: String(view?.gateJoinMemberPlaceholder || ""),
    gateJoinMemberHint: String(view?.gateJoinMemberHint || ""),
    gateDispatchTitle: String(view?.gateDispatchTitle || ""),
    gateDispatchHint: String(view?.gateDispatchHint || ""),
    gateConditionsTitle: String(view?.gateConditionsTitle || ""),
    gateEmptyBranchHint: String(view?.gateEmptyBranchHint || ""),
    gateWiringTitle: String(view?.gateWiringTitle || ""),
    gateIncomingLabel: String(view?.gateIncomingLabel || ""),
    gateOutgoingLabel: String(view?.gateOutgoingLabel || ""),
    branchConditionModeConditionLabel: String(view?.branchConditionModeConditionLabel || ""),
    branchConditionModeFallbackLabel: String(view?.branchConditionModeFallbackLabel || ""),
    branchConditionTargetPrefix: String(view?.branchConditionTargetPrefix || ""),
    graphConditionTargetMissingLabel: String(view?.graphConditionTargetMissingLabel || ""),
    graphConditionOwnerOptionTemplate: String(view?.graphConditionOwnerOptionTemplate || ""),
    graphConditionFieldOptionTemplate: String(view?.graphConditionFieldOptionTemplate || ""),
    graphInputParamSourceLabel: String(view?.graphInputParamSourceLabel || ""),
    sourceFileLabel: String(view?.sourceFileLabel || ""),
    sourceFileAriaLabel: String(view?.sourceFileAriaLabel || ""),
    sourceFileGlyph: String(view?.sourceFileGlyph || ""),
    sourceFileRoleLabel: String(view?.sourceFileRoleLabel || ""),
    sourceFileNodeId: String(view?.sourceFileNodeId || ""),
    sourceFileNodeKind: String(view?.sourceFileNodeKind || ""),
    sourceFileNodeColOffset: Number(view?.sourceFileNodeColOffset || 0),
    sourceFileNodeRowOffset: Number(view?.sourceFileNodeRowOffset || 0),
    sourceFileActivationHash: String(view?.sourceFileActivationHash || ""),
    sourceFileActivationSelector: String(view?.sourceFileActivationSelector || ""),
    branchConditionFieldPlaceholder: String(view?.branchConditionFieldPlaceholder || ""),
    branchConditionNoOptionsHint: String(view?.branchConditionNoOptionsHint || ""),
    edgeConditionTitle: String(view?.edgeConditionTitle || ""),
    edgeNoConditionOptionsHint: String(view?.edgeNoConditionOptionsHint || ""),
    edgeOwnerPlaceholder: String(view?.edgeOwnerPlaceholder || ""),
    edgeFromTitle: String(view?.edgeFromTitle || ""),
    edgeToTitle: String(view?.edgeToTitle || ""),
    edgeRowInstanceLabel: String(view?.edgeRowInstanceLabel || ""),
    edgeRowMemberLabel: String(view?.edgeRowMemberLabel || ""),
    edgeRowSchemaLabel: String(view?.edgeRowSchemaLabel || ""),
    edgeRowMissingValue: String(view?.edgeRowMissingValue || ""),
    edgeTerminalMemberValue: String(view?.edgeTerminalMemberValue || ""),
  };
}

export function graphConditionRefForEdge(edge) {
  const condition = normalizedEdgeCondition(edge);
  return parseGraphConditionVar(condition?.path || "");
}

export function graphConditionOptions({ instances, members, schemas, edge, flow, graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const graphInstances = Array.isArray(instances) ? instances : [];
  const byId = new Map(graphInstances.map((inst) => [inst.id, inst]));
  const memberById = new Map((Array.isArray(members) ? members : []).map((member) => [member.id, member]));
  const schemaById = new Map((Array.isArray(schemas) ? schemas : []).map((schema) => [schema.id, schema]));
  const from = byId.get(edge?.from);
  const to = byId.get(edge?.to);
  const fromCol = Number(from?.col ?? 0);
  const toCol = Number(to?.col ?? fromCol);
  const limitCol = Math.max(fromCol, toCol);
  const condRef = graphConditionRefForEdge(edge);
  const params = inputParamsForStep((flow?.steps || []).find((step) => step.type === "input"));
  const paramFields = params.length ? params : (condRef.namespace === "params" && condRef.field
    ? [{ id: condRef.field, name: condRef.field, type: "string" }]
    : []);
  const options = graphInstances
    .filter((inst) => inst.memberId && !inst.isGate && !inst.isTerminal)
    .filter((inst) => inst.id !== to?.id)
    .filter((inst) => inst.id === from?.id || Number(inst.col ?? 0) <= limitCol)
    .map((inst) => {
      const member = memberById.get(inst.memberId);
      const schema = member?.schema ? schemaById.get(member.schema) || null : null;
      return { inst, member, schema, fields: schema?.fields || [] };
    })
    .filter((option) => option.member && option.fields.length > 0)
    .sort((a, b) => (Number(a.inst.col || 0) - Number(b.inst.col || 0)) || (Number(a.inst.row || 0) - Number(b.inst.row || 0)));
  if (paramFields.length) {
    options.unshift({
      inst: { id: "params" },
      member: { name: view.graphInputParamSourceLabel },
      schema: { id: "params" },
      fields: paramFields,
      isParams: true,
    });
  }
  return options;
}

export function graphEdgeConditionPatch(edge, patch = {}, options = {}) {
  const previous = normalizedEdgeCondition(edge);
  const previousText = previous?.path ? conditionTextForPath(previous.path, previous) : "";
  const currentCond = edge?.cond && typeof edge.cond === "object" && !Array.isArray(edge.cond)
    ? edge.cond
    : {};
  const merged = { ...currentCond, ...patch };
  const normalized = normalizedEdgeCondition({ cond: merged });
  if (!normalized?.path) {
    return { cond: { ...merged, var: "" }, label: "" };
  }
  const cond = {
    var: normalized.path,
    op: normalized.op || options.defaultOperator || "",
    val: normalized.val ?? "",
  };
  const currentLabel = String(edge?.label || "");
  const shouldReplaceLabel = !!options.forceLabel
    || !currentLabel
    || (!!previousText && currentLabel === previousText);
  return {
    cond,
    label: shouldReplaceLabel ? conditionTextForPath(cond.var, cond) : edge.label,
  };
}

export function graphEdgeConditionOperatorPatch(edge, rawOperator, options = {}) {
  const patch = basicConditionOperatorPatch(rawOperator, options.contract);
  if (!("op" in patch)) return {};
  return graphEdgeConditionPatch(edge, patch, options);
}

export function graphEdgeConditionValuePatch(edge, rawValue, options = {}) {
  return graphEdgeConditionPatch(edge, basicConditionValuePatch(rawValue), options);
}

export function graphConditionPathForOption(option, field) {
  const name = String(field || "").trim();
  if (!option || !name) return "";
  const instanceId = String(option?.inst?.id || "").trim();
  if (!instanceId) return "";
  if (option.isParams || instanceId === "params") return `params.${name}`;
  return `steps.${instanceId}.${name}`;
}

export function graphFirstConditionPatch(edge, conditionOptions = [], options = {}) {
  const rows = Array.isArray(conditionOptions) ? conditionOptions : [];
  const condRef = graphConditionRefForEdge(edge);
  const preferredId = String(options.instanceId || condRef.instanceId || "").trim();
  const owner = rows.find((option) => option?.inst?.id === preferredId) || rows[0];
  const field = options.field !== undefined
    ? String(options.field || "").trim()
    : String(condRef.field || owner?.fields?.[0]?.name || "").trim();
  const path = graphConditionPathForOption(owner, field);
  return path
    ? { var: path, op: edge?.cond?.op || options.defaultOperator || "", val: edge?.cond?.val ?? "" }
    : { var: "" };
}

export function graphConditionEdgeKindForPatch(options = {}) {
  return String(options.conditionKind || contractDefaultValue(options.contract, "graph_condition_edge_kind")).trim();
}

export function graphEdgeConditionOwnerPatch(edge, conditionOptions = [], instanceId, options = {}) {
  const rows = Array.isArray(conditionOptions) ? conditionOptions : [];
  const id = String(instanceId || "").trim();
  if (id && !rows.some((option) => String(option?.inst?.id || "").trim() === id)) return {};
  const owner = rows.find((option) => option?.inst?.id === instanceId);
  const firstField = String(owner?.fields?.[0]?.name || "").trim();
  const conditionPatch = graphFirstConditionPatch(edge, rows, {
    instanceId,
    field: firstField,
    defaultOperator: options.defaultOperator,
  });
  const patch = graphEdgeConditionPatch(edge, conditionPatch, {
    defaultOperator: options.defaultOperator,
    forceLabel: options.forceLabel,
  });
  return options.includeKind ? { kind: graphConditionEdgeKindForPatch(options), ...patch } : patch;
}

export function graphEdgeConditionFieldPatch(edge, conditionOptions = [], field, options = {}) {
  const rows = Array.isArray(conditionOptions) ? conditionOptions : [];
  const condRef = graphConditionRefForEdge(edge);
  const owner = rows.find((option) => option?.inst?.id === condRef.instanceId) || rows[0];
  const fieldName = String(field || "").trim();
  if (fieldName && !((owner?.fields || []).some((candidate) => String(candidate?.name || "").trim() === fieldName))) return {};
  const conditionPatch = graphFirstConditionPatch(edge, rows, {
    instanceId: owner?.inst?.id || "",
    field,
    defaultOperator: options.defaultOperator,
  });
  const patch = graphEdgeConditionPatch(edge, conditionPatch, {
    defaultOperator: options.defaultOperator,
    forceLabel: options.forceLabel,
  });
  return options.includeKind ? { kind: graphConditionEdgeKindForPatch(options), ...patch } : patch;
}

export function graphEdgeKindPatch(edge, nextKind, options = {}) {
  const kind = String(nextKind || "").trim();
  const conditionKind = graphConditionEdgeKindForPatch(options);
  if (kind !== conditionKind) {
    const previous = normalizedEdgeCondition(edge);
    const previousText = previous?.path ? conditionTextForPath(previous.path, previous) : "";
    const currentLabel = String(edge?.label || "");
    return {
      kind,
      cond: null,
      label: previousText && currentLabel === previousText ? "" : currentLabel,
    };
  }
  return {
    kind: conditionKind,
    ...graphEdgeConditionPatch(edge, options.conditionPatch || {}, {
      defaultOperator: options.defaultOperator,
      forceLabel: options.forceLabel,
    }),
  };
}

export function graphEdgeFallbackPatch(edge, contract) {
  const kind = contractDefaultValue(contract, "graph_edge_kind");
  const draft = editorGraphDraftContract(contract);
  if (!kind || !draft) return null;
  return { kind, label: draft.fallbackEdgeLabel, cond: null };
}

export function graphBranchConditionModePatch(edge, mode, options = {}) {
  const value = String(mode || "").trim();
  if (value === "fallback") return graphEdgeFallbackPatch(edge, options.contract);
  const conditionKind = graphConditionEdgeKindForPatch(options);
  if (value !== conditionKind) return {};
  return graphEdgeConditionOwnerPatch(edge, options.conditionOptions, options.firstOwnerId, {
    defaultOperator: options.defaultOperator,
    forceLabel: true,
    includeKind: true,
    contract: options.contract,
    conditionKind,
  });
}

export function graphSelectionState({ selection = {}, instances = [], edges = [] } = {}) {
  const kind = String(selection?.kind || "");
  if (kind === "instance") {
    const instance = (Array.isArray(instances) ? instances : []).find((candidate) => candidate.id === selection.id) || null;
    return { kind, instance, edge: null, missing: !instance };
  }
  if (kind === "edge") {
    const edge = (Array.isArray(edges) ? edges : []).find((candidate) => candidate.id === selection.id) || null;
    return { kind, instance: null, edge, missing: !edge };
  }
  return { kind: "", instance: null, edge: null, missing: false };
}

export function graphSelectionProjection(kind, id) {
  const selectionKind = String(kind || "").trim();
  const selectionId = String(id || "").trim();
  if (!selectionId || (selectionKind !== "instance" && selectionKind !== "edge")) return { kind: null, id: null };
  return { kind: selectionKind, id: selectionId };
}

export function graphTemplateInspectorState({ studio = {}, template = null, templateSeed = null, templateView = null } = {}) {
  const seed = templateSeed && typeof templateSeed === "object" ? templateSeed : {};
  const view = graphTemplateViewForState(templateView);
  const members = Array.isArray(studio.members) ? studio.members : [];
  const instances = Array.isArray(studio.instances) ? studio.instances : [];
  const edges = Array.isArray(studio.edges) ? studio.edges : [];
  const frames = Array.isArray(studio.frames) ? studio.frames : [];
  const triggerLabel = template?.trigger || (Array.isArray(seed.triggers?.labels) ? seed.triggers.labels.join(", ") : "");
  const labels = triggerLabel ? [triggerLabel] : [];
  const placedMembers = new Set(instances.filter((instance) => instance?.memberId).map((instance) => instance.memberId)).size;
  const memberSummary = view.summaryMembersValueTemplate
    .replaceAll("{placed}", String(placedMembers))
    .replaceAll("{total}", String(members.length));
  return {
    name: template?.name || seed.name || "",
    repo: template?.repo || seed.repo || "",
    version: template?.version || seed.version || "",
    templateEyebrow: view.templateEyebrow,
    summaryTitle: view.summaryTitle,
    triggersTitle: view.triggersTitle,
    quickStartTitle: view.quickStartTitle,
    quickStartRows: view.quickStartRows,
    triggers: {
      labels,
      default: !!template?.defaultTrigger,
    },
    triggerRows: [
      { key: "labels", label: view.triggerLabelsLabel, value: labels.join(", ") },
      {
        key: "default",
        label: view.triggerDefaultLabel,
        value: template?.defaultTrigger ? view.defaultYesLabel : view.defaultNoLabel,
      },
    ],
    summaryRows: [
      { key: "members", label: view.summaryMembersLabel, value: memberSummary },
      { key: "instances", label: view.summaryInstancesLabel, value: instances.filter((instance) => !instance?.isTerminal).length },
      { key: "terminals", label: view.summaryTerminalsLabel, value: instances.filter((instance) => instance?.isTerminal).length },
      { key: "edges", label: view.summaryEdgesLabel, value: edges.length },
      { key: "frames", label: view.summaryFramesLabel, value: frames.length },
    ],
  };
}

export function graphInstanceControlState({ inst, instances = [], members = [], schemas = [], graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const sourceMembers = Array.isArray(members) ? members : [];
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const member = inst?.memberId
    ? sourceMembers.find((candidate) => candidate.id === inst.memberId) || null
    : null;
  const id = String(inst?.id || "");
  const col = Number.isFinite(Number(inst?.col)) ? Number(inst.col) + 1 : 1;
  const row = Number.isFinite(Number(inst?.row)) ? Number(inst.row) + 1 : 1;
  const outputSchema = member?.schema
    ? (Array.isArray(schemas) ? schemas : []).find((schema) => schema.id === member.schema) || null
    : null;
  const outputFields = Array.isArray(outputSchema?.fields) ? outputSchema.fields : [];
  const outputFieldRows = outputFields.map((field) => ({
    id: field.id,
    name: field.name,
    type: field.type,
    required: !!field.required,
    requiredLabel: field.required ? view.instanceOutputRequiredLabel : "",
  }));
  const tools = normalizeStringList(member?.tools);
  const memberToolSummary = tools.length
    ? `${tools.length} · ${tools.slice(0, 3).join(", ")}${tools.length > 3 ? "…" : ""}`
    : "0";
  const forkSourceOptions = sourceInstances
    .filter((candidate) => !candidate?.isTerminal && candidate.id !== inst?.id)
    .map((candidate) => {
      const sourceMember = candidate?.memberId
        ? sourceMembers.find((memberCandidate) => memberCandidate.id === candidate.memberId) || null
        : null;
      return {
        value: candidate.id,
        label: `${sourceMember?.name || candidate.id} · ${candidate.id}`,
        instance: candidate,
        member: sourceMember,
      };
    });
  return {
    member,
    memberId: member?.id || "",
    eyebrow: view.instanceEyebrow,
    title: member ? member.name : view.edgeRowMissingValue,
    idLine: graphTemplateText(view.instanceIdLineTemplate, { id, col, row }),
    deleteLabel: view.inspectorDeleteLabel,
    memberTitle: member ? member.name : view.edgeRowMissingValue,
    memberRoleLabel: member ? graphTemplateText(view.instanceMemberRoleTemplate, { role: member.role || "" }) : "",
    editMemberLabel: view.instanceEditMemberLabel,
    memberName: member?.name || "",
    memberSchemaLabel: member?.schema || view.edgeRowMissingValue,
    memberToolSummary,
    memberSummaryRows: [
      { key: "model", label: view.instanceModelLabel, value: member?.model || view.edgeRowMissingValue },
      { key: "schema", label: view.instanceSchemaLabel, value: member?.schema || view.edgeRowMissingValue },
      { key: "tools", label: view.instanceToolsLabel, value: memberToolSummary },
    ],
    memberHint: view.instanceMemberHint,
    positionTitle: view.instancePositionTitle,
    positionRows: [
      { key: "stage", label: view.instancePositionStageLabel, value: col },
      { key: "slot", label: view.instancePositionSlotLabel, value: row },
    ],
    outputSchema,
    outputFields,
    outputTitle: graphTemplateText(view.instanceOutputTitleTemplate, { schema: member?.schema || view.edgeRowMissingValue }),
    outputFieldRows,
    outputHint: view.instanceOutputHint,
    outputOpenMemberLabel: view.instanceOutputOpenMemberLabel,
    adaptiveLockedTitle: view.adaptiveAuthoringLockedTitle,
    adaptiveLockedHint: view.adaptiveAuthoringLockedHint,
    forkSourceOptions,
    firstForkSourceId: forkSourceOptions[0]?.value || "",
  };
}

export function graphTemplateText(template, values = {}) {
  let out = String(template || "");
  for (const [key, value] of Object.entries(values || {})) {
    out = out.replaceAll(`{${key}}`, String(value ?? ""));
  }
  return out;
}

export function graphToolTagClass(toolId, toolCatalog = []) {
  const id = String(toolId || "");
  const tool = (Array.isArray(toolCatalog) ? toolCatalog : [])
    .find((candidate) => String(candidate?.id || "") === id) || null;
  const tagClass = String(tool?.tagClass || tool?.tag_class || tool?.raw?.tag_class || "").trim();
  return tagClass ? ` ${tagClass}` : "";
}

export function graphGridState({ instances = [], gridBase = {} } = {}) {
  const baseCols = Math.max(1, Number(gridBase?.cols || 1));
  const baseRows = Math.max(1, Number(gridBase?.rows || 1));
  let maxCol = baseCols - 1;
  let maxRow = baseRows - 1;
  for (const instance of Array.isArray(instances) ? instances : []) {
    const col = Number(instance?.col);
    const row = Number(instance?.row);
    if (Number.isFinite(col) && col > maxCol) maxCol = col;
    if (Number.isFinite(row) && row > maxRow) maxRow = row;
  }
  const grid = {
    ...gridBase,
    cols: maxCol + 2,
    rows: maxRow + 2,
  };
  const totalW = Number(grid.padX || 0) * 2 +
    Number(grid.cols || 0) * Number(grid.cellW || 0) +
    Math.max(0, Number(grid.cols || 0) - 1) * Number(grid.gapX || 0);
  const totalH = Number(grid.padY || 0) * 2 +
    Number(grid.rows || 0) * Number(grid.cellH || 0) +
    Math.max(0, Number(grid.rows || 0) - 1) * Number(grid.gapY || 0);
  return { grid, totalW, totalH };
}

export function graphCellXY(grid, col, row) {
  return {
    x: Number(grid?.padX || 0) + Number(col || 0) * (Number(grid?.cellW || 0) + Number(grid?.gapX || 0)),
    y: Number(grid?.padY || 0) + Number(row || 0) * (Number(grid?.cellH || 0) + Number(grid?.gapY || 0)),
  };
}

export function graphNodeBox(grid, inst) {
  const { x, y } = graphCellXY(grid, inst?.col, inst?.row);
  if (inst?.isSourceFile) {
    const sw = 210;
    const sh = 58;
    return {
      x: x + (Number(grid?.cellW || 0) - sw) / 2,
      y: y + (Number(grid?.cellH || 0) - sh) / 2,
      w: sw,
      h: sh,
    };
  }
  if (inst?.isGate) {
    const gw = 156;
    const gh = 56;
    return {
      x: x + (Number(grid?.cellW || 0) - gw) / 2,
      y: y + (Number(grid?.cellH || 0) - gh) / 2,
      w: gw,
      h: gh,
    };
  }
  return {
    x: x + (Number(grid?.cellW || 0) - GRAPH_NODE_W) / 2,
    y: y + (Number(grid?.cellH || 0) - GRAPH_NODE_H) / 2,
    w: GRAPH_NODE_W,
    h: GRAPH_NODE_H,
  };
}

export function graphPortOut(grid, inst) {
  const box = graphNodeBox(grid, inst);
  return { x: box.x + box.w, y: box.y + box.h / 2 };
}

export function graphPortIn(grid, inst) {
  const box = graphNodeBox(grid, inst);
  return { x: box.x, y: box.y + box.h / 2 };
}

export function graphEdgePath(a, b) {
  if (b.x < a.x - 20) {
    const dropY = Math.max(a.y, b.y) + 90;
    const dx = 60;
    return `M ${a.x} ${a.y} C ${a.x + dx} ${a.y}, ${a.x + dx} ${dropY}, ${a.x} ${dropY} L ${b.x} ${dropY} C ${b.x - dx} ${dropY}, ${b.x - dx} ${b.y}, ${b.x} ${b.y}`;
  }
  const dx = Math.max(40, (b.x - a.x) * 0.5);
  return `M ${a.x} ${a.y} C ${a.x + dx} ${a.y}, ${b.x - dx} ${b.y}, ${b.x} ${b.y}`;
}

export function graphEdgeMidpoint(a, b) {
  if (b.x < a.x - 20) return { x: (a.x + b.x) / 2, y: Math.max(a.y, b.y) + 90 };
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 - 6 };
}

export function graphCellAt(grid, x, y) {
  const col = Math.floor((Number(x || 0) - Number(grid?.padX || 0) + Number(grid?.gapX || 0) / 2) / (Number(grid?.cellW || 0) + Number(grid?.gapX || 0)));
  const row = Math.floor((Number(y || 0) - Number(grid?.padY || 0) + Number(grid?.gapY || 0) / 2) / (Number(grid?.cellH || 0) + Number(grid?.gapY || 0)));
  if (col < 0 || col >= Number(grid?.cols || 0) || row < 0 || row >= Number(grid?.rows || 0)) return null;
  return { col, row };
}

export function graphDragCellAt(grid, world, drag) {
  const cx = Number(world?.x || 0) - Number(drag?.dx || 0) + GRAPH_NODE_W / 2;
  const cy = Number(world?.y || 0) - Number(drag?.dy || 0) + GRAPH_NODE_H / 2;
  return graphCellAt(grid, cx, cy);
}

export function graphCellCanvasRows({ grid, instances = [], hoverCell = null } = {}) {
  const occupied = new Set();
  for (const instance of Array.isArray(instances) ? instances : []) {
    occupied.add(`${instance?.col}:${instance?.row}`);
  }
  const cols = Math.max(0, Number(grid?.cols || 0));
  const rows = Math.max(0, Number(grid?.rows || 0));
  const out = [];
  for (let col = 0; col < cols; col++) {
    for (let row = 0; row < rows; row++) {
      const cellOccupied = occupied.has(`${col}:${row}`);
      const hovered = Number(hoverCell?.col) === col && Number(hoverCell?.row) === row;
      const { x, y } = graphCellXY(grid, col, row);
      out.push({
        key: `cell-${col}-${row}`,
        col,
        row,
        occupied: cellOccupied,
        addVisible: !cellOccupied,
        className: "cell" + (cellOccupied ? " is-occupied" : "") + (hovered ? " is-hover" : ""),
        style: { left: x, top: y, width: Number(grid?.cellW || 0), height: Number(grid?.cellH || 0) },
      });
    }
  }
  return out;
}

export function graphGridHeaderCanvasRows({ grid } = {}) {
  const cols = Math.max(0, Number(grid?.cols || 0));
  const rows = Math.max(0, Number(grid?.rows || 0));
  const columns = [];
  const rowHeaders = [];
  for (let col = 0; col < cols; col++) {
    const { x } = graphCellXY(grid, col, 0);
    columns.push({
      key: `col-${col}`,
      label: String(col + 1).padStart(2, "0"),
      className: "grid-head grid-head--col",
      style: { left: x, top: 28, width: Number(grid?.cellW || 0) },
    });
  }
  for (let row = 0; row < rows; row++) {
    const { y } = graphCellXY(grid, 0, row);
    rowHeaders.push({
      key: `row-${row}`,
      label: String.fromCharCode(65 + row),
      className: "grid-head grid-head--row",
      style: { left: 14, top: y + Number(grid?.cellH || 0) / 2 - 8 },
    });
  }
  return { columns, rows: rowHeaders };
}

export function graphNodeCanvasState({ inst, members = [], density = "", graphView = null, toolCatalog = [] } = {}) {
  const view = graphCanvasViewState(graphView);
  const isCompact = density === "compact";
  if (inst?.isTerminal) {
    return {
      hidden: false,
      isTerminal: true,
      isSourceFile: false,
      dataKind: inst.kind,
      role: undefined,
      tabIndex: undefined,
      ariaLabel: undefined,
      sourceGlyph: "",
      sourceActivationHash: "",
      sourceActivationSelector: "",
      roleLabel: `terminal · ${inst.kind}`,
      title: inst.label,
      subtitle: inst.kind,
    };
  }
  const member = inst?.memberId
    ? (Array.isArray(members) ? members : []).find((candidate) => candidate.id === inst.memberId) || null
    : null;
  if (!member) return { hidden: true, isTerminal: false, isCompact };
  const tools = normalizeStringList(member.tools);
  const visibleTools = tools.slice(0, isCompact ? 3 : 6);
  return {
    hidden: false,
    isTerminal: false,
    isCompact,
    roleLabel: member.role,
    launchLabel: inst?.launchMode?.kind?.toLowerCase() || "—",
    title: member.name,
    subtitle: member.model,
    toolRows: visibleTools.map((tool) => ({
      id: tool,
      className: "tag" + graphToolTagClass(tool, toolCatalog),
    })),
    overflowLabel: tools.length > visibleTools.length ? `+${tools.length - visibleTools.length}` : "",
  };
}

export function graphFrameCanvasState({ frame, grid } = {}) {
  const cell = (col, row) => ({
    x: Number(grid?.padX || 0) + Number(col || 0) * (Number(grid?.cellW || 0) + Number(grid?.gapX || 0)),
    y: Number(grid?.padY || 0) + Number(row || 0) * (Number(grid?.cellH || 0) + Number(grid?.gapY || 0)),
  });
  const rows = Math.max(1, Number(grid?.rows || 1));
  const cellW = Number(grid?.cellW || 0);
  const cellH = Number(grid?.cellH || 0);
  const startCol = Number.isFinite(Number(frame?.colStart)) ? Number(frame.colStart) : 0;
  const endCol = Number.isFinite(Number(frame?.colEnd)) ? Number(frame.colEnd) : startCol;
  const start = cell(startCol, 0);
  const end = cell(endCol, rows - 1);
  const x = start.x - 14;
  const y = start.y - 18;
  const width = (end.x + cellW) - x + 14;
  const height = (end.y + cellH) - y + 18;
  return {
    id: String(frame?.id || ""),
    label: String(frame?.label || ""),
    frameStyle: { left: x, top: y, width, height },
    labelStyle: { left: x + 12, top: y - 10 },
  };
}

export function graphSourceFileAdornment({ instances = [], graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  if (!view.sourceFileNodeId || !view.sourceFileNodeKind || !view.sourceFileLabel) return null;
  const sourceInstances = graphCanvasInstances({ instances, graphView });
  const positioned = sourceInstances
    .filter((instance) => Number.isFinite(Number(instance?.col)) && Number.isFinite(Number(instance?.row)));
  const minCol = positioned.length
    ? Math.min(...positioned.map((instance) => Number(instance.col)))
    : 0;
  const minRow = positioned.length
    ? Math.min(...positioned.map((instance) => Number(instance.row)))
    : 0;
  return {
    id: view.sourceFileNodeId,
    isSourceFile: true,
    isGraphAdornment: true,
    adornmentKind: "source_file",
    kind: view.sourceFileNodeKind,
    label: view.sourceFileLabel,
    col: minCol + view.sourceFileNodeColOffset,
    row: minRow + view.sourceFileNodeRowOffset,
  };
}

export function graphCanvasInstances({ instances = [], graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  return (Array.isArray(instances) ? instances : [])
    .filter((instance) => {
      if (!instance || typeof instance !== "object") return false;
      if (instance.isGraphAdornment || instance.isSourceFile) return false;
      return String(instance.id || "") !== view.sourceFileNodeId;
    });
}

export function graphCanvasAdornments({ instances = [], graphView = null } = {}) {
  const sourceFileAdornment = graphSourceFileAdornment({ instances, graphView });
  return sourceFileAdornment ? [sourceFileAdornment] : [];
}

export function graphSourceFileAdornmentCanvasState({ adornment = null, graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  return {
    hidden: !adornment,
    isSourceFile: true,
    role: "button",
    tabIndex: 0,
    dataKind: String(adornment?.kind || view.sourceFileNodeKind || ""),
    ariaLabel: view.sourceFileAriaLabel,
    sourceGlyph: view.sourceFileGlyph,
    sourceActivationHash: view.sourceFileActivationHash,
    sourceActivationSelector: view.sourceFileActivationSelector,
    roleLabel: view.sourceFileRoleLabel,
    title: String(adornment?.label || view.sourceFileLabel || ""),
  };
}

export function graphGateCanvasState({ inst, edges = [], contract = null, graphView = null } = {}) {
  const gateKind = String(inst?.gateKind || "");
  const draft = editorGraphDraftContract(contract) || emptyGraphDraftContract();
  const view = graphCanvasViewState(graphView);
  const glyph = view.gatePaletteRows.find((row) => row.id === gateKind)?.glyph || "";
  let sublabel = inst?.label || gateKind;
  if (gateKind === "join" && inst?.collection === "quorum" && inst?.quorum) {
    const incoming = (Array.isArray(edges) ? edges : []).filter((edge) => edge.to === inst?.id).length;
    sublabel = `${draft.joinQuorumLabelPrefix}${inst.quorum.n}/${incoming || inst.quorum.m}`;
  } else if (gateKind === "join" && inst?.collection) {
    sublabel = `${draft.joinLabelPrefix}${inst.collection}`;
  }
  return { glyph, sublabel, gateKind };
}

export function graphEdgeCanvasState({ edge, to, active = false, selected = false, edgeStyle = "", contract = null, graphView = null } = {}) {
  const kind = String(edge?.kind || "next").trim();
  const terminalTarget = !!to?.isTerminal;
  const view = graphCanvasViewState(graphView);
  const labelText = String(edge?.label || view.edgeKindLabels[kind] || "");
  const edgeKinds = graphProjectionEdgeKinds(contract);
  const isCondition = kind === edgeKinds.conditionKind;
  const isFanout = kind === edgeKinds.fanoutKind;
  const mode = edgeStyle === "icons" ? "icons" : edgeStyle === "colored" ? "colored" : "text";
  return {
    kind,
    mode,
    labelText,
    labelWidth: labelText.length * 6 + 12,
    iconGlyph: isCondition ? "?" : isFanout ? "‖" : terminalTarget ? "■" : "→",
    labelFill: isCondition ? "var(--danger)" : isFanout ? "var(--accent)" : terminalTarget ? "var(--muted)" : "var(--ok)",
    iconLabelClass: "edge-label" + (isCondition ? " is-cond" : ""),
    textLabelClass: "edge-label" + (isCondition ? " is-cond" : "") + (active ? " is-active" : "") + (selected ? " is-selected" : ""),
    lineClass: "edge-line" +
      (isCondition ? " is-cond" : "") +
      (isFanout ? " is-fanout" : "") +
      (terminalTarget ? " is-term" : "") +
      (active ? " is-active" : "") +
      (selected ? " is-selected" : ""),
    markerEnd: selected || active ? "url(#arr-acc)" :
      isCondition ? "url(#arr-red)" :
        isFanout ? "url(#arr-acc)" :
          terminalTarget ? "url(#arr-dim)" :
            "url(#arr)",
  };
}

export function graphGateControlState(inst, { edges, members, contract, graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const incoming = (edges || []).filter((edge) => edge.to === inst?.id);
  const outgoing = (edges || []).filter((edge) => edge.from === inst?.id);
  const defaultGateKind = contractDefaultValue(contract, "graph_gate_kind");
  const gateKind = String(inst?.gateKind || defaultGateKind || "").trim();
  const gateKindOptions = graphGateKindOptions(contract, gateKind, graphView);
  const collection = String(inst?.collection || (inst?.quorum?.n ? "quorum" : "")).trim();
  const collectionOptions = [
    { value: "", label: view.inspectorRuntimeDefaultLabel, disabled: false, reason: "" },
    ...collectionPolicyOptions(contract, collection),
  ];
  const dispatch = String(inst?.dispatch || inst?.dispatchMode || "").trim();
  const dispatchOptions = [
    { value: "", label: view.inspectorRuntimeDefaultLabel, disabled: false, reason: "" },
    ...dispatchModeOptions(contract, dispatch),
  ];
  const col = Number(inst?.col ?? 0);
  const row = Number(inst?.row ?? 0);
  return {
    incoming,
    outgoing,
    eyebrow: graphTemplateText(view.gateEyebrowTemplate, { kind: gateKind }),
    title: String(inst?.label || ""),
    idLine: graphTemplateText(view.gateIdLineTemplate, { id: inst?.id || "", col: col + 1, row: row + 1 }),
    deleteLabel: view.inspectorDeleteLabel,
    labelTitle: view.inspectorLabelTitle,
    kindTitle: view.inspectorKindTitle,
    gateKind,
    gateKindOptions,
    selectedGateKind: gateKindOptions.find((option) => option.value === gateKind),
    collectionTitle: view.gateCollectionTitle,
    collection,
    collectionOptions,
    selectedCollection: collectionOptions.find((option) => option.value === collection),
    quorumIncomingLabel: graphTemplateText(view.gateQuorumIncomingTemplate, { count: incoming.length }),
    joinMemberLabel: view.gateJoinMemberLabel,
    joinMemberPlaceholderOption: { value: "", label: view.gateJoinMemberPlaceholder },
    joinMemberHint: view.gateJoinMemberHint,
    dispatchTitle: view.gateDispatchTitle,
    dispatch,
    dispatchOptions,
    selectedDispatch: dispatchOptions.find((option) => option.value === dispatch),
    dispatchHint: view.gateDispatchHint,
    conditionsTitle: view.gateConditionsTitle,
    emptyBranchHint: view.gateEmptyBranchHint,
    wiringTitle: view.gateWiringTitle,
    incomingLabel: view.gateIncomingLabel,
    outgoingLabel: view.gateOutgoingLabel,
    firstMemberId: (members || []).find((member) => member?.id)?.id || "",
    memberOptions: (Array.isArray(members) ? members : [])
      .filter((member) => member?.id)
      .map((member) => ({
        value: member.id,
        label: graphTemplateText(view.gateMemberOptionTemplate, {
          id: member.id,
          name: member.name || member.id,
          role: member.role || "profile",
        }),
        member,
      })),
    incomingCount: incoming.length,
    outgoingCount: outgoing.length,
  };
}

export function graphBranchConditionRows({ inst, edges = [], instances = [], members = [], schemas = [], flow, contract, graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const sourceEdges = Array.isArray(edges) ? edges : [];
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const sourceMembers = Array.isArray(members) ? members : [];
  const instanceById = new Map(sourceInstances.map((candidate) => [candidate.id, candidate]));
  const memberById = new Map(sourceMembers.map((candidate) => [candidate.id, candidate]));
  const defaultOperator = contractDefaultValue(contract, "condition_operator");
  const conditionKind = contractDefaultValue(contract, "graph_condition_edge_kind");
  return sourceEdges
    .filter((edge) => edge?.from === inst?.id)
    .map((edge) => {
      const target = instanceById.get(edge.to) || null;
      const targetMember = target?.memberId ? memberById.get(target.memberId) || null : null;
      const condRef = graphConditionRefForEdge(edge);
      const conditionOptions = graphConditionOptions({
        instances: sourceInstances,
        members: sourceMembers,
        schemas,
        edge,
        flow,
        graphView,
      });
      const condOwner = conditionOptions.find((option) => option.inst.id === condRef.instanceId) || null;
      const fields = condOwner?.fields || conditionOptions[0]?.fields || [];
      const condField = fields.find((field) => field.name === condRef.field) || null;
      const operatorValue = edge?.cond?.op || defaultOperator;
      const isCondition = !!conditionKind && edge?.kind === conditionKind;
      return {
        edge,
        isCondition,
        conditionEdgeKind: conditionKind,
        modeValue: isCondition ? conditionKind : "fallback",
        modeOptions: [
          ...(conditionKind ? [{ value: conditionKind, label: view.branchConditionModeConditionLabel }] : []),
          { value: "fallback", label: view.branchConditionModeFallbackLabel },
        ],
        targetPrefix: view.branchConditionTargetPrefix,
        target,
        targetLabel: target?.isTerminal
          ? target.label
          : (targetMember?.name || target?.label || view.graphConditionTargetMissingLabel),
        condRef,
        conditionOptions,
        ownerOptions: conditionOptions.map((option) => ({
          value: option.inst.id,
          label: graphTemplateText(view.graphConditionOwnerOptionTemplate, {
            id: option.inst.id,
            name: option.member.name,
          }),
          option,
        })),
        ownerValue: condRef.instanceId || conditionOptions[0]?.inst.id || "",
        firstOwnerId: conditionOptions[0]?.inst.id || "",
        fields,
        fieldOptions: fields.map((field) => ({
          value: field.name,
          label: graphTemplateText(view.graphConditionFieldOptionTemplate, {
            id: field.id || field.name,
            name: field.name,
            type: field.type,
          }),
          field,
        })),
        fieldValue: condRef.field || "",
        fieldPlaceholderOption: { value: "", label: view.branchConditionFieldPlaceholder },
        condField,
        defaultOperator,
        operatorValue,
        operatorOptions: conditionOperatorOptions(contract, operatorValue),
        hasConditionOptions: conditionOptions.length > 0,
        noConditionOptionsHint: view.branchConditionNoOptionsHint,
      };
    });
}

export function graphTerminalControlState(inst, contract, graphView = null) {
  const view = graphCanvasViewState(graphView);
  const defaultTerminalKind = contractDefaultValue(contract, "graph_terminal_kind");
  const terminalKind = String(inst?.kind || defaultTerminalKind || "").trim();
  const terminalKindOptions = graphTerminalKindOptions(contract, terminalKind, graphView);
  const id = String(inst?.id || "");
  const labelValue = String(inst?.label || "");
  const col = Number.isFinite(Number(inst?.col)) ? Number(inst.col) + 1 : 1;
  const row = Number.isFinite(Number(inst?.row)) ? Number(inst.row) + 1 : 1;
  return {
    eyebrow: graphTemplateText(view.terminalEyebrowTemplate, { kind: terminalKind }),
    title: labelValue,
    idLine: graphTemplateText(view.terminalIdLineTemplate, { id, col, row }),
    deleteLabel: view.inspectorDeleteLabel,
    labelTitle: view.inspectorLabelTitle,
    labelValue,
    kindTitle: view.inspectorKindTitle,
    terminalKind,
    terminalKindOptions,
    selectedTerminalKind: terminalKindOptions.find((option) => option.value === terminalKind) || null,
    authoringLockedTitle: view.terminalAuthoringLockedTitle,
    authoringLockedHint: view.terminalAuthoringLockedHint,
    editable: false,
  };
}

export function graphEdgeInspectorState({ edge, instances = [], members = [], schemas = [], flow, contract, graphView = null } = {}) {
  const view = graphCanvasViewState(graphView);
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const sourceMembers = Array.isArray(members) ? members : [];
  const instanceById = new Map(sourceInstances.map((candidate) => [candidate.id, candidate]));
  const memberById = new Map(sourceMembers.map((candidate) => [candidate.id, candidate]));
  const fromInstance = instanceById.get(edge?.from) || null;
  const toInstance = instanceById.get(edge?.to) || null;
  const fromMember = fromInstance?.memberId ? memberById.get(fromInstance.memberId) || null : null;
  const toMember = toInstance?.memberId ? memberById.get(toInstance.memberId) || null : null;
  const condRef = graphConditionRefForEdge(edge);
  const conditionOptions = graphConditionOptions({
    instances: sourceInstances,
    members: sourceMembers,
    schemas,
    edge,
    flow,
    graphView,
  });
  const condOwner = conditionOptions.find((option) => option.inst.id === condRef.instanceId) || null;
  const fields = condOwner?.fields || conditionOptions[0]?.fields || [];
  const condField = fields.find((field) => field.name === condRef.field) || null;
  const defaultOperator = contractDefaultValue(contract, "condition_operator");
  const operatorValue = edge?.cond?.op || defaultOperator;
  const defaultEdgeKind = contractDefaultValue(contract, "graph_edge_kind");
  const conditionKind = contractDefaultValue(contract, "graph_condition_edge_kind");
  const edgeKind = String(edge?.kind || defaultEdgeKind || "").trim();
  const edgeKindOptions = graphEdgeKindOptions(contract, edgeKind, graphView);
  const isCondition = !!conditionKind && edgeKind === conditionKind;
  return {
    edge,
    fromInstance,
    toInstance,
    fromMember,
    toMember,
    eyebrow: graphTemplateText(view.edgeEyebrowTemplate, { kind: edgeKind }),
    title: graphTemplateText(view.edgeTitleTemplate, {
      from: fromMember?.name || fromInstance?.label || view.edgeRowMissingValue,
      to: toMember?.name || toInstance?.label || view.edgeRowMissingValue,
    }),
    idLine: graphTemplateText(view.edgeIdLineTemplate, { id: edge?.id || "" }),
    deleteLabel: view.inspectorDeleteLabel,
    kindTitle: view.inspectorKindTitle,
    labelTitle: view.inspectorLabelTitle,
    conditionTitle: view.edgeConditionTitle,
    noConditionOptionsHint: view.edgeNoConditionOptionsHint,
    ownerPlaceholderOption: { value: "", label: view.edgeOwnerPlaceholder },
    fromTitle: view.edgeFromTitle,
    toTitle: view.edgeToTitle,
    fromRows: [
      { key: "instance", label: view.edgeRowInstanceLabel, value: fromInstance?.id || "" },
      { key: "member", label: view.edgeRowMemberLabel, value: fromMember?.name || view.edgeRowMissingValue },
      { key: "schema", label: view.edgeRowSchemaLabel, value: fromMember?.schema || view.edgeRowMissingValue },
    ],
    toRows: [
      { key: "instance", label: view.edgeRowInstanceLabel, value: toInstance?.id || "" },
      { key: "member", label: view.edgeRowMemberLabel, value: toMember?.name || (toInstance?.isTerminal ? view.edgeTerminalMemberValue : view.edgeRowMissingValue) },
      { key: "schema", label: view.edgeRowSchemaLabel, value: toMember?.schema || view.edgeRowMissingValue },
    ],
    condRef,
    conditionOptions,
    condOwner,
    condField,
    ownerOptions: conditionOptions.map((option) => ({
      value: option.inst.id,
      label: graphTemplateText(view.graphConditionOwnerOptionTemplate, {
        id: option.inst.id,
        name: option.member.name,
      }),
      option,
    })),
    ownerValue: condRef.instanceId || "",
    fields,
    fieldOptions: fields.map((field) => ({
      value: field.name,
      label: graphTemplateText(view.graphConditionFieldOptionTemplate, {
        id: field.id || field.name,
        name: field.name,
        type: field.type,
      }),
      field,
    })),
    fieldValue: condRef.field || "",
    fieldPlaceholder: condOwner ? view.edgeFieldPlaceholder : view.edgeFieldNoSchemaPlaceholder,
    defaultOperator,
    operatorValue,
    operatorOptions: conditionOperatorOptions(contract, operatorValue),
    defaultEdgeKind,
    edgeKind,
    isCondition,
    conditionEdgeKind: conditionKind,
    edgeKindOptions,
    selectedEdgeKind: edgeKindOptions.find((option) => option.value === edgeKind) || null,
    conditionPatch: graphFirstConditionPatch(edge, conditionOptions, { defaultOperator }),
    hasConditionOptions: conditionOptions.length > 0,
  };
}

export function graphGateKindAllowed(contract, kind) {
  return contractStringValues(contract?.mob_definition?.graph_gate_kinds).includes(String(kind || "").trim());
}

export function graphTerminalKindAllowed(contract, kind) {
  return contractStringValues(contract?.mob_definition?.graph_terminal_kinds).includes(String(kind || "").trim());
}

export function graphGateKindPatch(rawKind, contract) {
  const gateKind = String(rawKind || "").trim();
  return graphGateKindAllowed(contract, gateKind) ? { gateKind } : {};
}

export function graphInstanceLabelPatch(rawLabel) {
  return { label: String(rawLabel || "") };
}

export function graphEdgeLabelPatch(rawLabel) {
  return { label: String(rawLabel || "") };
}

export function graphTerminalKindPatch(rawKind, contract) {
  const kind = String(rawKind || "").trim();
  return graphTerminalKindAllowed(contract, kind) ? { kind } : {};
}

export function graphJoinCollectionPatch(inst, collection, { incomingCount = 0, firstMemberId = "", contract } = {}) {
  const next = String(collection || "").trim();
  if (!collectionPolicyAllowed(contract, next)) return {};
  const draft = editorGraphDraftContract(contract) || emptyGraphDraftContract();
  const count = Math.max(1, Number(incomingCount) || 0);
  return {
    collection: next,
    label: `${draft.joinLabelPrefix}${next || draft.parallelMissingCollectionLabel}`,
    quorum: next === "quorum"
      ? { ...(inst?.quorum || {}), n: inst?.quorum?.n || count, m: count }
      : null,
    controllerRole: next && next !== "all" ? (inst?.controllerRole || firstMemberId || "") : "",
  };
}

export function graphJoinQuorumPatch(inst, n, incomingCount = 0) {
  return {
    quorum: {
      ...(inst?.quorum || {}),
      n: Number(n) || 1,
      m: Math.max(1, Number(incomingCount) || 0),
    },
  };
}

export function graphJoinControllerRolePatch(rawRole, members) {
  const controllerRole = String(rawRole || "").trim();
  return memberRoleAllowed(members, controllerRole) ? { controllerRole } : {};
}

export function graphForkDispatchPatch(_inst, dispatch, contract) {
  const next = String(dispatch || "").trim();
  if (!dispatchModeAllowed(contract, next)) return {};
  return { dispatch: next, label: next };
}
