// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the agent-editor functions move byte-verbatim as plain JS, and
// their destructured `= {}` parameter defaults (agentListState,
// agentSelectionState, agentDefaultSelectionProjection,
// agentEditorControlState, schemaEditorControlState) raise TS2339 under .ts
// semantics. Source-contract pins this exact text, so suppression must live
// at file level, not in the moved bodies.
// Resolution/linkage stays guarded behaviorally: the projection suite and
// export-keys test load the bundle and exercise these functions, so a missed
// import or re-export still fails the gate as a ReferenceError.
//
// Agent editor control plane for the Flow Editor. Moved verbatim from the
// controller.js agent-editor range: agent list/selection states, the agent
// editor control state with its source provenance
// sections, the agent definition catalog/add controls, the *ErrorState
// quartet, and schemaEditorControlState. agentListState is re-homed here
// from the residue's view-state range (extraction design S12; its
// source-contract block was re-anchored in S3). normalizeAgentDefinitionRows
// (needed by sourceDefinitionRefRows) is facade-internal, so it moved early
// to its design-destined home in registry/flow-registry.ts (S15).
import {
  profileBackendOptions,
  profileBindingOptions,
  profileBindingRestriction,
  runtimeModeOptions,
} from "../contract/options";
import { normalizeAgentDefinitionRows } from "../registry/flow-registry";
import { operationErrorText } from "../rpc/client";
import {
  agentDetailViewForState,
  agentViewForState,
  roleAccentStyle,
  schemaViewForState,
} from "../views/view-config";
import { graphTemplateText } from "./graph-editor";

export function agentListState({ members = [], instances = [], schemas = [], selection = null, agentView = null } = {}) {
  const sourceMembers = Array.isArray(members) ? members : [];
  const sourceInstances = Array.isArray(instances) ? instances : [];
  const sourceSchemas = Array.isArray(schemas) ? schemas : [];
  const view = agentViewForState(agentView);
  const memberRows = sourceMembers.map((member) => {
    const placedCount = sourceInstances.filter((instance) => instance?.memberId === member.id).length;
    const selected = selection?.kind === "agent" && selection.id === member.id;
    const placedLabel = placedCount === 0
      ? view.memberPlacedEmptyLabel
      : graphTemplateText(view.memberPlacedCountTemplate, { count: placedCount });
    const isUnplaced = placedCount === 0;
    return {
      id: member.id,
      name: member.name,
      role: member.role,
      model: member.model,
      member,
      selected,
      itemClass: `agents-list__item${selected ? " is-selected" : ""}`,
      bulletRole: member.role,
      bulletStyle: roleAccentStyle(member.role),
      subLabel: graphTemplateText(view.memberSubLabelTemplate, {
        role: member.role,
        model: member.model,
      }),
      placedCount,
      placedLabel,
      isUnplaced,
      placedClass: `agents-list__placed${isUnplaced ? " is-zero" : ""}`,
    };
  });
  const schemaRows = sourceSchemas.map((schema) => {
    const fieldCount = Array.isArray(schema.fields) ? schema.fields.length : 0;
    const usedCount = sourceMembers.filter((member) => member?.schema === schema.id).length;
    const selected = selection?.kind === "schema" && selection.id === schema.id;
    const fieldLabel = graphTemplateText(
      fieldCount === 1 ? view.schemaFieldSingularTemplate : view.schemaFieldPluralTemplate,
      { count: fieldCount },
    );
    const usageLabel = graphTemplateText(view.schemaUsageLabelTemplate, { count: usedCount });
    return {
      id: schema.id,
      schema,
      selected,
      itemClass: `agents-list__item${selected ? " is-selected" : ""}`,
      bulletRole: "schema",
      bulletStyle: roleAccentStyle("schema"),
      fieldCount,
      fieldLabel,
      usedCount,
      usageLabel,
      subLabel: [fieldLabel, usageLabel].filter(Boolean).join(view.sidebarSubLabelSeparator),
    };
  });
  return {
    agentsHeading: view.agentsHeading,
    schemasHeading: view.schemasHeading,
    addSchemaLabel: view.addSchemaLabel,
    authoringOperationUnavailableError: view.authoringOperationUnavailableError,
    schemaAddFallbackError: view.schemaAddFallbackError,
    emptyTitle: view.emptyTitle,
    emptyLines: view.emptyLines,
    missingSchemaLabel: view.missingSchemaLabel,
    missingAgentLabel: view.missingAgentLabel,
    memberCount: memberRows.length,
    schemaCount: schemaRows.length,
    memberRows,
    schemaRows,
  };
}

export function agentSelectionState({ selection = null, members = [], schemas = [], agentView = null } = {}) {
  const view = agentViewForState(agentView);
  const emptyState = {
    title: view.emptyTitle,
    lines: view.emptyLines,
  };
  const base = {
    emptyState,
    missingSchemaLabel: view.missingSchemaLabel,
    missingAgentLabel: view.missingAgentLabel,
  };
  if (!selection) return { ...base, kind: "empty", member: null, schema: null, missing: false };
  if (selection.kind === "schema") {
    const schema = (Array.isArray(schemas) ? schemas : []).find((candidate) => candidate.id === selection.id) || null;
    return { ...base, kind: "schema", member: null, schema, missing: !schema };
  }
  if (selection.kind === "agent") {
    const member = (Array.isArray(members) ? members : []).find((candidate) => candidate.id === selection.id) || null;
    return { ...base, kind: "agent", member, schema: null, missing: !member };
  }
  return { ...base, kind: String(selection.kind || ""), member: null, schema: null, missing: true };
}

export function agentListSelectionProjection(kind, id) {
  const selectionKind = String(kind || "").trim();
  const selectionId = String(id || "").trim();
  if (!selectionId || (selectionKind !== "agent" && selectionKind !== "schema")) return null;
  return { kind: selectionKind, id: selectionId };
}

export function agentDefaultSelectionProjection({
  selection = null,
  members = [],
  schemas = [],
  agentView = null,
} = {}) {
  const current = agentSelectionState({ selection, members, schemas, agentView });
  if ((current.kind === "agent" || current.kind === "schema") && !current.missing) {
    return selection;
  }
  const firstMember = (Array.isArray(members) ? members : []).find((member) => member?.id);
  if (firstMember) return agentListSelectionProjection("agent", firstMember.id);
  const firstSchema = (Array.isArray(schemas) ? schemas : []).find((schema) => schema?.id);
  if (firstSchema) return agentListSelectionProjection("schema", firstSchema.id);
  return null;
}

export function agentEditorControlState({ member, instances = [], schemas = [], contract, deploySettings, modelCatalog = [], agentView = null, agentDetailView = null } = {}) {
  const agentUiView = agentViewForState(agentView);
  const view = agentDetailViewForState(agentDetailView);
  const placedAt = (Array.isArray(instances) ? instances : []).filter((instance) => instance?.memberId === member?.id);
  const placedCount = placedAt.length;
  const memberName = String(member?.name || member?.id || "agent");
  const instanceNoun = placedCount === 1 ? view.instanceSingular : view.instancePlural;
  const cellNoun = placedCount === 1 ? view.cellSingular : view.cellPlural;
  const schema = (Array.isArray(schemas) ? schemas : []).find((candidate) => candidate.id === member?.schema) || null;
  const profileBinding = typeof member?.profileBinding === "string"
    ? member.profileBinding
    : (member?.realmProfile ? "realm_profile" : "");
  const realmProfileRestriction = profileBindingRestriction(contract, "realm_profile");
  const bindingOptions = [
    { value: "", label: view.missingProfileBindingLabel, disabled: false, reason: "" },
    ...profileBindingOptions(contract, profileBinding),
  ];
  const runtimeMode = typeof member?.runtimeMode === "string" ? member.runtimeMode : "";
  const runtimeOptions = [
    { value: "", label: view.missingRuntimeModeLabel, disabled: false, reason: "" },
    ...runtimeModeOptions(contract, deploySettings, runtimeMode),
  ];
  const backendValue = String(member?.backend || "");
  const backendOptions = profileBackendOptions(
    contract,
    backendValue,
    true,
    view.backendDefinitionDefaultLabel,
  );
  const schemaOptions = [
    { value: "", label: view.schemaNoneLabel, schema: null },
    ...(Array.isArray(schemas) ? schemas : [])
      .filter((candidate) => candidate?.id)
      .map((candidate) => ({ value: candidate.id, label: candidate.id, schema: candidate })),
  ];
  const schemaPreviewRows = (Array.isArray(schema?.fields) ? schema.fields : [])
    .map((field) => ({
      id: field.id,
      name: field.name,
      type: field.type,
      required: !!field.required,
      requiredLabel: field.required ? view.schemaRequiredLabel : "",
    }));
  const modelOptions = (Array.isArray(modelCatalog) ? modelCatalog : [])
    .filter((model) => model?.id)
    .map((model) => ({
      value: model.id,
      label: `${model.label || model.id}${model.vendor ? ` · ${model.vendor}` : ""}`,
      model,
    }));
  if (member?.model && !modelOptions.some((model) => model.value === member.model)) {
    modelOptions.push({ value: member.model, label: member.model, model: null });
  }
  return {
    placedAt,
    placedCount,
    authoringOperationUnavailableError: agentUiView.authoringOperationUnavailableError,
    memberUpdateFallbackError: agentUiView.memberUpdateFallbackError,
    toolUpdateFallbackError: agentUiView.toolUpdateFallbackError,
    schemaAssignmentFallbackError: agentUiView.schemaAssignmentFallbackError,
    eyebrow: [view.agentEyebrowPrefix, member?.role || ""].filter(Boolean).join(" · "),
    idLine: `${member?.id || ""} · ${view.usedInLabel} ${placedCount} ${instanceNoun}`,
    deleteLabel: view.deleteLabel,
    deleteCancelLabel: view.deleteCancelLabel,
    deleteNeedsConfirmation: placedCount > 0,
    deleteConfirmMessage: placedCount > 0
      ? `${view.deleteConfirmIntro} "${memberName}"? ${view.deleteConfirmPlacedPrefix} ${placedCount} ${cellNoun} - ${view.deleteConfirmCellsSuffix}`
      : "",
    usageTitle: `${view.usageTitlePrefix} · ${placedCount}`,
    emptyUsageHint: view.emptyUsageHint,
    usageRows: placedAt.map((instance) => ({
      id: instance.id,
      cellLabel: `cell (${Number(instance.col || 0) + 1},${Number(instance.row || 0) + 1})`,
      laneLabel: instance.lane || "—",
      instance,
    })),
    identityTitle: view.identityTitle,
    profileBindingLabel: view.profileBindingLabel,
    realmProfileLabel: view.realmProfileLabel,
    realmProfilePlaceholder: view.realmProfilePlaceholder,
    realmProfileImportHint: realmProfileRestriction.reason || view.realmProfileImportHintFallback,
    realmProfileTitle: view.realmProfileTitle,
    realmProfileReferenceLabel: member?.realmProfile || member?.role || member?.name || "",
    realmProfileReferenceHintBefore: view.realmProfileReferenceHintBefore,
    realmProfileReferenceHintAfter: realmProfileRestriction.reason
      ? `from a target realm. ${realmProfileRestriction.reason}`
      : view.realmProfileReferenceHintAfterFallback,
    modelLabel: view.modelLabel,
    runtimeModeLabel: view.runtimeModeLabel,
    runtimeSectionTitle: view.runtimeSectionTitle,
    backendLabel: view.backendLabel,
    inlinePeerNotificationsLabel: view.inlinePeerNotificationsLabel,
    inlinePeerNotificationsPlaceholder: view.inlinePeerNotificationsPlaceholder,
    systemPromptTitle: view.systemPromptTitle,
    applySkeletonLabel: view.applySkeletonLabel,
    applySkeletonTitle: view.applySkeletonTitle,
    systemPromptPlaceholder: view.systemPromptPlaceholder,
    schema,
    profileBinding,
    bindingOptions,
    selectedBinding: bindingOptions.find((option) => option.value === profileBinding) || null,
    isRealmProfile: profileBinding === "realm_profile",
    runtimeMode,
    runtimeOptions,
    selectedRuntime: runtimeOptions.find((option) => option.value === runtimeMode) || null,
    backendValue,
    backendOptions,
    selectedBackend: backendOptions.find((option) => option.value === backendValue) || null,
    schemaOptions,
    outputSchemaTitle: view.outputSchemaTitle,
    schemaPreviewRows,
    hasOutputSchema: !!schema,
    editSchemaLabel: view.editSchemaLabel,
    editSchemaSelection: schema ? { kind: "schema", id: schema.id } : null,
    emptySchemaHint: view.emptySchemaHint,
    modelOptions,
    sourceProvenance: agentSourceProvenanceState(member, agentDetailView),
  };
}

export function agentDeleteConfirmationState(editorState, open = false) {
  const needsConfirmation = !!editorState?.deleteNeedsConfirmation;
  return {
    open: needsConfirmation && !!open,
    needsConfirmation,
    message: String(editorState?.deleteConfirmMessage || ""),
    confirmLabel: String(editorState?.deleteLabel || ""),
    cancelLabel: String(editorState?.deleteCancelLabel || ""),
  };
}

export function sourceDefinitionRefRows(refs) {
  return normalizeAgentDefinitionRows(refs)
    .map((ref) => {
      const id = String(ref.id || "").trim();
      if (!id) return "";
      const source = String(ref.sourceMobpack || ref.source_mobpack || ref.source || "").trim();
      return source ? `${id} (${source})` : id;
    })
    .filter(Boolean);
}

export function agentSourceProvenanceState(member, agentDetailView = null) {
  const view = agentDetailViewForState(agentDetailView);
  const source = member?.sourceDefinition && typeof member.sourceDefinition === "object"
    ? member.sourceDefinition
    : null;
  const toolRefs = sourceDefinitionRefRows(source?.toolDefinitions || source?.tool_definitions);
  const skillRefs = sourceDefinitionRefRows(source?.skillDefinitions || source?.skill_definitions);
  const rows = [];
  const push = (label, value) => {
    const text = String(value || "").trim();
    if (label && text) rows.push({ label, value: text });
  };
  push(view.sourceDefinitionLabel, source?.definitionId || source?.definition_id || "");
  push(view.sourceMobpackLabel, source?.sourceMobpackName || source?.source_mobpack_name || source?.sourceMobpack || source?.source_mobpack || "");
  push(view.sourceOriginLabel, source?.sourceOrigin || source?.source_origin || source?.source || "");
  push(view.sourceDocumentPathLabel, source?.sourceDocumentPath || source?.source_document_path || "");
  push(view.sourceSchemaPathLabel, source?.schemaSourceDocumentPath || source?.schema_source_document_path || "");
  push(view.sourceToolsLabel, toolRefs.join(", "));
  push(view.sourceSkillsLabel, skillRefs.join(", "));
  return {
    title: view.sourceTitle,
    emptyHint: view.sourceEmptyHint,
    hasRows: rows.length > 0,
    rows,
  };
}

export function agentDefinitionOptions(agentDefinitions = []) {
  const definitions = (Array.isArray(agentDefinitions) ? agentDefinitions : [])
    .filter((definition) => definition?.id);
  const labelCounts = definitions.reduce((counts, definition) => {
    const label = String(definition.label || definition.role || definition.id);
    counts.set(label, (counts.get(label) || 0) + 1);
    return counts;
  }, new Map());
  const optionRows = definitions
    .map((definition) => {
      const label = String(definition.label || definition.role || definition.id);
      const sourceLabel = String(definition.sourceMobpackName || definition.sourceMobpack || "").trim();
      return {
        value: definition.id,
        label: labelCounts.get(label) > 1 && sourceLabel ? `${label} · ${sourceLabel}` : label,
        definition,
      };
    });
  return {
    hasDefinitions: optionRows.length > 0,
    optionRows,
  };
}

export function agentDefinitionAddControlState(agentDefinitions = [], agentView = null) {
  const view = agentViewForState(agentView);
  const definitionState = agentDefinitionOptions(agentDefinitions);
  return {
    ...definitionState,
    controlClass: definitionState.hasDefinitions
      ? "agents-list__add agents-list__add--select"
      : "agents-list__add",
    disabled: !definitionState.hasDefinitions,
    title: definitionState.hasDefinitions
      ? view.addAgentTitle
      : view.addAgentUnavailableTitle,
    unavailableLabel: view.addAgentUnavailableLabel,
    authoringOperationUnavailableError: view.authoringOperationUnavailableError,
    placeholderOption: { value: "", label: view.addAgentPlaceholderLabel },
    value: "",
  };
}

export function agentDefinitionAddErrorState(result = null, agentView = null) {
  const view = agentViewForState(agentView);
  const error = operationErrorText(result, "");
  const prefix = view.addAgentErrorPrefix
    ? `${view.addAgentErrorPrefix}${/\s$/.test(view.addAgentErrorPrefix) ? "" : " "}`
    : "";
  return {
    hasError: !!error,
    text: error ? `${prefix}${error}` : "",
    rawError: error,
  };
}

export function agentDefinitionCatalogState(agentDefinitions = [], agentView = null) {
  const view = agentViewForState(agentView);
  const rows = (Array.isArray(agentDefinitions) ? agentDefinitions : [])
    .filter((definition) => definition?.id)
    .map((definition) => {
      const label = String(definition.label || definition.name || definition.role || definition.id).trim();
      const role = String(definition.role || "").trim();
      const source = [
        definition.sourceMobpackName || definition.source_mobpack_name || definition.sourceMobpack || definition.source_mobpack || "",
        definition.sourceOrigin || definition.source_origin || definition.source || "",
      ].map((value) => String(value || "").trim()).filter(Boolean).join(" · ");
      const tools = sourceDefinitionRefRows(definition.toolDefinitions || definition.tool_definitions);
      const skills = sourceDefinitionRefRows(definition.skillDefinitions || definition.skill_definitions);
      return {
        id: String(definition.id || "").trim(),
        title: label,
        role,
        sourceLabel: view.definitionCatalogSourceLabel,
        toolsLabel: view.definitionCatalogToolsLabel,
        skillsLabel: view.definitionCatalogSkillsLabel,
        source,
        tools: tools.join(", "),
        skills: skills.join(", "),
        definition,
      };
    });
  return {
    title: view.definitionCatalogTitle,
    empty: view.definitionCatalogEmpty,
    hasRows: rows.length > 0,
    rows,
  };
}

export function memberSchemaChangeErrorState(result = null, fallback = "") {
  const error = operationErrorText(result, fallback);
  return {
    hasError: !!error,
    text: error,
    rawError: error,
  };
}

export function schemaDefinitionAddErrorState(result = null, fallback = "") {
  return memberSchemaChangeErrorState(result, fallback);
}

export function schemaFieldAddErrorState(result = null, fallback = "") {
  return memberSchemaChangeErrorState(result, fallback);
}

export function inputParamAddErrorState(result = null, fallback = "") {
  return memberSchemaChangeErrorState(result, fallback);
}

export function schemaEditorControlState({ schema, members = [], schemaView = null } = {}) {
  const view = schemaViewForState(schemaView);
  const fields = Array.isArray(schema?.fields) ? schema.fields : [];
  const usedBy = (Array.isArray(members) ? members : [])
    .filter((member) => member?.schema === schema?.id)
    .map((member) => ({
      id: member.id,
      name: member.name,
      role: member.role,
      model: member.model,
      selection: { kind: "agent", id: member.id },
      member,
    }));
  const fieldRows = fields.map((field) => ({
    id: field.id,
    field,
  }));
  return {
    eyebrow: view.eyebrow,
    descriptionTitle: view.descriptionTitle,
    descriptionPlaceholder: view.descriptionPlaceholder,
    fieldsTitle: graphTemplateText(view.fieldsTitleTemplate, {
      prefix: view.fieldsTitlePrefix,
      count: fields.length,
    }),
    addFieldLabel: view.addFieldLabel,
    authoringOperationUnavailableError: view.authoringOperationUnavailableError,
    schemaOperationFallbackError: view.schemaOperationFallbackError,
    fieldAddFallbackError: view.fieldAddFallbackError,
    headerLabels: view.headerLabels,
    fieldRows,
    emptyFieldsHint: view.emptyFieldsHint,
    usedBy,
    usedCount: usedBy.length,
    usageLabel: graphTemplateText(
      usedBy.length === 1 ? view.usageSingularTemplate : view.usagePluralTemplate,
      { count: usedBy.length },
    ),
    usedByTitle: graphTemplateText(view.usedByTitleTemplate, {
      prefix: view.usedByPrefix,
      count: usedBy.length,
    }),
    emptyUsedByHint: view.emptyUsedByHint,
    deleteLabel: view.deleteLabel,
    canDelete: usedBy.length === 0,
    deleteTitle: usedBy.length > 0 ? view.deleteBlockedTitle : "",
  };
}
