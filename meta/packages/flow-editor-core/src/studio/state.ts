// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the studio-state functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (studioAddMemberPatch,
// studioUpdateMemberPatch, studioDeleteMemberPatch, memberUpdateCascadePatch,
// memberDeleteCascadePatch, the instance/edge/schema patch family, and the
// undo/redo history patches) raise TS2339 under .ts semantics.
// Source-contract pins this exact text, so suppression must live at file
// level, not in the moved bodies. Resolution/linkage stays guarded
// behaviorally: the projection suite and export-keys test load the bundle
// and exercise these functions, so a missed import or re-export still fails
// the gate as a ReferenceError.
//
// Studio document state transitions for the Flow Editor controller plane.
// Moved verbatim from the controller.js studio-state range: member
// add/update/delete validation and patches, the member update/delete cascade
// patches (which fan out into nine flow/reconcile.ts functions), graph
// instance/edge validation and patches, schema add/update/delete patches,
// and the studio undo/redo history snapshot family. The duplicate
// graphInstanceIdSet that lived in this range was already canonicalized
// into shared/normalize.ts in S1; this module imports that copy.
import { contractStringValues, profileBindingRestriction } from "../contract/options";
import { normalizeMobSettings } from "../drafts/mob-settings";
import { normalizedEdgeCondition } from "../flow/launch-modes";
import {
  clearDeletedLaunchSource,
  memberIdSet,
  reconcileAuthoringForMembers,
  reconcileConditionFieldAvailability,
  reconcileFlowControlRoles,
  reconcileFlowLaunchSources,
  reconcileFlowMemberSchemas,
  reconcileFlowMemberSteps,
  reconcileFlowStepToolScopes,
  reconcileGraphControlRoles,
  reconcileGraphLaunchSources,
  reconcileGraphStepToolScopes,
  reconcileMobSettingsProfiles,
} from "../flow/reconcile";
import { graphInstanceIdSet } from "../shared/normalize";

export function directMemberAddValidation(member, members = [], contract = null) {
  if (!member || typeof member !== "object") {
    return { ok: false, error: "member must be an object" };
  }
  const id = String(member.id || "").trim();
  const role = String(member.role || member.name || "").trim();
  const profileBinding = String(member.profileBinding || member.profile_binding || "").trim();
  const runtimeMode = String(member.runtimeMode || member.runtime_mode || "").trim();
  const model = String(member.model || "").trim();
  if (!id || !role) {
    return { ok: false, error: "member must include id and role/name" };
  }
  if ((Array.isArray(members) ? members : []).some((candidate) => String(candidate?.id || "").trim() === id)) {
    return { ok: false, error: "member id already exists" };
  }
  if (!deployableInlineProfileBindingAllowed(contract)) {
    return { ok: false, error: "Meta schema contract must allow deployable inline profileBinding" };
  }
  if (profileBinding !== "inline") {
    return { ok: false, error: "direct member adds must use an inline deployable profileBinding" };
  }
  if (!runtimeMode) {
    return { ok: false, error: "member must include runtimeMode" };
  }
  if (!contractStringValues(contract?.mob_definition?.runtime_modes).includes(runtimeMode)) {
    return { ok: false, error: "member runtimeMode must be allowed by mob_definition.runtime_modes" };
  }
  if (!model) {
    return { ok: false, error: "inline member definitions must include a model" };
  }
  return { ok: true, error: "" };
}

export function studioAddMemberPatch({ members, contract } = {}, member) {
  const list = Array.isArray(members) ? members : [];
  const validation = directMemberAddValidation(member, list, contract);
  if (!validation.ok) {
    return { ok: false, error: validation.error, members: list, member: null };
  }
  return { ok: true, error: "", members: [...list, member], member };
}

export function studioUpdateMemberPatch({ members, contract } = {}, id, patch = {}) {
  const target = String(id || "");
  const list = Array.isArray(members) ? members : [];
  const current = list.find((member) => member?.id === target) || null;
  if (!current) return { ok: false, error: "member not found", members: list };
  const nextMember = { ...current, ...(patch && typeof patch === "object" ? patch : {}) };
  const validation = memberUpdateValidation(current, nextMember, patch, contract);
  if (!validation.ok) {
    return { ok: false, error: validation.error, members: list };
  }
  return {
    ok: true,
    error: "",
    members: list.map((member) => member?.id === target ? nextMember : member),
    member: nextMember,
  };
}

export function memberUpdateValidation(current, nextMember, patch = {}, contract = null) {
  const touched = patch && typeof patch === "object" ? new Set(Object.keys(patch)) : new Set();
  const exportCritical = [
    "id",
    "profileBinding",
    "profile_binding",
    "runtimeMode",
    "runtime_mode",
    "model",
    "realmProfile",
    "realm_profile",
    "backend",
    "schema",
    "tools",
    "skills",
    "providerParams",
    "provider_params",
    "maxInlinePeerNotifications",
    "max_inline_peer_notifications",
  ];
  if (!exportCritical.some((key) => touched.has(key))) return { ok: true, error: "" };
  if (touched.has("id") && String(nextMember.id || "").trim() !== String(current?.id || "").trim()) {
    return { ok: false, error: "member id changes must use projection reconciliation" };
  }
  const binding = String(nextMember.profileBinding || nextMember.profile_binding || "").trim();
  const runtimeMode = String(nextMember.runtimeMode || nextMember.runtime_mode || "").trim();
  const model = String(nextMember.model || "").trim();
  if ((touched.has("profileBinding") || touched.has("profile_binding") || touched.has("runtimeMode") || touched.has("runtime_mode"))
    && !deployableInlineProfileBindingAllowed(contract)) {
    return { ok: false, error: "Meta schema contract must allow deployable inline profileBinding" };
  }
  if (binding && binding !== "inline") {
    return { ok: false, error: "member updates must keep deployable inline profileBinding" };
  }
  if (!binding && (touched.has("profileBinding") || touched.has("profile_binding"))) {
    return { ok: false, error: "member updates must keep profileBinding explicit" };
  }
  if (!runtimeMode && (touched.has("runtimeMode") || touched.has("runtime_mode"))) {
    return { ok: false, error: "member updates must keep runtimeMode explicit" };
  }
  if ((touched.has("runtimeMode") || touched.has("runtime_mode"))
    && !contractStringValues(contract?.mob_definition?.runtime_modes).includes(runtimeMode)) {
    return { ok: false, error: "member updates must use a mob_definition.runtime_modes value" };
  }
  if ((binding || current?.profileBinding === "inline" || current?.profile_binding === "inline") && !model && touched.has("model")) {
    return { ok: false, error: "inline member updates must keep a model" };
  }
  if (touched.has("schema") && typeof nextMember.schema !== "string") {
    return { ok: false, error: "member schema must be a string reference" };
  }
  if (touched.has("tools") && !stringListPatchValueIsValid(nextMember.tools)) {
    return { ok: false, error: "member tools must be an array of non-empty strings" };
  }
  if (touched.has("skills") && !stringListPatchValueIsValid(nextMember.skills)) {
    return { ok: false, error: "member skills must be an array of non-empty strings" };
  }
  if ((touched.has("providerParams") || touched.has("provider_params")) && !providerParamsPatchValueIsValid(nextMember.providerParams ?? nextMember.provider_params)) {
    return { ok: false, error: "member providerParams must be a JSON object" };
  }
  if ((touched.has("maxInlinePeerNotifications") || touched.has("max_inline_peer_notifications")) && !maxInlinePeerNotificationsPatchValueIsValid(nextMember.maxInlinePeerNotifications ?? nextMember.max_inline_peer_notifications)) {
    return { ok: false, error: "member maxInlinePeerNotifications must be an integer >= -1 or blank" };
  }
  return { ok: true, error: "" };
}

export function deployableInlineProfileBindingAllowed(contract) {
  const bindings = contractStringValues(contract?.mob_definition?.profile_binding);
  const restriction = profileBindingRestriction(contract, "inline");
  return bindings.includes("inline") && restriction.deployable !== false;
}

export function stringListPatchValueIsValid(value) {
  if (!Array.isArray(value)) return false;
  return value.every((item) => typeof item === "string" && !!item.trim());
}

export function providerParamsPatchValueIsValid(value) {
  if (value === null || value === undefined) return true;
  return typeof value === "object" && !Array.isArray(value);
}

export function maxInlinePeerNotificationsPatchValueIsValid(value) {
  if (value === null || value === undefined || value === "") return true;
  const number = typeof value === "number" ? value : Number(value);
  return Number.isInteger(number) && number >= -1;
}

export function studioDeleteMemberPatch({ members, instances, edges } = {}, id) {
  const target = String(id || "");
  const nextMembers = (members || []).filter((member) => member?.id !== target);
  const nextInstances = (instances || []).filter((instance) => instance?.memberId !== target);
  const remainingInstanceIds = new Set(nextInstances.map((instance) => instance?.id).filter(Boolean));
  const nextEdges = (edges || []).filter((edge) => remainingInstanceIds.has(edge?.from) && remainingInstanceIds.has(edge?.to));
  return { members: nextMembers, instances: nextInstances, edges: nextEdges };
}

export function memberUpdateCascadePatch({ memberId, members, flow, instances, edges, mobSettings, contract } = {}, patch = {}) {
  const sourceMembers = Array.isArray(members) ? members : [];
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const sourceEdges = Array.isArray(edges) ? edges : [];
  const normalizedMobSettings = normalizeMobSettings(mobSettings);
  const updated = studioUpdateMemberPatch({ members: sourceMembers, contract }, memberId, patch);
  if (!updated.ok) {
    return {
      ok: false,
      error: updated.error || "",
      patch: null,
      member: null,
      members: sourceMembers,
      flow,
      instances: sourceInstances,
      edges: sourceEdges,
      mobSettings: normalizedMobSettings,
    };
  }
  const reconciled = reconcileAuthoringForMembers({
    flow,
    instances: sourceInstances,
    edges: sourceEdges,
    mobSettings: normalizedMobSettings,
    previousMembers: sourceMembers,
    members: updated.members,
  });
  return {
    ok: true,
    error: "",
    patch,
    member: updated.member,
    members: updated.members,
    flow: reconciled.flow,
    instances: reconciled.instances,
    edges: reconciled.edges,
    mobSettings: reconciled.mobSettings,
  };
}

export function memberDeleteCascadePatch({ memberId, members, instances, edges, flow, mobSettings } = {}) {
  const target = String(memberId || "").trim();
  const sourceMembers = Array.isArray(members) ? members : [];
  const current = sourceMembers.find((member) => String(member?.id || "").trim() === target) || null;
  if (!current) {
    return {
      ok: false,
      error: "member not found",
      members: sourceMembers,
      instances: Array.isArray(instances) ? instances : [],
      edges: Array.isArray(edges) ? edges : [],
      flow,
      mobSettings: normalizeMobSettings(mobSettings),
      selection: null,
    };
  }
  const graphDeleted = studioDeleteMemberPatch({ members: sourceMembers, instances, edges }, target);
  let nextFlow = reconcileFlowMemberSteps(flow, graphDeleted.members);
  nextFlow = reconcileFlowMemberSchemas(nextFlow, graphDeleted.members);
  nextFlow = reconcileFlowControlRoles(nextFlow, graphDeleted.members);
  nextFlow = reconcileFlowLaunchSources(nextFlow, graphDeleted.members);
  nextFlow = reconcileFlowStepToolScopes(nextFlow, graphDeleted.members);
  let nextInstances = reconcileGraphControlRoles(graphDeleted.instances, graphDeleted.members);
  nextInstances = reconcileGraphLaunchSources(nextInstances, graphDeleted.members);
  nextInstances = reconcileGraphStepToolScopes(nextInstances, graphDeleted.members);
  const nextMobSettings = reconcileMobSettingsProfiles(mobSettings, sourceMembers, graphDeleted.members);
  return {
    ok: true,
    error: "",
    removed: current,
    members: graphDeleted.members,
    instances: nextInstances,
    edges: graphDeleted.edges,
    flow: nextFlow,
    mobSettings: nextMobSettings,
    selection: null,
  };
}

export function graphInstanceValidation(instance, { instances, members, currentId = "" } = {}) {
  if (!instance || typeof instance !== "object") return { ok: false, error: "graph node must be an object" };
  const id = String(instance.id || "").trim();
  if (!id) return { ok: false, error: "graph node must include id" };
  const target = String(currentId || "").trim();
  const duplicate = (Array.isArray(instances) ? instances : []).some((candidate) => {
    const candidateId = String(candidate?.id || "").trim();
    return candidateId && candidateId === id && (!target || candidateId !== target);
  });
  if (duplicate) return { ok: false, error: "graph node id already exists" };
  const isControlNode = !!instance.isGate || !!instance.isTerminal || !!String(instance.gateKind || "").trim();
  const memberId = String(instance.memberId || "").trim();
  if (!isControlNode) {
    if (!memberId) return { ok: false, error: "member graph node must reference a member" };
    if (!memberIdSet(members).has(memberId)) return { ok: false, error: "member graph node must reference an existing member" };
  }
  return { ok: true, error: "" };
}

export function studioUpdateInstancePatch({ instances, members } = {}, id, patch = {}) {
  const target = String(id || "");
  const list = Array.isArray(instances) ? instances : [];
  const current = list.find((instance) => instance?.id === target) || null;
  if (!current) return { ok: false, error: "graph node not found", instances: list };
  const nextInstance = { ...current, ...(patch && typeof patch === "object" ? patch : {}) };
  const validation = graphInstanceValidation(nextInstance, { instances: list, members, currentId: target });
  if (!validation.ok) return { ok: false, error: validation.error, instances: list };
  return {
    ok: true,
    error: "",
    instances: list.map((instance) => instance?.id === target ? nextInstance : instance),
    instance: nextInstance,
  };
}

export function studioMoveInstancePatch({ instances } = {}, id, cell, originalCell = {}) {
  const target = String(id || "");
  const nextCol = Number.isInteger(cell?.col) ? cell.col : null;
  const nextRow = Number.isInteger(cell?.row) ? cell.row : null;
  if (!target || nextCol === null || nextRow === null) return { instances: instances || [] };
  const sourceInstances = instances || [];
  const moving = sourceInstances.find((instance) => instance?.id === target);
  if (!moving) return { instances: sourceInstances };
  const occupant = sourceInstances.find((instance) =>
    instance?.id !== target && instance?.col === nextCol && instance?.row === nextRow
  );
  const originalCol = Number.isInteger(originalCell?.col) ? originalCell.col : moving.col;
  const originalRow = Number.isInteger(originalCell?.row) ? originalCell.row : moving.row;
  return {
    instances: sourceInstances.map((instance) => {
      if (instance?.id === target) return { ...instance, col: nextCol, row: nextRow };
      if (occupant && instance?.id === occupant.id) return { ...instance, col: originalCol, row: originalRow };
      return instance;
    }),
  };
}

export function studioDeleteInstancePatch({ instances, edges } = {}, id) {
  const target = String(id || "");
  const nextInstances = (instances || [])
    .filter((instance) => instance?.id !== target)
    .map((instance) => clearDeletedLaunchSource(instance, target));
  const nextEdges = clearDeletedGraphConditionEdges(
    (edges || []).filter((edge) => edge?.from !== target && edge?.to !== target),
    target,
  );
  return {
    instances: nextInstances,
    edges: nextEdges,
    selection: { kind: null, id: null },
  };
}

export function clearDeletedGraphConditionEdges(edges, deletedId) {
  const target = String(deletedId || "").trim();
  if (!target) return edges || [];
  let changed = false;
  const next = (edges || []).map((edge) => {
    const condition = normalizedEdgeCondition(edge);
    const parts = String(condition?.path || "").split(".").filter(Boolean);
    if (parts.length !== 3 || parts[0] !== "steps" || parts[1] !== target) return edge;
    changed = true;
    return { ...edge, cond: null, label: "" };
  });
  return changed ? next : edges;
}

export function graphEdgeValidation(edge, { instances, edges, currentId = "" } = {}) {
  if (!edge || typeof edge !== "object") return { ok: false, error: "edge must be an object" };
  const id = String(edge.id || "").trim();
  const from = String(edge.from || "").trim();
  const to = String(edge.to || "").trim();
  if (!id || !from || !to) return { ok: false, error: "edge must include id, from, and to" };
  if (from === to) return { ok: false, error: "edge endpoints must be different graph nodes" };
  const instanceIds = graphInstanceIdSet(instances);
  if (!instanceIds.has(from) || !instanceIds.has(to)) {
    return { ok: false, error: "edge endpoints must reference existing graph nodes" };
  }
  const target = String(currentId || "").trim();
  const duplicate = (Array.isArray(edges) ? edges : []).some((candidate) => {
    const candidateId = String(candidate?.id || "").trim();
    if (target && candidateId === target) return false;
    return candidateId === id || (candidate?.from === from && candidate?.to === to);
  });
  if (duplicate) return { ok: false, error: "edge already exists" };
  return { ok: true, error: "" };
}

export function studioUpdateEdgePatch({ edges, instances } = {}, id, patch = {}) {
  const target = String(id || "");
  const list = Array.isArray(edges) ? edges : [];
  const current = list.find((edge) => edge?.id === target) || null;
  if (!current) return { ok: false, error: "edge not found", edges: list };
  const nextEdge = { ...current, ...(patch && typeof patch === "object" ? patch : {}) };
  const validation = graphEdgeValidation(nextEdge, { instances, edges: list, currentId: target });
  if (!validation.ok) return { ok: false, error: validation.error, edges: list };
  return {
    ok: true,
    error: "",
    edges: list.map((edge) => edge?.id === target ? nextEdge : edge),
    edge: nextEdge,
  };
}

export function studioDeleteEdgePatch({ edges } = {}, id) {
  const target = String(id || "");
  return {
    edges: (edges || []).filter((edge) => edge?.id !== target),
    selection: { kind: null, id: null },
  };
}

export function studioAddSchemaPatch({ schemas } = {}, schema) {
  const list = Array.isArray(schemas) ? schemas : [];
  const id = String(schema?.id || "").trim();
  if (!id) return { ok: false, error: "schema must include id", schemas: list, schema: null };
  if (list.some((candidate) => String(candidate?.id || "").trim() === id)) {
    return { ok: false, error: "schema id already exists", schemas: list, schema: null };
  }
  return { ok: true, error: "", schemas: [...list, schema], schema };
}

export function studioUpdateSchemaPatch({ schemas } = {}, id, patch = {}) {
  const target = String(id || "");
  const list = Array.isArray(schemas) ? schemas : [];
  const current = list.find((schema) => schema?.id === target) || null;
  if (!current) return { ok: false, error: "schema not found", schemas: list };
  if (patch && typeof patch === "object" && Object.prototype.hasOwnProperty.call(patch, "id")) {
    const nextId = String(patch.id || "").trim();
    if (nextId !== target) {
      return { ok: false, error: "schema id changes must use renameSchemaDefinition", schemas: list };
    }
  }
  const nextSchema = { ...current, ...(patch && typeof patch === "object" ? patch : {}) };
  return {
    ok: true,
    error: "",
    schemas: list.map((schema) => schema?.id === target ? nextSchema : schema),
    schema: nextSchema,
  };
}

export function studioDeleteSchemaPatch({ schemas, members, flow, edges, instances } = {}, id) {
  const target = String(id || "");
  const nextSchemas = (schemas || []).filter((schema) => schema?.id !== target);
  const nextMembers = (members || []).map((member) => member?.schema === target ? { ...member, schema: "" } : member);
  const result = {
    schemas: nextSchemas,
    members: nextMembers,
    selection: null,
  };
  if (flow || edges) {
    const reconciled = reconcileConditionFieldAvailability({
      flow,
      edges,
      members: nextMembers,
      instances,
      schemas: nextSchemas,
    });
    if (flow) result.flow = reconciled.flow;
    if (edges) result.edges = reconciled.edges;
  }
  return result;
}

export function studioSnapshotState(state = {}) {
  return {
    members: Array.isArray(state.members) ? state.members : [],
    instances: Array.isArray(state.instances) ? state.instances : [],
    edges: Array.isArray(state.edges) ? state.edges : [],
    frames: Array.isArray(state.frames) ? state.frames : [],
    schemas: Array.isArray(state.schemas) ? state.schemas : [],
    skillRealms: Array.isArray(state.skillRealms) ? state.skillRealms : [],
  };
}

export function studioHistorySnapshotPatch({ history, future, state } = {}) {
  const currentHistory = Array.isArray(history) ? history : [];
  return {
    history: [...currentHistory.slice(-30), studioSnapshotState(state)],
    future: [],
  };
}

export function studioUndoPatch({ history, future, state } = {}) {
  const currentHistory = Array.isArray(history) ? history : [];
  if (!currentHistory.length) return null;
  const previous = studioSnapshotState(currentHistory[currentHistory.length - 1]);
  return {
    state: previous,
    history: currentHistory.slice(0, -1),
    future: [...(Array.isArray(future) ? future : []), studioSnapshotState(state)],
  };
}

export function studioRedoPatch({ history, future, state } = {}) {
  const currentFuture = Array.isArray(future) ? future : [];
  if (!currentFuture.length) return null;
  const next = studioSnapshotState(currentFuture[currentFuture.length - 1]);
  return {
    state: next,
    history: [...(Array.isArray(history) ? history : []), studioSnapshotState(state)],
    future: currentFuture.slice(0, -1),
  };
}
