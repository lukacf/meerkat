// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the reconcile functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (reconcileAuthoringForMembers,
// renameSchemaDefinition, reconcileAuthoringWithContract,
// reconcileInputParamReferences, reconcileConditionFieldAvailability,
// reconcileSchemaFieldReferences) raise TS2339 under .ts semantics.
// Source-contract pins this exact text, so suppression must live at file
// level, not in the moved bodies. Resolution/linkage stays guarded
// behaviorally: the projection suite and export-keys test load the bundle
// and exercise these functions, so a missed import or re-export still fails
// the gate as a ReferenceError.
//
// Authoring reconciliation for the Flow Editor controller plane. Moved
// verbatim from the controller.js reconcile range: deleted-step reference
// cleanup, member/schema/control-role/launch-source/tool-scope
// reconciliation for Basic and Graph projections, schema id renames,
// deploy/mob settings contract reconciliation and patches,
// reconcileAuthoringWithContract (the aggregate entry), input-param and
// schema-field reference rewrites, and condition availability — plus the
// four reconcile-domain functions that were misfiled in the graph range
// (reconcileSchemaFieldReferencesInEdges, conditionTextForPath,
// reconcileMemberSchemasInSteps, reconcileMemberSchemaInStep), kept in
// their original relative order after the main cluster.
//
// SCC note: members/patches.ts is the co-moved S8 partner (it calls
// catalogValueAllowed/optionValueAllowed/reconcileConditionFieldAvailability
// here). The schema/field-edit.ts -> flow/reconcile.ts edge that used to go
// through the lazy _residue-bridge is now a relative import, a runtime-only
// cycle with reconcile's enumValuesForField import (no module-init
// cross-calls). The last bridge-side straggler, deploySettingsForUi, was
// re-homed to catalogs/hydration.ts in S17 and is now a relative import.
import { deploySettingsForUi } from "../catalogs/hydration";
import {
  contractStringValues,
  firstDeploySurfaceRuntimeMode,
  runtimeModeDeploySurfaceAllowed,
} from "../contract/options";
import { profileName } from "../domain/tool-skill-access";
import { mobSettingsForUi, normalizeMobSettings } from "../drafts/mob-settings";
import { enumValuesForField } from "../schema/field-edit";
import { MOB_SETTINGS_PATCH_KEYS } from "../shared/constants";
import { normalizeStringList } from "../shared/normalize";
import {
  collectVisualSteps,
  launchModeFromAuthoringSource,
  normalizedEdgeCondition,
} from "./launch-modes";

export function reconcileDeletedFlowStepReferences(flow, deletedId) {
  if (!flow || typeof flow !== "object") return flow;
  const target = String(deletedId || "").trim();
  if (!target) return flow;
  const steps = reconcileDeletedFlowStepReferencesInSteps(flow.steps || [], target);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileDeletedFlowStepReferencesInSteps(steps, deletedId) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileDeletedFlowStepReferencesInStep(step, deletedId);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileDeletedFlowStepReferencesInStep(step, deletedId) {
  if (!step || typeof step !== "object") return step;
  let next = clearDeletedLaunchSource(step, deletedId);
  if (step.type === "repeat") {
    const cond = clearDeletedStepCondition(step.cond, deletedId);
    const until = clearDeletedStepConditionText(step.until, deletedId, cond);
    const nested = reconcileDeletedFlowStepReferencesInSteps(step.steps || [], deletedId);
    if (cond !== step.cond || until !== step.until || nested !== step.steps) {
      next = { ...next, cond, until, steps: nested };
    }
  }
  if (step.type === "branch") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const cond = clearDeletedStepCondition(branch.cond, deletedId);
      const condition = clearDeletedStepConditionText(branch.condition, deletedId, cond);
      const branchSteps = reconcileDeletedFlowStepReferencesInSteps(branch.steps || [], deletedId);
      if (cond === branch.cond && condition === branch.condition && branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, cond, condition, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileDeletedFlowStepReferencesInSteps(step.fallback, deletedId)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    if (changed) next = { ...next, branches, fallback };
  }
  if (step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileDeletedFlowStepReferencesInSteps(branch.steps || [], deletedId);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    if (changed) next = { ...next, branches };
  }
  return next;
}

export function clearDeletedLaunchSource(source, deletedId) {
  const mode = launchModeFromAuthoringSource(source);
  if (!mode || mode.kind !== "Fork" || String(mode.from || "").trim() !== deletedId) return source;
  return {
    ...source,
    launchMode: { kind: "Fresh" },
  };
}

export function clearDeletedStepCondition(cond, deletedId) {
  if (!cond || typeof cond !== "object") return cond;
  const stepId = String(cond.stepId || cond.step_id || "").trim();
  if (stepId !== deletedId) return cond;
  return {};
}

export function clearDeletedStepConditionText(text, deletedId, preferredCond) {
  if (preferredCond !== undefined) {
    if (preferredCond && preferredCond.field) return editorConditionText(preferredCond);
    return "";
  }
  if (!text) return text;
  const parsed = parseEditorConditionText(text);
  if (!parsed || parsed.namespace === "params" || parsed.stepId !== deletedId) return text;
  return "";
}

export function reconcileFlowMemberSchemas(flow, members) {
  if (!flow || typeof flow !== "object") return flow;
  const memberById = new Map((members || []).map((member) => [member.id, member]));
  const steps = reconcileMemberSchemasInSteps(flow.steps || [], memberById);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function renameSchemaDefinition({ schemas, members, flow } = {}, oldId, newId) {
  const previousId = String(oldId || "").trim();
  const nextId = String(newId || "").trim();
  const sourceSchemas = Array.isArray(schemas) ? schemas : [];
  const sourceMembers = Array.isArray(members) ? members : [];
  if (!previousId || !nextId || previousId === nextId) {
    return { schemas: sourceSchemas, members: sourceMembers, flow, renamed: false, selection: null };
  }
  if (sourceSchemas.some((schema) => String(schema?.id || "").trim() === nextId)) {
    return {
      schemas: sourceSchemas,
      members: sourceMembers,
      flow,
      renamed: false,
      selection: null,
      reason: "duplicate_schema_id",
    };
  }
  let found = false;
  const nextSchemas = sourceSchemas.map((schema) => {
    if (String(schema?.id || "").trim() !== previousId) return schema;
    found = true;
    return { ...schema, id: nextId };
  });
  if (!found) {
    return {
      schemas: sourceSchemas,
      members: sourceMembers,
      flow,
      renamed: false,
      selection: null,
      reason: "unknown_schema_id",
    };
  }
  const nextMembers = sourceMembers.map((member) =>
    String(member?.schema || "").trim() === previousId
      ? { ...member, schema: nextId }
      : member
  );
  return {
    schemas: nextSchemas,
    members: nextMembers,
    flow: rewriteAdaptiveResultSchemaInFlow(
      reconcileFlowMemberSchemas(flow, nextMembers),
      previousId,
      nextId,
    ),
    renamed: true,
    selection: { kind: "schema", id: nextId },
  };
}

// Mirrors Meta's rewrite_adaptive_result_schema_references: a schema id
// rename remaps an adaptive layer's matching resultSchema alongside the
// member schema refs (the server also remaps the graph instance's embedded
// adaptive payload, which the authoritative apply_operation result carries).
export function rewriteAdaptiveResultSchemaInFlow(flow, oldId, newId) {
  if (!flow || typeof flow !== "object") return flow;
  const steps = rewriteAdaptiveResultSchemaInSteps(flow.steps || [], oldId, newId);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function rewriteAdaptiveResultSchemaInSteps(steps, oldId, newId) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = rewriteAdaptiveResultSchemaInStep(step, oldId, newId);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function rewriteAdaptiveResultSchemaInStep(step, oldId, newId) {
  if (!step || typeof step !== "object") return step;
  if (step.type === "adaptive") {
    return String(step.resultSchema || "").trim() === oldId
      ? { ...step, resultSchema: newId }
      : step;
  }
  if (step.type === "repeat") {
    const nested = rewriteAdaptiveResultSchemaInSteps(step.steps || [], oldId, newId);
    return nested === step.steps ? step : { ...step, steps: nested };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = rewriteAdaptiveResultSchemaInSteps(branch?.steps || [], oldId, newId);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? rewriteAdaptiveResultSchemaInSteps(step.fallback, oldId, newId)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  return step;
}

export function reconcileFlowMemberSteps(flow, members) {
  if (!flow || typeof flow !== "object") return flow;
  const memberIds = memberIdSet(members);
  const steps = pruneMissingMemberSteps(flow.steps || [], memberIds);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function pruneMissingMemberSteps(steps, memberIds) {
  let changed = false;
  const next = [];
  for (const step of steps || []) {
    const pruned = pruneMissingMemberStep(step, memberIds);
    if (!pruned) {
      changed = true;
      continue;
    }
    if (pruned !== step) changed = true;
    next.push(pruned);
  }
  return changed ? next : steps;
}

export function pruneMissingMemberStep(step, memberIds) {
  // Mirrors Meta's prune_step_array_for_members: member steps without a
  // live member are dropped, but containers (repeat/branch/parallel) stay
  // even when emptied — an empty lane is a legitimate draft state, and the
  // server keeps it. Adaptive steps also stay: only their stale member refs
  // reconcile (flowmasterId clears, stale profile templates drop).
  if (!step || typeof step !== "object") return step;
  if (step.type === "member") {
    const role = String(step.role || "").trim();
    return role && memberIds.has(role) ? step : null;
  }
  if (step.type === "adaptive") {
    return pruneAdaptiveStepMemberRefs(step, memberIds);
  }
  if (step.type === "repeat") {
    const steps = pruneMissingMemberSteps(step.steps || [], memberIds);
    return steps === step.steps ? step : { ...step, steps };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = pruneMissingMemberSteps(branch?.steps || [], memberIds);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? pruneMissingMemberSteps(step.fallback, memberIds)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  return step;
}

// Mirrors Meta's prune_adaptive_step_member_refs: a stale flowmasterId
// clears to "", stale profileTemplateIds entries drop, and the step itself
// never drops. Applied to the flow step and to a graph instance's embedded
// adaptive payload alike so graph_derives_flow equivalence keeps holding.
export function pruneAdaptiveStepMemberRefs(step, memberIds) {
  if (!step || typeof step !== "object") return step;
  const flowmaster = String(step.flowmasterId || "").trim();
  const flowmasterStale = !!flowmaster && !memberIds.has(flowmaster);
  const templates = Array.isArray(step.profileTemplateIds) ? step.profileTemplateIds : [];
  const keptTemplates = templates.filter(
    (template) => typeof template === "string" && memberIds.has(template.trim()),
  );
  if (!flowmasterStale && keptTemplates.length === templates.length) return step;
  return {
    ...step,
    flowmasterId: flowmasterStale ? "" : step.flowmasterId,
    profileTemplateIds: keptTemplates,
  };
}

export function reconcileFlowControlRoles(flow, members) {
  if (!flow || typeof flow !== "object") return flow;
  const memberIds = memberIdSet(members);
  const steps = reconcileControlRolesInSteps(flow.steps || [], memberIds);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileGraphControlRoles(instances, members) {
  const memberIds = memberIdSet(members);
  let changed = false;
  const next = (instances || []).map((instance) => {
    const reconciled = reconcileControlRoleObject(instance, memberIds);
    if (reconciled !== instance) changed = true;
    return reconciled;
  });
  return changed ? next : instances;
}

export function reconcileGraphMemberInstances({ instances, edges }, members) {
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const sourceEdges = Array.isArray(edges) ? edges : [];
  const memberIds = memberIdSet(members);
  const keptIds = new Set();
  let instancesChanged = false;
  const nextInstances = [];
  for (const instance of sourceInstances) {
    const memberId = String(instance?.memberId || "").trim();
    const keep = !memberId || memberIds.has(memberId);
    if (!keep) {
      instancesChanged = true;
      continue;
    }
    const id = String(instance?.id || "").trim();
    if (id) keptIds.add(id);
    let nextInstance = instance;
    if (
      String(instance?.kind || "") === "adaptive"
      && !instance?.isGate
      && !instance?.isTerminal
      && instance?.adaptive && typeof instance.adaptive === "object"
    ) {
      // Mirror the flow-side adaptive ref pruning on the embedded payload so
      // the graph still derives the flow byte-identically (server parity).
      const prunedPayload = pruneAdaptiveStepMemberRefs(instance.adaptive, memberIds);
      if (prunedPayload !== instance.adaptive) {
        instancesChanged = true;
        nextInstance = { ...instance, adaptive: prunedPayload };
      }
    }
    nextInstances.push(nextInstance);
  }
  let edgesChanged = false;
  const nextEdges = sourceEdges.filter((edge) => {
    const from = String(edge?.from || "").trim();
    const to = String(edge?.to || "").trim();
    const keep = (!from || keptIds.has(from)) && (!to || keptIds.has(to));
    if (!keep) edgesChanged = true;
    return keep;
  });
  return {
    instances: instancesChanged ? nextInstances : instances,
    edges: edgesChanged ? nextEdges : edges,
  };
}

export function reconcileFlowLaunchSources(flow, members) {
  if (!flow || typeof flow !== "object") return flow;
  const allowedSources = flowLaunchSourceSet(flow, members);
  const steps = reconcileLaunchSourcesInSteps(flow.steps || [], allowedSources);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileGraphLaunchSources(instances, members) {
  const allowedSources = graphLaunchSourceSet(instances, members);
  let changed = false;
  const next = (instances || []).map((instance) => {
    const reconciled = reconcileLaunchSourceObject(instance, allowedSources);
    if (reconciled !== instance) changed = true;
    return reconciled;
  });
  return changed ? next : instances;
}

export function reconcileFlowStepToolScopes(flow, members) {
  if (!flow || typeof flow !== "object") return flow;
  const memberTools = memberToolIndex(members);
  const steps = reconcileToolScopesInSteps(flow.steps || [], (step) => memberTools.get(step?.role) || new Set());
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileGraphStepToolScopes(instances, members) {
  const memberTools = memberToolIndex(members);
  let changed = false;
  const next = (instances || []).map((instance) => {
    if (!instance?.memberId) return instance;
    const reconciled = reconcileToolScopeObject(instance, memberTools.get(instance.memberId) || new Set());
    if (reconciled !== instance) changed = true;
    return reconciled;
  });
  return changed ? next : instances;
}

export function reconcileAuthoringForMembers({ flow, instances, edges, mobSettings, previousMembers, members } = {}) {
  const nextMembers = Array.isArray(members) ? members : [];
  let nextFlow = reconcileFlowMemberSteps(flow, nextMembers);
  nextFlow = reconcileFlowMemberSchemas(nextFlow, nextMembers);
  nextFlow = reconcileFlowControlRoles(nextFlow, nextMembers);
  nextFlow = reconcileFlowLaunchSources(nextFlow, nextMembers);
  nextFlow = reconcileFlowStepToolScopes(nextFlow, nextMembers);
  const memberSynced = reconcileGraphMemberInstances({ instances, edges }, nextMembers);
  let nextInstances = reconcileGraphControlRoles(memberSynced.instances, nextMembers);
  nextInstances = reconcileGraphLaunchSources(nextInstances, nextMembers);
  nextInstances = reconcileGraphStepToolScopes(nextInstances, nextMembers);
  return {
    flow: nextFlow,
    instances: nextInstances,
    edges: memberSynced.edges,
    mobSettings: reconcileMobSettingsProfiles(mobSettings, previousMembers, nextMembers),
  };
}

export function reconcileMemberSkillRefs(members, skillRealms, options = {}) {
  const knownSkills = skillIdSet(skillRealms);
  if (knownSkills.size === 0 && !options.strictEmpty) return members;
  let changed = false;
  const next = (members || []).map((member) => {
    if (!member || typeof member !== "object") return member;
    const skills = normalizeStringList(member.skills).filter((skill) => knownSkills.has(skill));
    if (JSON.stringify(skills) === JSON.stringify(normalizeStringList(member.skills))) return member;
    changed = true;
    return { ...member, skills };
  });
  return changed ? next : members;
}

export function reconcileMemberSchemaRefs(members, schemas, options = {}) {
  const knownSchemas = new Set((Array.isArray(schemas) ? schemas : [])
    .map((schema) => String(schema?.id || "").trim())
    .filter(Boolean));
  if (knownSchemas.size === 0 && !options.strictEmpty) return members;
  let changed = false;
  const next = (members || []).map((member) => {
    if (!member || typeof member !== "object") return member;
    const schema = String(member.schema || "").trim();
    if (!schema || knownSchemas.has(schema)) return member;
    changed = true;
    return { ...member, schema: "" };
  });
  return changed ? next : members;
}

export function reconcileDeploySettingsWithContract(settings, contract, modelCatalog, options = {}) {
  const source = deploySettingsForUi(settings);
  let next = source;
  const write = (key, value) => {
    if (next[key] === value) return;
    if (next === source) next = { ...source };
    next[key] = value;
  };
  const command = String(contract?.deploy_settings?.command || "").trim();
  if (command) write("command", command);
  reconcileStringField(next, write, "surface", contract?.deploy_settings?.surfaces);
  reconcileStringField(next, write, "trustPolicy", contract?.deploy_settings?.trust_policies);
  reconcileStringField(next, write, "realmBackend", contract?.deploy_settings?.realm_backends);
  const modelIds = (modelCatalog || [])
    .map((model) => String(model?.id || "").trim())
    .filter(Boolean);
  if ((modelIds.length || options.strictEmptyModels) && source.model && !modelIds.includes(source.model)) {
    write("model", "");
  }
  return next === source ? settings : next;
}

export function deploySettingsPatch(settings, patch, options = {}) {
  const source = deploySettingsForUi(settings);
  const rawPatch = patch && typeof patch === "object" ? patch : {};
  const next = { ...source };
  const command = String(options.contract?.deploy_settings?.command || "").trim();
  const modelIds = (options.modelCatalog || [])
    .map((model) => String(model?.id || "").trim())
    .filter(Boolean);
  for (const [key, value] of Object.entries(rawPatch)) {
    if (key === "command" && command) {
      if (String(value || "").trim() === command) next.command = command;
      continue;
    }
    if (key === "surface" && !contractValueAllowed(options.contract?.deploy_settings?.surfaces, value)) continue;
    if (key === "trustPolicy" && !contractValueAllowed(options.contract?.deploy_settings?.trust_policies, value)) continue;
    if (key === "realmBackend" && !contractValueAllowed(options.contract?.deploy_settings?.realm_backends, value)) continue;
    if (key === "model" && !catalogValueAllowed(modelIds, value)) continue;
    next[key] = value;
  }
  return deploySettingsForUi(next);
}

export function deploySettingsFieldPatch(settings, field, value, options = {}) {
  const key = String(field || "").trim();
  if (!key) return deploySettingsForUi(settings);
  return deploySettingsPatch(settings, { [key]: value }, options);
}

export function mobSettingsPatch(settings, patch, options = {}) {
  const source = normalizeMobSettings(settings);
  const rawPatch = patch && typeof patch === "object" ? patch : {};
  const next = { ...source };
  for (const [key, value] of Object.entries(rawPatch)) {
    if (!MOB_SETTINGS_PATCH_KEYS.has(key)) continue;
    if (key === "backendDefault" && !contractValueAllowed(options.contract?.mob_definition?.profile_backends, value, { allowBlank: true })) continue;
    next[key] = value;
  }
  return normalizeMobSettings(next);
}

export function mobSettingsFieldPatch(settings, field, value, options = {}) {
  const key = String(field || "").trim();
  if (!key) return normalizeMobSettings(settings);
  return mobSettingsPatch(settings, { [key]: value }, options);
}

export function reconcileMembersWithContract(members, contract, deploySettings, modelCatalog, toolCatalog, options = {}) {
  const source = Array.isArray(members) ? members : [];
  const runtimeModes = contractStringValues(contract?.mob_definition?.runtime_modes);
  const profileBindings = contractStringValues(contract?.mob_definition?.profile_binding);
  const profileBackends = contractStringValues(contract?.mob_definition?.profile_backends);
  const modelIds = (modelCatalog || [])
    .map((model) => String(model?.id || "").trim())
    .filter(Boolean);
  const toolIds = (toolCatalog || [])
    .map((tool) => String(tool?.id || "").trim())
    .filter(Boolean);
  if (!runtimeModes.length && !profileBindings.length && !profileBackends.length && !modelIds.length && !toolIds.length && !options.strictEmptyModels && !options.strictEmptyTools) {
    return members;
  }
  const surface = String(deploySettings?.surface || "").trim();
  let changed = false;
  const next = source.map((member) => {
    if (!member || typeof member !== "object") return member;
    let out = member;
    const write = (key, value) => {
      if (out[key] === value) return;
      if (out === member) out = { ...member };
      out[key] = value;
    };
    if (profileBindings.length) {
      const binding = String(member.profileBinding || member.profile_binding || "").trim();
      if (binding && !profileBindings.includes(binding)) write("profileBinding", "");
    }
    if (runtimeModes.length) {
      const runtimeMode = String(member.runtimeMode || member.runtime_mode || "").trim();
      const knownRuntimeMode = runtimeMode && runtimeModes.includes(runtimeMode);
      if (runtimeMode && !knownRuntimeMode) write("runtimeMode", "");
      if (knownRuntimeMode && !runtimeModeDeploySurfaceAllowed(contract, surface, runtimeMode)) {
        const replacement = firstDeploySurfaceRuntimeMode(contract, surface);
        if (replacement) write("runtimeMode", replacement);
        else write("runtimeMode", "");
      }
    }
    if (profileBackends.length) {
      const backend = String(member.backend || "").trim();
      if (backend && !profileBackends.includes(backend)) write("backend", "");
    }
    if (modelIds.length || options.strictEmptyModels) {
      const model = String(member.model || "").trim();
      if (model && !modelIds.includes(model)) write("model", "");
    }
    if (toolIds.length || options.strictEmptyTools) {
      const allowedTools = new Set(toolIds);
      const tools = normalizeStringList(member.tools).filter((tool) => allowedTools.has(tool));
      if (JSON.stringify(tools) !== JSON.stringify(normalizeStringList(member.tools))) write("tools", tools);
    }
    if (out !== member) changed = true;
    return out;
  });
  return changed ? next : members;
}

export function reconcileMobSettingsWithContract(settings, contract) {
  const source = mobSettingsForUi(settings);
  const backends = contractStringValues(contract?.mob_definition?.profile_backends);
  if (!backends.length) return settings;
  const normalizedChanged = JSON.stringify(source) !== JSON.stringify(settings || {});
  const backendDefault = String(source.backendDefault || "").trim();
  if (!backendDefault || backends.includes(backendDefault)) return normalizedChanged ? source : settings;
  return { ...source, backendDefault: "" };
}

export function reconcileAuthoringWithContract({
  members,
  skillRealms,
  schemas,
  deploySettings,
  mobSettings,
  flow,
  instances,
  edges,
  contract,
  modelCatalog,
  toolCatalog,
  contractLoaded = false,
} = {}) {
  const strictEmpty = !!contractLoaded;
  let nextMembers = reconcileMemberSkillRefs(
    members,
    skillRealms,
    { strictEmpty },
  );
  nextMembers = reconcileMemberSchemaRefs(
    nextMembers,
    schemas,
    { strictEmpty },
  );
  const nextDeploySettings = reconcileDeploySettingsWithContract(
    deploySettings,
    contract,
    modelCatalog,
    { strictEmptyModels: strictEmpty },
  );
  nextMembers = reconcileMembersWithContract(
    nextMembers,
    contract,
    nextDeploySettings,
    modelCatalog,
    toolCatalog,
    {
      strictEmptyModels: strictEmpty,
      strictEmptyTools: strictEmpty,
    },
  );
  const authoring = reconcileAuthoringForMembers({
    flow,
    instances,
    edges,
    mobSettings,
    previousMembers: members,
    members: nextMembers,
  });
  const nextMobSettings = reconcileMobSettingsWithContract(authoring.mobSettings, contract);
  return {
    members: nextMembers,
    deploySettings: nextDeploySettings,
    flow: authoring.flow,
    instances: authoring.instances,
    edges: authoring.edges,
    mobSettings: nextMobSettings,
    changed: nextMembers !== members
      || nextDeploySettings !== deploySettings
      || authoring.flow !== flow
      || authoring.instances !== instances
      || authoring.edges !== edges
      || nextMobSettings !== mobSettings,
  };
}

export function reconcileStringField(source, write, key, values) {
  const allowed = contractStringValues(values);
  if (!allowed.length) return;
  const value = String(source[key] || "").trim();
  if (value && !allowed.includes(value)) write(key, "");
}

export function contractValueAllowed(values, raw, { allowBlank = false } = {}) {
  const value = String(raw || "").trim();
  if (!value) return allowBlank;
  const allowed = contractStringValues(values);
  return allowed.length ? allowed.includes(value) : true;
}

export function catalogValueAllowed(values, raw, { allowBlank = true } = {}) {
  const value = String(raw || "").trim();
  if (!value) return allowBlank;
  const allowed = Array.isArray(values)
    ? values.map((candidate) => String(candidate || "").trim()).filter(Boolean)
    : [];
  return allowed.length ? allowed.includes(value) : true;
}

export function optionValueAllowed(options, raw, { allowBlank = false } = {}) {
  const value = String(raw || "").trim();
  if (!value) return allowBlank;
  const enabled = (Array.isArray(options) ? options : [])
    .filter((option) => option && option.disabled !== true)
    .map((option) => String(option.value || "").trim())
    .filter(Boolean);
  return enabled.length ? enabled.includes(value) : true;
}

export function skillIdSet(skillRealms) {
  const ids = new Set();
  for (const realm of skillRealms || []) {
    for (const skill of realm?.skills || []) {
      const id = String(skill?.id || "").trim();
      if (id) ids.add(id);
    }
  }
  return ids;
}

export function skillRealmsForDocument(members, skillRealms) {
  const selected = new Set();
  for (const member of members || []) {
    for (const skill of member?.skills || []) {
      const id = String(skill || "").trim();
      if (id) selected.add(id);
    }
  }
  if (!selected.size) return [];

  const out = [];
  const seen = new Set();
  for (const realm of skillRealms || []) {
    if (!realm || typeof realm !== "object") continue;
    const skills = [];
    for (const skill of realm.skills || []) {
      const id = String(skill?.id || "").trim();
      if (!id || !selected.has(id) || seen.has(id)) continue;
      seen.add(id);
      skills.push(skill);
    }
    if (!skills.length) continue;
    out.push({
      ...realm,
      skills,
      default: out.length === 0 ? !!realm.default : false,
    });
  }
  return out;
}

export function memberToolIndex(members) {
  return new Map((members || [])
    .filter((member) => member?.id)
    .map((member) => [member.id, new Set(normalizeStringList(member.tools))]));
}

export function reconcileToolScopesInSteps(steps, allowedForStep) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileToolScopesInStep(step, allowedForStep);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileToolScopesInStep(step, allowedForStep) {
  if (!step || typeof step !== "object") return step;
  let next = step;
  if (step.type === "member") {
    next = reconcileToolScopeObject(step, allowedForStep(step));
  }
  if (step.type === "repeat") {
    const nested = reconcileToolScopesInSteps(step.steps || [], allowedForStep);
    if (nested !== step.steps) next = { ...next, steps: nested };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileToolScopesInSteps(branch.steps || [], allowedForStep);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileToolScopesInSteps(step.fallback, allowedForStep)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    if (changed) next = { ...next, branches, fallback };
  }
  return next;
}

export function reconcileToolScopeObject(source, allowedTools) {
  if (!source || typeof source !== "object") return source;
  const allowed = allowedTools || new Set();
  const allowedToolsList = normalizeStringList(source.allowedTools || source.allowed_tools)
    .filter((tool) => allowed.has(tool));
  const blockedToolsList = normalizeStringList(source.blockedTools || source.blocked_tools)
    .filter((tool) => allowed.has(tool));
  const currentAllowed = normalizeStringList(source.allowedTools || source.allowed_tools);
  const currentBlocked = normalizeStringList(source.blockedTools || source.blocked_tools);
  if (
    JSON.stringify(allowedToolsList) === JSON.stringify(currentAllowed)
    && JSON.stringify(blockedToolsList) === JSON.stringify(currentBlocked)
    && !("allowed_tools" in source)
    && !("blocked_tools" in source)
  ) {
    return source;
  }
  const { allowed_tools: _allowedToolsSnake, blocked_tools: _blockedToolsSnake, ...rest } = source;
  return {
    ...rest,
    allowedTools: allowedToolsList,
    blockedTools: blockedToolsList,
  };
}

export function memberIdSet(members) {
  return new Set((members || []).map((member) => String(member?.id || "").trim()).filter(Boolean));
}

export function flowLaunchSourceSet(flow, members) {
  const ids = memberIdSet(members);
  collectVisualSteps(flow?.steps || [], (step) => {
    if (step?.type === "member" && step.id) ids.add(String(step.id));
  });
  return ids;
}

export function graphLaunchSourceSet(instances, members) {
  const ids = memberIdSet(members);
  for (const instance of instances || []) {
    if (instance?.id && instance.memberId && !instance.isGate && !instance.isTerminal) {
      ids.add(String(instance.id));
    }
  }
  return ids;
}

export function reconcileLaunchSourcesInSteps(steps, allowedSources) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileLaunchSourcesInStep(step, allowedSources);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileLaunchSourcesInStep(step, allowedSources) {
  if (!step || typeof step !== "object") return step;
  let next = reconcileLaunchSourceObject(step, allowedSources);
  if (step.type === "repeat") {
    const nested = reconcileLaunchSourcesInSteps(step.steps || [], allowedSources);
    if (nested !== step.steps) next = { ...next, steps: nested };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileLaunchSourcesInSteps(branch.steps || [], allowedSources);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileLaunchSourcesInSteps(step.fallback, allowedSources)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    if (changed) next = { ...next, branches, fallback };
  }
  return next;
}

export function reconcileLaunchSourceObject(source, allowedSources) {
  if (!source || typeof source !== "object") return source;
  const mode = launchModeFromAuthoringSource(source);
  if (!mode) return source;
  if (mode.kind !== "Fork" || !mode.from || allowedSources.has(mode.from)) return source;
  return {
    ...source,
    launchMode: { kind: "Fresh" },
  };
}

export function reconcileControlRolesInSteps(steps, memberIds) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileControlRolesInStep(step, memberIds);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileControlRolesInStep(step, memberIds) {
  if (!step || typeof step !== "object") return step;
  let next = reconcileControlRoleObject(step, memberIds);
  if (step.type === "repeat") {
    const nested = reconcileControlRolesInSteps(step.steps || [], memberIds);
    if (nested !== step.steps) next = { ...next, steps: nested };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileControlRolesInSteps(branch.steps || [], memberIds);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileControlRolesInSteps(step.fallback, memberIds)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    if (changed) next = { ...next, branches, fallback };
  }
  return next;
}

export function reconcileControlRoleObject(source, memberIds) {
  if (!source || typeof source !== "object") return source;
  const controllerRole = String(source.controllerRole || source.controllerMemberId || source.controlRole || source.joinRole || "").trim();
  if (!controllerRole || memberIds.has(controllerRole)) return source;
  const { controllerRole: _controllerRole, controllerMemberId: _controllerMemberId, controlRole: _controlRole, joinRole: _joinRole, ...rest } = source;
  return rest;
}

export function reconcileMobSettingsProfiles(settings, previousMembers, members) {
  const normalized = normalizeMobSettings(settings);
  const previousById = new Map((previousMembers || [])
    .filter((member) => member?.id)
    .map((member) => [member.id, profileName(member)]));
  const currentProfiles = new Set((members || []).map(profileName).filter(Boolean));
  const renameByProfile = new Map();
  for (const member of members || []) {
    if (!member?.id) continue;
    const previous = previousById.get(member.id);
    const current = profileName(member);
    if (previous && current && previous !== current) renameByProfile.set(previous, current);
  }
  const rewriteProfile = (value) => {
    const raw = String(value || "").trim();
    if (!raw) return "";
    const renamed = renameByProfile.get(raw) || raw;
    return currentProfiles.has(renamed) ? renamed : "";
  };
  const orchestrator = rewriteProfile(normalized.orchestrator);
  const roleWiring = [];
  const seen = new Set();
  for (const rule of normalized.roleWiring || []) {
    const a = rewriteProfile(rule.a);
    const b = rewriteProfile(rule.b);
    if (!a || !b) continue;
    const key = `${a}\u0000${b}`;
    if (seen.has(key)) continue;
    seen.add(key);
    roleWiring.push({ a, b });
  }
  if (
    orchestrator === normalized.orchestrator
    && JSON.stringify(roleWiring) === JSON.stringify(normalized.roleWiring || [])
  ) {
    return settings;
  }
  return {
    ...normalized,
    orchestrator,
    roleWiring,
  };
}

export function reconcileSchemaFieldReferences({ flow, edges, members, instances, schemaId, oldName, newName }) {
  const schema = String(schemaId || "").trim();
  const oldField = String(oldName || "").trim();
  const nextField = String(newName || "").trim();
  if (!schema || !oldField || oldField === nextField) {
    return { flow, edges };
  }
  const memberSchemaById = new Map((members || []).map((member) => [
    member.id,
    String(member.schema || "").trim(),
  ]));
  const flowStepSchemas = flowStepSchemaIndex(flow?.steps || [], memberSchemaById);
  const graphStepSchemas = new Map((instances || [])
    .filter((instance) => instance?.id && instance?.memberId)
    .map((instance) => [instance.id, memberSchemaById.get(instance.memberId) || ""]));
  const reconciledFlow = reconcileSchemaFieldReferencesInFlow(flow, flowStepSchemas, schema, oldField, nextField);
  const reconciledEdges = reconcileSchemaFieldReferencesInEdges(edges, graphStepSchemas, schema, oldField, nextField);
  return {
    flow: reconciledFlow,
    edges: reconciledEdges,
  };
}

export function reconcileInputParamReferences({ flow, edges, oldName, newName }) {
  const oldField = String(oldName || "").trim();
  const nextField = String(newName || "").trim();
  if (!oldField || oldField === nextField) {
    return { flow, edges };
  }
  return {
    flow: reconcileInputParamReferencesInFlow(flow, oldField, nextField),
    edges: reconcileInputParamReferencesInEdges(edges, oldField, nextField),
  };
}

export function reconcileInputParamReferencesInFlow(flow, oldName, newName) {
  if (!flow || typeof flow !== "object") return flow;
  const steps = reconcileInputParamReferencesInSteps(flow.steps || [], oldName, newName);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileInputParamReferencesInSteps(steps, oldName, newName) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileInputParamReferencesInStep(step, oldName, newName);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileInputParamReferencesInStep(step, oldName, newName) {
  if (!step || typeof step !== "object") return step;
  if (step.type === "repeat") {
    const cond = rewriteInputParamCondition(step.cond, oldName, newName);
    const until = rewriteInputParamConditionText(step.until, oldName, newName, cond);
    const nested = reconcileInputParamReferencesInSteps(step.steps || [], oldName, newName);
    if (cond === step.cond && until === step.until && nested === step.steps) return step;
    return { ...step, cond, until, steps: nested };
  }
  if (step.type === "branch") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const cond = rewriteInputParamCondition(branch.cond, oldName, newName);
      const condition = rewriteInputParamConditionText(branch.condition, oldName, newName, cond);
      const branchSteps = reconcileInputParamReferencesInSteps(branch.steps || [], oldName, newName);
      if (cond === branch.cond && condition === branch.condition && branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, cond, condition, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileInputParamReferencesInSteps(step.fallback, oldName, newName)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  if (step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileInputParamReferencesInSteps(branch.steps || [], oldName, newName);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    return changed ? { ...step, branches } : step;
  }
  return step;
}

export function rewriteInputParamCondition(cond, oldName, newName) {
  if (!cond || typeof cond !== "object") return cond;
  const isParam = cond.namespace === "params" || cond.stepId === "params" || cond.step_id === "params";
  if (!isParam || String(cond.field || "").trim() !== oldName) return cond;
  if (!newName) return {};
  return {
    ...cond,
    namespace: "params",
    stepId: "params",
    field: newName,
  };
}

export function rewriteInputParamConditionText(text, oldName, newName, preferredCond) {
  if (!text) return text;
  if (preferredCond && preferredCond.field) return editorConditionText(preferredCond);
  const parsed = parseEditorConditionText(text);
  if (!parsed || parsed.namespace !== "params" || parsed.field !== oldName) return text;
  if (!newName) return "";
  return editorConditionText({ ...parsed, field: newName });
}

export function reconcileInputParamReferencesInEdges(edges, oldName, newName) {
  let changed = false;
  const next = (edges || []).map((edge) => {
    const condition = normalizedEdgeCondition(edge);
    const path = String(condition?.path || "").trim();
    const parts = path.split(".").filter(Boolean);
    if (parts.length !== 2 || parts[0] !== "params" || parts[1] !== oldName) return edge;
    changed = true;
    if (!newName) {
      return { ...edge, cond: null, label: "" };
    }
    const nextPath = `params.${newName}`;
    const nextCond = { var: nextPath, op: condition.op || "", val: condition.val ?? "" };
    return {
      ...edge,
      cond: nextCond,
      label: edge.label && edge.label === conditionTextForPath(path, condition)
        ? conditionTextForPath(nextPath, nextCond)
        : edge.label,
    };
  });
  return changed ? next : edges;
}

export function reconcileConditionFieldAvailability({ flow, edges, members, instances, schemas }) {
  const schemaFields = schemaFieldNameIndex(schemas);
  const inputFields = inputParamNameSet(flow);
  const memberSchemaById = new Map((members || []).map((member) => [
    member.id,
    String(member.schema || "").trim(),
  ]));
  const flowStepSchemas = flowStepSchemaIndex(flow?.steps || [], memberSchemaById);
  const graphStepSchemas = new Map((instances || [])
    .filter((instance) => instance?.id && instance?.memberId)
    .map((instance) => [instance.id, memberSchemaById.get(instance.memberId) || ""]));
  return {
    flow: reconcileConditionAvailabilityInFlow(flow, flowStepSchemas, schemaFields, inputFields),
    edges: reconcileConditionAvailabilityInEdges(edges, graphStepSchemas, schemaFields, inputFields),
  };
}

export function schemaFieldNameIndex(schemas) {
  const out = new Map();
  for (const schema of schemas || []) {
    const id = String(schema?.id || "").trim();
    if (!id) continue;
    const fields = new Map();
    for (const field of schema.fields || []) {
      const name = String(field?.name || "").trim();
      if (name) fields.set(name, field);
    }
    out.set(id, fields);
  }
  return out;
}

export function inputParamNameSet(flow) {
  const out = new Map();
  collectVisualSteps(flow?.steps || [], (step) => {
    if (step?.type !== "input") return;
    for (const param of step.inputParams || []) {
      const name = String(param?.name || "").trim();
      if (name) out.set(name, param);
    }
  });
  return out;
}

export function reconcileConditionAvailabilityInFlow(flow, stepSchemas, schemaFields, inputFields) {
  if (!flow || typeof flow !== "object") return flow;
  const steps = reconcileConditionAvailabilityInSteps(flow.steps || [], stepSchemas, schemaFields, inputFields);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileConditionAvailabilityInSteps(steps, stepSchemas, schemaFields, inputFields) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileConditionAvailabilityInStep(step, stepSchemas, schemaFields, inputFields);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileConditionAvailabilityInStep(step, stepSchemas, schemaFields, inputFields) {
  if (!step || typeof step !== "object") return step;
  if (step.type === "repeat") {
    const hadCond = !!step.cond;
    const cond = clearUnavailableEditorCondition(step.cond, stepSchemas, schemaFields, inputFields);
    const until = clearUnavailableConditionText(step.until, stepSchemas, schemaFields, inputFields, hadCond ? cond : undefined);
    const nested = reconcileConditionAvailabilityInSteps(step.steps || [], stepSchemas, schemaFields, inputFields);
    if (cond === step.cond && until === step.until && nested === step.steps) return step;
    return { ...step, cond, until, steps: nested };
  }
  if (step.type === "branch") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const hadCond = !!branch.cond;
      const cond = clearUnavailableEditorCondition(branch.cond, stepSchemas, schemaFields, inputFields);
      const condition = clearUnavailableConditionText(branch.condition, stepSchemas, schemaFields, inputFields, hadCond ? cond : undefined);
      const branchSteps = reconcileConditionAvailabilityInSteps(branch.steps || [], stepSchemas, schemaFields, inputFields);
      if (cond === branch.cond && condition === branch.condition && branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, cond, condition, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileConditionAvailabilityInSteps(step.fallback, stepSchemas, schemaFields, inputFields)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  if (step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileConditionAvailabilityInSteps(branch.steps || [], stepSchemas, schemaFields, inputFields);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    return changed ? { ...step, branches } : step;
  }
  return step;
}

export function clearUnavailableEditorCondition(cond, stepSchemas, schemaFields, inputFields) {
  if (!cond || typeof cond !== "object") return cond;
  return editorConditionFieldAvailable(cond, stepSchemas, schemaFields, inputFields) ? cond : {};
}

export function clearUnavailableConditionText(text, stepSchemas, schemaFields, inputFields, preferredCond) {
  if (preferredCond !== undefined) {
    if (preferredCond && preferredCond.field) return editorConditionText(preferredCond);
    return "";
  }
  if (!text) return text;
  const parsed = parseEditorConditionText(text);
  if (!parsed) return text;
  return editorConditionFieldAvailable(parsed, stepSchemas, schemaFields, inputFields) ? text : "";
}

export function editorConditionFieldAvailable(cond, stepSchemas, schemaFields, inputFields) {
  if (!cond || typeof cond !== "object") return true;
  const field = String(cond.field || "").trim();
  if (!field) return true;
  if (cond.namespace === "params" || cond.stepId === "params" || cond.step_id === "params") {
    return conditionFieldValueAvailable(inputFields.get(field), cond);
  }
  const stepId = String(cond.stepId || cond.step_id || "").trim();
  if (!stepId) return true;
  return conditionFieldValueAvailable(schemaFieldForCondition(schemaFields, stepSchemas.get(stepId), field), cond);
}

export function reconcileConditionAvailabilityInEdges(edges, stepSchemas, schemaFields, inputFields) {
  let changed = false;
  const next = (edges || []).map((edge) => {
    const condition = normalizedEdgeCondition(edge);
    const path = String(condition?.path || "").trim();
    const parts = path.split(".").filter(Boolean);
    let available = true;
    if (parts.length === 2 && parts[0] === "params") {
      available = conditionFieldValueAvailable(inputFields.get(parts[1]), condition);
    } else if (parts.length === 3 && parts[0] === "steps") {
      available = conditionFieldValueAvailable(schemaFieldForCondition(schemaFields, stepSchemas.get(parts[1]), parts[2]), condition);
    }
    if (available) return edge;
    changed = true;
    return { ...edge, cond: null, label: "" };
  });
  return changed ? next : edges;
}

export function schemaHasField(schemaFields, schemaId, field) {
  return !!schemaFieldForCondition(schemaFields, schemaId, field);
}

export function schemaFieldForCondition(schemaFields, schemaId, field) {
  const id = String(schemaId || "").trim();
  const name = String(field || "").trim();
  if (!id || !name) return null;
  const fields = schemaFields.get(id);
  if (!fields) return null;
  if (fields instanceof Map) return fields.get(name) || null;
  return fields.has?.(name) ? { name } : null;
}

export function conditionFieldValueAvailable(field, cond) {
  if (!field) return false;
  const type = String(field.type || "").trim();
  if (type !== "enum") return true;
  const values = enumValuesForField(field).map(String);
  if (!values.length) return true;
  const raw = cond?.val ?? cond?.value;
  if (raw == null || String(raw).trim() === "") return true;
  return values.includes(String(raw));
}

export function flowStepSchemaIndex(steps, memberSchemaById, out = new Map()) {
  for (const step of steps || []) {
    if (!step || typeof step !== "object") continue;
    if (step.type === "member" && step.id) {
      out.set(step.id, memberSchemaById.get(step.role) || String(step.schema || "").trim());
    }
    if (step.type === "repeat") flowStepSchemaIndex(step.steps || [], memberSchemaById, out);
    if (step.type === "branch" || step.type === "parallel") {
      for (const branch of step.branches || []) flowStepSchemaIndex(branch.steps || [], memberSchemaById, out);
      flowStepSchemaIndex(step.fallback || [], memberSchemaById, out);
    }
  }
  return out;
}

export function reconcileSchemaFieldReferencesInFlow(flow, stepSchemas, schemaId, oldName, newName) {
  if (!flow || typeof flow !== "object") return flow;
  const steps = reconcileSchemaFieldReferencesInSteps(flow.steps || [], stepSchemas, schemaId, oldName, newName);
  return steps === flow.steps ? flow : { ...flow, steps };
}

export function reconcileSchemaFieldReferencesInSteps(steps, stepSchemas, schemaId, oldName, newName) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileSchemaFieldReferencesInStep(step, stepSchemas, schemaId, oldName, newName);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileSchemaFieldReferencesInStep(step, stepSchemas, schemaId, oldName, newName) {
  if (!step || typeof step !== "object") return step;
  if (step.type === "repeat") {
    const cond = rewriteEditorCondition(step.cond, stepSchemas, schemaId, oldName, newName);
    const until = rewriteConditionTextReference(step.until, stepSchemas, schemaId, oldName, newName);
    const nested = reconcileSchemaFieldReferencesInSteps(step.steps || [], stepSchemas, schemaId, oldName, newName);
    if (cond === step.cond && until === step.until && nested === step.steps) return step;
    return {
      ...step,
      cond,
      until,
      steps: nested,
    };
  }
  if (step.type === "branch") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const cond = rewriteEditorCondition(branch.cond, stepSchemas, schemaId, oldName, newName);
      const condition = rewriteConditionTextReference(branch.condition, stepSchemas, schemaId, oldName, newName, cond);
      const branchSteps = reconcileSchemaFieldReferencesInSteps(branch.steps || [], stepSchemas, schemaId, oldName, newName);
      if (cond === branch.cond && condition === branch.condition && branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, cond, condition, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileSchemaFieldReferencesInSteps(step.fallback, stepSchemas, schemaId, oldName, newName)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  if (step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileSchemaFieldReferencesInSteps(branch.steps || [], stepSchemas, schemaId, oldName, newName);
      if (branchSteps === branch.steps) return branch;
      changed = true;
      return { ...branch, steps: branchSteps };
    });
    return changed ? { ...step, branches } : step;
  }
  return step;
}

export function rewriteEditorCondition(cond, stepSchemas, schemaId, oldName, newName) {
  if (!cond || typeof cond !== "object") return cond;
  if (cond.namespace === "params" || cond.stepId === "params") return cond;
  const stepId = String(cond.stepId || cond.step_id || "").trim();
  if (!stepId || stepSchemas.get(stepId) !== schemaId || String(cond.field || "").trim() !== oldName) return cond;
  if (!newName) return {};
  const next = { ...cond, field: newName || "", val: newName ? (cond.val ?? "") : "" };
  return next;
}

export function rewriteConditionTextReference(text, stepSchemas, schemaId, oldName, newName, preferredCond) {
  if (!text) return text;
  if (preferredCond && preferredCond.field) return editorConditionText(preferredCond);
  const parsed = parseEditorConditionText(text);
  if (!parsed || parsed.namespace === "params") return text;
  if (stepSchemas.get(parsed.stepId) !== schemaId || parsed.field !== oldName) return text;
  if (!newName) return "";
  return editorConditionText({ ...parsed, field: newName });
}

export function parseEditorConditionText(text) {
  const raw = String(text || "").trim();
  const params = /^params\.([A-Za-z0-9_.-]+)\s*(==|>|<)\s*(.+)$/.exec(raw);
  if (params) {
    return {
      namespace: "params",
      stepId: "params",
      field: params[1],
      op: params[2],
      val: params[3].trim().replace(/^["']|["']$/g, ""),
    };
  }
  const steps = /^steps\.([A-Za-z0-9_.-]+)\.([A-Za-z0-9_.-]+)\s*(==|>|<)\s*(.+)$/.exec(raw);
  if (!steps) return null;
  return {
    namespace: "steps",
    stepId: steps[1],
    field: steps[2],
    op: steps[3],
    val: steps[4].trim().replace(/^["']|["']$/g, ""),
  };
}

export function editorConditionText(cond) {
  if (!cond || !cond.stepId || !cond.field) return "";
  const op = cond.op || cond.operator || "";
  if (!op) return "";
  const val = cond.val ?? cond.value ?? "";
  if (cond.namespace === "params" || cond.stepId === "params") {
    return `params.${cond.field} ${op} ${JSON.stringify(String(val))}`;
  }
  return `steps.${cond.stepId}.${cond.field} ${op} ${JSON.stringify(String(val))}`;
}

export function conditionValueLiteral(value) {
  return /^(true|false|-?\d+(\.\d+)?)$/.test(String(value ?? ""))
    ? String(value ?? "")
    : JSON.stringify(String(value ?? ""));
}

export function reconcileSchemaFieldReferencesInEdges(edges, stepSchemas, schemaId, oldName, newName) {
  let changed = false;
  const next = (edges || []).map((edge) => {
    const condition = normalizedEdgeCondition(edge);
    const path = String(condition?.path || "").trim();
    const parts = path.split(".").filter(Boolean);
    if (parts.length !== 3 || parts[0] !== "steps") return edge;
    const stepId = parts[1];
    const field = parts[2];
    if (stepSchemas.get(stepId) !== schemaId || field !== oldName) return edge;
    changed = true;
    if (!newName) {
      return { ...edge, cond: null, label: "" };
    }
    const nextPath = `steps.${stepId}.${newName}`;
    const nextCond = { var: nextPath, op: condition.op || "", val: condition.val ?? "" };
    return {
      ...edge,
      cond: nextCond,
      label: edge.label && edge.label === conditionTextForPath(path, condition)
        ? conditionTextForPath(nextPath, nextCond)
        : edge.label,
    };
  });
  return changed ? next : edges;
}

export function conditionTextForPath(path, condition) {
  const op = condition.op || "";
  return op ? `${path} ${op} ${JSON.stringify(String(condition.val ?? ""))}` : "";
}

export function reconcileMemberSchemasInSteps(steps, memberById) {
  let changed = false;
  const next = (steps || []).map((step) => {
    const reconciled = reconcileMemberSchemaInStep(step, memberById);
    if (reconciled !== step) changed = true;
    return reconciled;
  });
  return changed ? next : steps;
}

export function reconcileMemberSchemaInStep(step, memberById) {
  if (!step || typeof step !== "object") return step;
  if (step.type === "member") {
    const member = memberById.get(step.role);
    if (!member) return step;
    const memberSchema = String(member.schema || "").trim();
    const stepSchema = String(step.schema || "").trim();
    const expected = String(step.expectedSchemaRef || step.expected_schema_ref || "").trim();
    if (!memberSchema) {
      if (!stepSchema && !expected && !("expected_schema_ref" in step)) return step;
      const { schema, expectedSchemaRef, expected_schema_ref, ...rest } = step;
      return rest;
    }
    let next = step;
    if (stepSchema !== memberSchema) {
      next = { ...next, schema: memberSchema };
      if (expected && stepSchema && expected === `schemas/${stepSchema}.json`) {
        next.expectedSchemaRef = `schemas/${memberSchema}.json`;
      }
    }
    if ("expected_schema_ref" in next) {
      const { expected_schema_ref, ...rest } = next;
      next = rest;
    }
    return next;
  }
  if (step.type === "repeat") {
    const nested = reconcileMemberSchemasInSteps(step.steps || [], memberById);
    return nested === step.steps ? step : { ...step, steps: nested };
  }
  if (step.type === "branch" || step.type === "parallel") {
    let changed = false;
    const branches = (step.branches || []).map((branch) => {
      const branchSteps = reconcileMemberSchemasInSteps(branch.steps || [], memberById);
      if (branchSteps !== branch.steps) changed = true;
      return branchSteps === branch.steps ? branch : { ...branch, steps: branchSteps };
    });
    const fallback = Array.isArray(step.fallback)
      ? reconcileMemberSchemasInSteps(step.fallback, memberById)
      : step.fallback;
    if (fallback !== step.fallback) changed = true;
    return changed ? { ...step, branches, fallback } : step;
  }
  return step;
}
