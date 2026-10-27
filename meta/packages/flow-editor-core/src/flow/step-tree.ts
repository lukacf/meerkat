// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the flow-step-tree functions move byte-verbatim as plain JS,
// and the `options = {}` parameter defaults (flowStepUpdatePatch,
// flowStepInsertPatch, flowStepInsertTransition, basicConditionSourcePatch)
// plus flowStepValidation's destructured `= {}` default raise TS2339 under
// .ts semantics. Source-contract pins this exact text, so suppression must
// live at file level, not in the moved bodies. Resolution/linkage stays
// guarded behaviorally: the projection suite and export-keys test load the
// bundle and exercise these functions, so a missed import or re-export
// still fails the gate as a ReferenceError.
//
// Flow step-tree helpers for the Flow Editor controller plane. Moved
// verbatim from the controller.js flow-step-tree range: lane/step-tree
// traversal and structural patches (insert/update/delete/map), Basic picker
// and selection transitions, the per-field flow-step patch family, Basic
// condition patches, flowStepValidation, and emptyAuthoringFlowState.
// childLanes (S6, needed by flow/launch-modes.ts collectVisualSteps) and
// collectFlowStepIds (S7, needed by drafts/mob-settings.ts uniqueFlowStepId)
// were seeded here early and keep their original intra-cluster positions
// now that the rest of the cluster has landed (S9). The launch-modes and
// mob-settings edges back into this module are runtime-only import cycles
// with no module-init cross-calls.
import { conditionOperatorOptions, repeatIterationInputOptions } from "../contract/options";
import { normalizeStepToolScopeList } from "../domain/tool-skill-access";
import { outputFormatAllowed } from "../drafts/mob-settings";
import { normalizeOutputFormat, normalizePositiveInteger } from "../shared/normalize";
import { collectionPolicyAllowed, dependencyModeAllowed, dispatchModeAllowed } from "./launch-modes";
import { memberIdSet, optionValueAllowed, reconcileDeletedFlowStepReferences } from "./reconcile";

export function childLanes(step) {
  if (!step) return [];
  if (step.type === "repeat") return [{ id: "body", steps: step.steps || [] }];
  if (step.type === "branch") {
    return [
      ...(step.branches || []).map((branch) => ({ id: branch.id, steps: branch.steps || [] })),
      { id: "fallback", steps: step.fallback || [] },
    ];
  }
  if (step.type === "parallel") {
    return (step.branches || []).map((branch) => ({ id: branch.id, steps: branch.steps || [] }));
  }
  return [];
}

export function collectFlowMemberSteps(steps, out = []) {
  for (const step of steps || []) {
    if (step?.type === "member") out.push(step);
    for (const lane of childLanes(step || {})) collectFlowMemberSteps(lane.steps, out);
  }
  return out;
}

export function flowStepUpdatePatch(flow, id, patch = {}, options = {}) {
  let accepted = false;
  const steps = flowStepMap(flow?.steps || [], id, (step) => {
    const nextStep = { ...step, ...(patch && typeof patch === "object" ? patch : {}) };
    const validation = flowStepValidation(nextStep, { flow, members: options.members, currentId: step.id });
    if (!validation.ok) return step;
    accepted = true;
    return nextStep;
  });
  if (!accepted) return flow || {};
  return { ...(flow || {}), steps };
}

export function flowStepInsertPatch(flow, laneRef, newStep, options = {}) {
  const validation = flowStepValidation(newStep, { flow, members: options.members });
  if (!validation.ok) return flow || {};
  const steps = flowStepInsertIntoLane(flow?.steps || [], laneRef || {}, newStep);
  return { ...(flow || {}), steps };
}

export function flowStepInsertTransition(flow, laneRef, newStep, options = {}) {
  const validation = flowStepValidation(newStep, { flow, members: options.members });
  if (!validation.ok) {
    return {
      ok: false,
      error: validation.error || "",
      flow: flow || {},
      selection: null,
      picker: { open: false },
    };
  }
  return {
    ok: true,
    error: "",
    flow: flowStepInsertPatch(flow, laneRef, newStep, options),
    selection: newStep.id,
    picker: { open: false },
  };
}

export function flowStepDeletePatch(flow, id) {
  const target = String(id || "").trim();
  const steps = flowStepRemoveFromTree(flow?.steps || [], target);
  const nextFlow = { ...(flow || {}), steps };
  return target ? reconcileDeletedFlowStepReferences(nextFlow, target) : nextFlow;
}

export function flowStepDeleteTransition(flow, id) {
  return {
    flow: flowStepDeletePatch(flow, id),
    selection: null,
    picker: { open: false },
  };
}

export function basicStepPickerOpenTransition(laneRef) {
  return { picker: { open: true, at: laneRef || null } };
}

export function basicStepPickerCloseTransition() {
  return { picker: { open: false } };
}

export function basicCanvasClearTransition() {
  return { selection: null, picker: { open: false } };
}

export function basicStepSelectionTransition(id) {
  const selection = String(id || "").trim() || null;
  return { selection, picker: { open: false } };
}

export function flowStepTaskPatch(rawTask) {
  return { task: String(rawTask || "") };
}

export function flowStepInstructionPatch(rawInstruction) {
  return { instruction: String(rawInstruction || "") };
}

export function flowStepQuorumPatch(rawValue) {
  return { quorum: normalizePositiveInteger(rawValue) };
}

export function flowStepTimeoutPatch(rawValue) {
  return { timeoutMs: normalizePositiveInteger(rawValue) };
}

export function flowStepMaxIterationsPatch(rawValue) {
  return { maxIterations: normalizePositiveInteger(rawValue) };
}

export function flowStepLoopIdPatch(rawLoopId) {
  return { loopId: String(rawLoopId || "").trim() };
}

export function flowStepRepeatConditionPatch(step, patch = {}) {
  const currentCond = step?.cond && typeof step.cond === "object" && !Array.isArray(step.cond)
    ? step.cond
    : {};
  return { cond: { ...currentCond, ...patch } };
}

export function basicConditionSourcePatch(conditionOptions, rawStepId, options = {}) {
  const stepId = String(rawStepId || "").trim();
  const rows = Array.isArray(conditionOptions) ? conditionOptions : [];
  if (stepId && !rows.some((candidate) => String(candidate?.stepId || "").trim() === stepId)) {
    return {};
  }
  const selected = rows.find((candidate) => String(candidate?.stepId || "").trim() === stepId);
  const patch = { stepId, field: "" };
  if (options.includeNamespace) {
    patch.namespace = String(selected?.namespace || options.defaultNamespace || "steps").trim();
  }
  return patch;
}

export function basicConditionFieldPatch(rawField, fieldOptions) {
  const field = String(rawField || "").trim();
  const rows = Array.isArray(fieldOptions) ? fieldOptions : [];
  if (rows.length && field && !rows.some((option) => String(option?.value || option?.field?.name || "").trim() === field)) {
    return {};
  }
  return { field };
}

export function basicConditionOperatorPatch(rawOperator, contract) {
  const op = String(rawOperator || "").trim();
  if (contract && op && !conditionOperatorOptions(contract, op).some((option) => option.value === op && !option.disabled)) {
    return {};
  }
  return { op };
}

export function basicConditionValuePatch(rawValue) {
  return { val: rawValue ?? "" };
}

export function flowStepIterationInputPatch(rawMode, contract) {
  const iterationInput = String(rawMode || "").trim();
  if (!optionValueAllowed(repeatIterationInputOptions(contract, iterationInput), iterationInput, { allowBlank: true })) return {};
  return { iterationInput };
}

export function memberRoleAllowed(members, rawRole) {
  const role = String(rawRole || "").trim();
  if (!role) return true;
  return memberIdSet(members).has(role);
}

export function flowStepControllerRolePatch(rawRole, members) {
  const controllerRole = String(rawRole || "").trim();
  return memberRoleAllowed(members, controllerRole) ? { controllerRole } : {};
}

export function flowStepMemberRolePatch(rawRole, members) {
  const role = String(rawRole || "").trim();
  return memberRoleAllowed(members, role) ? { role } : {};
}

export function flowStepDispatchModePatch(rawMode, contract) {
  const mode = String(rawMode || "").trim();
  return dispatchModeAllowed(contract, mode) ? { dispatchMode: mode } : {};
}

export function flowStepParallelDispatchPatch(rawMode, contract) {
  const mode = String(rawMode || "").trim();
  return dispatchModeAllowed(contract, mode) ? { dispatch: mode } : {};
}

export function flowStepCollectionPatch(rawPolicy, contract) {
  const policy = String(rawPolicy || "").trim();
  return collectionPolicyAllowed(contract, policy) ? { collection: policy } : {};
}

export function flowStepDependencyModePatch(rawMode, contract) {
  const mode = String(rawMode || "").trim();
  return dependencyModeAllowed(contract, mode) ? { dependsMode: mode } : {};
}

export function flowStepOutputFormatPatch(rawFormat, contract) {
  const format = normalizeOutputFormat(rawFormat);
  return outputFormatAllowed(contract, format) ? { outputFormat: format } : {};
}

export function flowStepAllowedToolsPatch(tools, options = {}) {
  return { allowedTools: normalizeStepToolScopeList(tools, { ...options, mode: "member" }) };
}

export function flowStepBlockedToolsPatch(tools, options = {}) {
  return { blockedTools: normalizeStepToolScopeList(tools, { ...options, mode: "catalog" }) };
}

export function flowStepValidation(step, { flow, members, currentId = "" } = {}) {
  if (!step || typeof step !== "object") return { ok: false, error: "flow step must be an object" };
  const id = String(step.id || "").trim();
  if (!id) return { ok: false, error: "flow step must include id" };
  const target = String(currentId || "").trim();
  const ids = collectFlowStepIds(flow?.steps || []);
  if (ids.has(id) && (!target || id !== target)) {
    return { ok: false, error: "flow step id already exists" };
  }
  if (target && id !== target) {
    return { ok: false, error: "flow step id changes must use projection reconciliation" };
  }
  if (step.type === "member") {
    const role = String(step.role || "").trim();
    if (!role) return { ok: false, error: "member flow step must reference a member" };
    if (Array.isArray(members) && !memberIdSet(members).has(role)) {
      return { ok: false, error: "member flow step must reference an existing member" };
    }
  }
  return { ok: true, error: "" };
}

export function collectFlowStepIds(steps, out = new Set()) {
  for (const step of steps || []) {
    const id = String(step?.id || "").trim();
    if (id) out.add(id);
    for (const lane of childLanes(step || {})) collectFlowStepIds(lane.steps, out);
  }
  return out;
}

export function flowStepById(steps, id) {
  const target = String(id || "").trim();
  if (!target) return null;
  for (const step of steps || []) {
    if (String(step?.id || "").trim() === target) return step;
    for (const lane of childLanes(step || {})) {
      const found = flowStepById(lane.steps || [], target);
      if (found) return found;
    }
  }
  return null;
}

export function flowStepMap(steps, id, fn) {
  return (steps || []).map((step) => {
    if (step?.id === id) return fn(step);
    if (step?.type === "branch") {
      return {
        ...step,
        branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepMap(branch.steps || [], id, fn) })),
        fallback: flowStepMap(step.fallback || [], id, fn),
      };
    }
    if (step?.type === "parallel") {
      return {
        ...step,
        branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepMap(branch.steps || [], id, fn) })),
      };
    }
    if (step?.type === "repeat") return { ...step, steps: flowStepMap(step.steps || [], id, fn) };
    return step;
  });
}

export function flowStepInsertIntoLane(steps, laneRef, newStep) {
  if (!newStep) return steps || [];
  if (laneRef?.lane === "main") {
    const idx = laneRef.index ?? (steps || []).length;
    return [...(steps || []).slice(0, idx), newStep, ...(steps || []).slice(idx)];
  }
  return (steps || []).map((step) => {
    if (step?.id !== laneRef?.parentId) {
      if (step?.type === "branch") {
        return {
          ...step,
          branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepInsertIntoLane(branch.steps || [], laneRef, newStep) })),
          fallback: flowStepInsertIntoLane(step.fallback || [], laneRef, newStep),
        };
      }
      if (step?.type === "parallel") {
        return {
          ...step,
          branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepInsertIntoLane(branch.steps || [], laneRef, newStep) })),
        };
      }
      if (step?.type === "repeat") return { ...step, steps: flowStepInsertIntoLane(step.steps || [], laneRef, newStep) };
      return step;
    }
    const at = (arr) => {
      const lane = arr || [];
      const idx = laneRef.index ?? lane.length;
      return [...lane.slice(0, idx), newStep, ...lane.slice(idx)];
    };
    if (laneRef.branchId === "body") return { ...step, steps: at(step.steps) };
    if (laneRef.branchId === "fallback") return { ...step, fallback: at(step.fallback) };
    return {
      ...step,
      branches: (step.branches || []).map((branch) => branch.id === laneRef.branchId ? { ...branch, steps: at(branch.steps) } : branch),
    };
  });
}

export function flowStepRemoveFromTree(steps, id) {
  return (steps || []).filter((step) => step?.id !== id).map((step) => {
    if (step?.type === "branch") {
      return {
        ...step,
        branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepRemoveFromTree(branch.steps || [], id) })),
        fallback: flowStepRemoveFromTree(step.fallback || [], id),
      };
    }
    if (step?.type === "parallel") {
      return {
        ...step,
        branches: (step.branches || []).map((branch) => ({ ...branch, steps: flowStepRemoveFromTree(branch.steps || [], id) })),
      };
    }
    if (step?.type === "repeat") return { ...step, steps: flowStepRemoveFromTree(step.steps || [], id) };
    return step;
  });
}

export function emptyAuthoringFlowState() {
  return { name: "", steps: [] };
}
