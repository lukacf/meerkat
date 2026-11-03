// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the tool-skill-access functions move byte-verbatim as plain JS,
// and their `spec = {}` / destructured `= {}` parameter defaults raise
// TS2339/TS2353 under .ts semantics (e.g. addInlineSkillToRealms's
// spec.label and stepToolScopeState's options shape). Source-contract pins
// this exact text, so suppression must live at file level, not in the moved
// bodies. Resolution/linkage stays guarded behaviorally: the projection
// suite and export-keys test load the bundle and exercise these functions,
// so a missed import or re-export still fails the gate as a ReferenceError.
//
// Tool and skill access domain logic for the Flow Editor controller plane.
// Moved verbatim from the controller.js tool-skill-access range: slug/profile
// naming, tool catalog ref normalization and availability, member tool/skill
// access state projections and patches, step tool-scope state and patches,
// and inline skill authoring against Meta skill realms.
//
// stepToolScopeState reaches basicEditorViewState, which went through the
// lazy _residue-bridge until editors/basic-editor.ts landed in S11; it is
// now a relative import (runtime-only — no module-init cross-calls).
import { basicEditorViewState } from "../editors/basic-editor";
import { normalizeStringList } from "../shared/normalize";
import { agentAccessViewForState } from "../views/view-config";

export function slug(value, fallback) {
  const out = String(value || fallback || "mobpack")
    .toLowerCase()
    .replace(/[^a-z0-9_ -]+/g, "")
    .trim()
    .replace(/[\s-]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return out || fallback || "mobpack";
}

export function profileName(member) {
  return slug(member?.name || member?.role || member?.id || "member", "member");
}

export function normalizeToolRef(raw, catalog) {
  const id = String(raw || "").trim();
  if (!id) return "";
  const entries = Array.isArray(catalog) ? catalog : [];
  const entry = (entries || []).find((tool) => tool.id === id);
  if (entry && toolCatalogEntryAvailable(entry)) return id;
  return "";
}

export function toolCatalogEntryAvailability(tool) {
  const availability = tool?.runtimeAvailability || tool?.runtime_availability || null;
  return availability && typeof availability === "object" ? availability : null;
}

export function toolCatalogEntryAvailable(tool) {
  const availability = toolCatalogEntryAvailability(tool);
  return availability?.available === false ? false : true;
}

export function normalizeSkillId(raw) {
  return String(raw || "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_.-]+/g, ".")
    .replace(/^[._-]+|[._-]+$/g, "")
    .replace(/\.{2,}/g, ".");
}

export function skillIdsFromRealms(realms) {
  const ids = new Set();
  for (const realm of realms || []) {
    for (const skill of realm?.skills || []) {
      if (skill?.id) ids.add(String(skill.id));
    }
  }
  return ids;
}

export function addInlineSkillToRealms(realms, spec = {}, accessView = null) {
  const view = agentAccessViewForState(accessView);
  const nextRealms = JSON.parse(JSON.stringify(realms || []));
  const label = String(spec.label || spec.id || "").trim();
  if (!label) throw new Error(view.skillInlineMissingLabelError);
  const content = String(spec.content || "").trim();
  if (!content) throw new Error(view.skillInlineMissingContentError);
  const used = skillIdsFromRealms(nextRealms);
  const explicitId = String(spec.id || "").trim();
  const identityText = explicitId || label;
  if (!normalizeSkillId(identityText)) throw new Error(view.skillInlineInvalidIdError);
  const rawId = explicitId || (label.includes(".") ? label : `mob.${label}`);
  const baseId = normalizeSkillId(rawId);
  if (!baseId) throw new Error(view.skillInlineInvalidIdError);
  let id = baseId;
  let index = 2;
  while (used.has(id)) id = `${baseId}.${index++}`;
  let realm = nextRealms.find((candidate) => candidate?.id === view.inlineSkillRealmId);
  if (!realm) {
    realm = {
      id: view.inlineSkillRealmId,
      label: view.inlineSkillRealmLabel,
      source: view.inlineSkillRealmSource,
      default: nextRealms.length === 0,
      skills: [],
    };
    nextRealms.unshift(realm);
  }
  if (!Array.isArray(realm.skills)) realm.skills = [];
  realm.skills.push({
    id,
    label,
    source: view.inlineSkillSource,
    content,
    desc: spec.desc || view.inlineSkillDefaultDescription,
  });
  return { id, skillRealms: nextRealms };
}

export function memberToolAccessPatch(member, raw, toolCatalog, accessView = null) {
  const view = agentAccessViewForState(accessView);
  const id = normalizeToolRef(raw, toolCatalog);
  if (!id) {
    return {
      ok: false,
      id: "",
      error: raw ? view.toolInvalidError : "",
      patch: null,
    };
  }
  const tools = normalizeStringList(member?.tools);
  if (tools.includes(id)) {
    return { ok: true, id, alreadySelected: true, patch: null };
  }
  return { ok: true, id, alreadySelected: false, patch: { tools: [...tools, id] } };
}

export function memberToolRemovePatch(member, toolId) {
  const id = String(toolId || "").trim();
  if (!id) return { ok: false, id: "", patch: null };
  const tools = normalizeStringList(member?.tools);
  return { ok: true, id, patch: { tools: tools.filter((candidate) => candidate !== id) } };
}

export function memberToolAccessState(member, toolCatalog = [], accessView = null) {
  const view = agentAccessViewForState(accessView);
  const catalog = Array.isArray(toolCatalog) ? toolCatalog.filter((tool) => tool?.id) : [];
  const metaById = new Map(catalog.map((tool) => [String(tool.id), tool]));
  const selectedTools = normalizeStringList(member?.tools);
  const selectedSet = new Set(selectedTools);
  const catalogSet = new Set(catalog.map((tool) => String(tool.id)));
  const toolRow = (id) => {
    const meta = metaById.get(id) || null;
    const unavailable = !catalogSet.has(id);
    const runtimeAvailability = toolCatalogEntryAvailability(meta);
    const runtimeUnavailable = runtimeAvailability?.available === false;
    const reason = unavailable
      ? view.toolInvalidError
      : (runtimeUnavailable ? (runtimeAvailability.reason || view.toolInvalidError) : "");
    return {
      id,
      name: id,
      unavailable: unavailable || runtimeUnavailable,
      reason,
      description: reason || meta?.desc || view.toolMissingDescription,
      meta,
      runtimeAvailability,
      className: `tool-row${(unavailable || runtimeUnavailable) ? " tool-row--invalid" : ""}`,
      removeLabel: view.toolRemoveLabel,
    };
  };
  const addableRow = (tool) => {
    const id = String(tool.id);
    const label = tool.label || id;
    const desc = tool.desc || id;
    return {
      id,
      value: id,
      label,
      description: desc,
      optionLabel: `${label} — ${desc}`,
      disabled: !toolCatalogEntryAvailable(tool),
      meta: tool,
    };
  };
  return {
    selectedTools,
    title: view.toolTitle,
    hint: view.toolHint,
    rows: selectedTools.map(toolRow),
    addableRows: catalog
      .filter((tool) => !selectedSet.has(String(tool.id)))
      .map(addableRow),
    addSelectValue: "",
    addSelectPlaceholder: view.toolAddSelectPlaceholder,
    sourceLabel: view.toolSourceLabel,
    sourcePlaceholder: view.toolSourcePlaceholder,
    addButtonLabel: view.toolAddButtonLabel,
    emptyToolError: view.toolEmptyError,
    authoringOperationUnavailableError: view.authoringOperationUnavailableError,
  };
}

export function stepToolScopeState({ member, selected, mode = "member", toolCatalog = [], basicView = null } = {}) {
  const view = basicEditorViewState(basicView);
  const catalog = Array.isArray(toolCatalog) ? toolCatalog.filter((tool) => tool?.id) : [];
  const catalogIds = Array.from(new Set(catalog.map((tool) => String(tool.id).trim()).filter(Boolean)));
  const selectedTools = Array.from(new Set(normalizeStringList(selected)));
  const memberToolIds = validMemberToolIds(member, catalogIds);
  const validToolIds = mode === "catalog" ? catalogIds : memberToolIds;
  const validToolSet = new Set(validToolIds);
  const addable = validToolIds.filter((id) => !selectedTools.includes(id));
  const metaById = new Map(catalog.map((tool) => [String(tool.id), tool]));
  const rowFor = (id) => {
    const meta = metaById.get(id) || null;
    const unavailable = !validToolSet.has(id);
    const reason = unavailable
      ? (mode === "catalog" ? view.toolScopeNotInCatalogReason : view.toolScopeNotEnabledReason)
      : "";
    return {
      id,
      name: id,
      meta,
      unavailable,
      reason,
      className: `tool-row${unavailable ? " tool-row--invalid" : ""}`,
      description: unavailable ? reason : (meta?.desc || view.toolScopeToolDescriptionFallback),
      removeLabel: view.toolScopeRemoveLabel,
    };
  };
  const optionFor = (id) => {
    const meta = metaById.get(id) || null;
    const label = meta?.label || id;
    const desc = meta?.desc || id;
    return {
      id,
      value: id,
      label,
      description: desc,
      optionLabel: `${label} — ${desc}`,
      meta,
    };
  };
  return {
    selectedTools,
    addable,
    addableRows: addable.map(optionFor),
    rows: selectedTools.map(rowFor),
    addSelectValue: "",
    addSelectPlaceholder: mode === "member" && !member
      ? view.toolScopeSelectMemberPlaceholder
      : (mode === "catalog" ? view.toolScopeBlockCatalogPlaceholder : view.toolScopeAddProfilePlaceholder),
    disabled: (mode === "member" && !member) || addable.length === 0,
  };
}

export function validMemberToolIds(member, catalogIds) {
  const catalogIdSet = new Set(catalogIds || []);
  return Array.from(new Set(normalizeStringList(member?.tools).filter((id) => catalogIdSet.has(id))));
}

export function validStepToolSet({ member, mode = "member", toolCatalog = [] } = {}) {
  const catalog = Array.isArray(toolCatalog) ? toolCatalog.filter((tool) => tool?.id) : [];
  const catalogIds = Array.from(new Set(catalog.map((tool) => String(tool.id).trim()).filter(Boolean)));
  if (mode === "catalog") return new Set(catalogIds);
  return new Set(validMemberToolIds(member, catalogIds));
}

export function normalizeStepToolScopeList(tools, options = {}) {
  const validTools = validStepToolSet(options);
  return Array.from(new Set(normalizeStringList(tools).filter((tool) => validTools.has(tool))));
}

export function stepToolScopeAddPatch(selected, raw, options = {}) {
  const id = String(raw || "").trim();
  const field = options.field || "allowedTools";
  if (!id) return { ok: false, id: "", patch: null };
  const state = stepToolScopeState({ ...options, selected });
  if (!state.addable.includes(id)) {
    return { ok: false, id, patch: null };
  }
  return {
    ok: true,
    id,
    patch: { [field]: [...state.selectedTools, id] },
  };
}

export function stepToolScopeRemovePatch(selected, raw, options = {}) {
  const id = String(raw || "").trim();
  const field = options.field || "allowedTools";
  if (!id) return { ok: false, id: "", patch: null };
  const selectedTools = Array.from(new Set(normalizeStringList(selected)));
  return {
    ok: true,
    id,
    patch: { [field]: selectedTools.filter((candidate) => candidate !== id) },
  };
}

export function memberSkillTogglePatch(member, skillId, skillRealms = []) {
  const id = String(skillId || "").trim();
  if (!id) return { ok: false, id: "", patch: null };
  const skills = normalizeStringList(member?.skills);
  const selected = skills.includes(id);
  if (!selected && !skillIdsFromRealms(skillRealms).has(id)) {
    return { ok: false, id, patch: null };
  }
  return {
    ok: true,
    id,
    selected: !selected,
    patch: { skills: selected ? skills.filter((candidate) => candidate !== id) : [...skills, id] },
  };
}

export function memberSkillRemovePatch(member, skillId) {
  const id = String(skillId || "").trim();
  if (!id) return { ok: false, id: "", patch: null };
  const skills = normalizeStringList(member?.skills);
  return { ok: true, id, patch: { skills: skills.filter((candidate) => candidate !== id) } };
}

export function memberInlineSkillPatch(member, realms, spec = {}, accessView = null) {
  const view = agentAccessViewForState(accessView);
  const result = addInlineSkillToRealms(realms, spec, accessView);
  const skills = normalizeStringList(member?.skills);
  return {
    ...result,
    realmId: view.inlineSkillRealmId,
    patch: { skills: skills.includes(result.id) ? skills : [...skills, result.id] },
  };
}

export function memberSkillAccessState({ member, skillRealms, realmId = "", inlineOpen = false, accessView = null } = {}) {
  const view = agentAccessViewForState(accessView);
  const realms = Array.isArray(skillRealms) ? skillRealms.filter((realm) => realm?.id) : [];
  const defaultRealm = realms.find((realm) => realm.default) || realms[0] || null;
  const selectedRealm = realms.find((realm) => realm.id === realmId) || defaultRealm;
  const selectedSkillIds = normalizeStringList(member?.skills);
  const selectedSet = new Set(selectedSkillIds);
  const byId = new Map();
  for (const sourceRealm of realms) {
    for (const skill of sourceRealm.skills || []) {
      const id = String(skill?.id || "").trim();
      if (!id || byId.has(id)) continue;
      byId.set(id, { ...skill, id, realm: sourceRealm });
    }
  }
  const skillRows = (selectedRealm?.skills || [])
    .filter((skill) => String(skill?.id || "").trim())
    .map((skill) => {
      const id = String(skill.id).trim();
      const selected = selectedSet.has(id);
      return {
        id,
        selected,
        className: `skill-row${selected ? " is-on" : ""}`,
        checkLabel: selected ? view.skillSelectedCheckLabel : "",
        name: id,
        desc: skill.desc || skill.path || skill.source || view.skillDefaultDescription,
        skill,
      };
    });
  const selectedOutsideRealm = selectedSkillIds
    .map((id) => byId.get(id))
    .filter((skill) => skill && skill.realm?.id !== selectedRealm?.id)
    .map((skill) => ({
      id: skill.id,
      realmId: skill.realm?.id || "",
      realmLabel: skill.realm?.label || skill.realm?.id || "",
      className: "skill-chip",
      title: skill.realm?.label || skill.realm?.id || "",
      label: skill.id,
      detail: skill.realm?.label || skill.realm?.id || "",
      removeLabel: view.skillRemoveLabel,
    }));
  const unavailableSelected = selectedSkillIds
    .filter((id) => !byId.has(id))
    .map((id) => ({
      id,
      className: "skill-chip is-invalid",
      label: id,
      removeLabel: view.skillRemoveLabel,
    }));
  return {
    sectionTitle: view.skillSectionTitle,
    inlineToggleLabel: inlineOpen ? view.skillInlineCancelLabel : view.skillInlineOpenLabel,
    hint: view.skillHint,
    inlineLabelPlaceholder: view.skillInlineLabelPlaceholder,
    inlineContentRows: view.skillInlineContentRows,
    inlineContentPlaceholder: view.skillInlineContentPlaceholder,
    inlineCreateHint: view.skillInlineCreateHint,
    inlineAddLabel: view.skillInlineAddLabel,
    inlineErrorFallback: view.skillInlineErrorFallback,
    authoringOperationUnavailableError: view.authoringOperationUnavailableError,
    noRealmsMessage: view.skillNoRealmsMessage,
    realmLabel: view.skillRealmLabel,
    hasRealms: realms.length > 0,
    realmId: selectedRealm?.id || "",
    realmOptions: realms.map((realm) => ({
      id: realm.id,
      label: `${realm.label || realm.id}${realm.default ? view.skillDefaultRealmSuffix : ""}`,
    })),
    skillRows,
    selectedOutsideRealm,
    unavailableSelected,
    unavailableHeading: view.skillUnavailableHeading,
    outsideRealmHeading: view.skillOutsideRealmHeading,
  };
}
