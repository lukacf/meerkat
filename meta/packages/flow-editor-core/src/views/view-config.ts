// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the view-config functions move byte-verbatim as plain JS, and
// tsc's inferred unions over their heterogeneous view-state object literals
// raise TS2365/TS2339 (e.g. agentViewFromSchema's every() validator and
// settingsViewFromSchema's .length checks). Source-contract pins this exact
// text, so suppression must live at file level, not in the moved bodies.
// Resolution/linkage stays guarded behaviorally: the projection suite and
// export-keys test load the bundle and exercise these projections, so a
// missed import or re-export still fails the gate as a ReferenceError.
//
// View-config projections for the Flow Editor controller plane. Moved
// verbatim from the controller.js ui-view-config range: role accent styling
// plus the *ViewFromSchema / *ViewForState pairs that project Meta
// editor view schemas into agent, flow-registry, schema, deploy, settings,
// basic, launch, and graph view state. The three control-state functions
// from this range (agentListState, basicEditorViewState,
// graphCanvasViewState) stay in the residue until their domain slices
// (S12, S11).
export function roleAccentColor(role) {
  switch (String(role || "").trim()) {
    case "planner":
      return "var(--accent)";
    case "coder":
      return "var(--ok, #2f7d4d)";
    case "reviewer":
      return "var(--warn, #c98810)";
    case "critic":
      return "var(--muted)";
    case "judge":
      return "#C99A2E";
    case "publisher":
      return "var(--ink)";
    case "illustrator":
      return "var(--accent)";
    case "schema":
      return "var(--subtle)";
    default:
      return "";
  }
}

export function roleAccentStyle(role) {
  const color = roleAccentColor(role);
  return color ? { "--role-color": color } : {};
}

export function agentViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_agent_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    agentsHeading: String(view.agents_heading || "").trim(),
    schemasHeading: String(view.schemas_heading || "").trim(),
    addSchemaLabel: String(view.add_schema_label || "").trim(),
    addAgentTitle: String(view.add_agent_title || "").trim(),
    addAgentUnavailableTitle: String(view.add_agent_unavailable_title || "").trim(),
    addAgentUnavailableLabel: String(view.add_agent_unavailable_label || "").trim(),
    addAgentPlaceholderLabel: String(view.add_agent_placeholder_label || "").trim(),
    addAgentErrorPrefix: String(view.add_agent_error_prefix || "").trim(),
    authoringOperationUnavailableError: String(view.authoring_operation_unavailable_error || "").trim(),
    memberUpdateFallbackError: String(view.member_update_fallback_error || "").trim(),
    toolUpdateFallbackError: String(view.tool_update_fallback_error || "").trim(),
    schemaAssignmentFallbackError: String(view.schema_assignment_fallback_error || "").trim(),
    schemaAddFallbackError: String(view.schema_add_fallback_error || "").trim(),
    definitionCatalogTitle: String(view.definition_catalog_title || "").trim(),
    definitionCatalogEmpty: String(view.definition_catalog_empty || "").trim(),
    definitionCatalogSourceLabel: String(view.definition_catalog_source_label || "").trim(),
    definitionCatalogToolsLabel: String(view.definition_catalog_tools_label || "").trim(),
    definitionCatalogSkillsLabel: String(view.definition_catalog_skills_label || "").trim(),
    memberSubLabelTemplate: String(view.member_sub_label_template || "").trim(),
    memberPlacedEmptyLabel: String(view.member_placed_empty_label || "").trim(),
    memberPlacedCountTemplate: String(view.member_placed_count_template || "").trim(),
    schemaFieldSingularTemplate: String(view.schema_field_singular_template || "").trim(),
    schemaFieldPluralTemplate: String(view.schema_field_plural_template || "").trim(),
    schemaUsageLabelTemplate: String(view.schema_usage_label_template || "").trim(),
    sidebarSubLabelSeparator: String(view.sidebar_sub_label_separator || ""),
    emptyTitle: String(view.empty_title || "").trim(),
    emptyLines: Array.isArray(view.empty_lines)
      ? view.empty_lines.map((line) => String(line || "").trim()).filter(Boolean)
      : [],
    missingSchemaLabel: String(view.missing_schema_label || "").trim(),
    missingAgentLabel: String(view.missing_agent_label || "").trim(),
  };
  return out.agentsHeading && out.schemasHeading && out.addSchemaLabel
    && out.addAgentTitle && out.addAgentUnavailableTitle
    && out.addAgentUnavailableLabel && out.addAgentPlaceholderLabel
    && out.authoringOperationUnavailableError && out.memberUpdateFallbackError
    && out.toolUpdateFallbackError && out.schemaAssignmentFallbackError
    && out.schemaAddFallbackError
    && out.definitionCatalogTitle && out.definitionCatalogEmpty
    && out.definitionCatalogSourceLabel && out.definitionCatalogToolsLabel && out.definitionCatalogSkillsLabel
    && out.memberSubLabelTemplate && out.memberPlacedEmptyLabel && out.memberPlacedCountTemplate
    && out.schemaFieldSingularTemplate && out.schemaFieldPluralTemplate && out.schemaUsageLabelTemplate
    && out.sidebarSubLabelSeparator
    && out.emptyTitle && out.emptyLines.length && out.missingSchemaLabel && out.missingAgentLabel
    ? out
    : null;
}

export function agentViewForState(agentView) {
  const view = agentView && typeof agentView === "object" ? agentView : null;
  return {
    agentsHeading: String(view?.agentsHeading || ""),
    schemasHeading: String(view?.schemasHeading || ""),
    addSchemaLabel: String(view?.addSchemaLabel || ""),
    addAgentTitle: String(view?.addAgentTitle || ""),
    addAgentUnavailableTitle: String(view?.addAgentUnavailableTitle || ""),
    addAgentUnavailableLabel: String(view?.addAgentUnavailableLabel || ""),
    addAgentPlaceholderLabel: String(view?.addAgentPlaceholderLabel || ""),
    addAgentErrorPrefix: String(view?.addAgentErrorPrefix || ""),
    authoringOperationUnavailableError: String(view?.authoringOperationUnavailableError || ""),
    memberUpdateFallbackError: String(view?.memberUpdateFallbackError || ""),
    toolUpdateFallbackError: String(view?.toolUpdateFallbackError || ""),
    schemaAssignmentFallbackError: String(view?.schemaAssignmentFallbackError || ""),
    schemaAddFallbackError: String(view?.schemaAddFallbackError || ""),
    definitionCatalogTitle: String(view?.definitionCatalogTitle || ""),
    definitionCatalogEmpty: String(view?.definitionCatalogEmpty || ""),
    definitionCatalogSourceLabel: String(view?.definitionCatalogSourceLabel || ""),
    definitionCatalogToolsLabel: String(view?.definitionCatalogToolsLabel || ""),
    definitionCatalogSkillsLabel: String(view?.definitionCatalogSkillsLabel || ""),
    memberSubLabelTemplate: String(view?.memberSubLabelTemplate || ""),
    memberPlacedEmptyLabel: String(view?.memberPlacedEmptyLabel || ""),
    memberPlacedCountTemplate: String(view?.memberPlacedCountTemplate || ""),
    schemaFieldSingularTemplate: String(view?.schemaFieldSingularTemplate || ""),
    schemaFieldPluralTemplate: String(view?.schemaFieldPluralTemplate || ""),
    schemaUsageLabelTemplate: String(view?.schemaUsageLabelTemplate || ""),
    sidebarSubLabelSeparator: String(view?.sidebarSubLabelSeparator || ""),
    emptyTitle: String(view?.emptyTitle || ""),
    emptyLines: Array.isArray(view?.emptyLines) ? view.emptyLines : [],
    missingSchemaLabel: String(view?.missingSchemaLabel || ""),
    missingAgentLabel: String(view?.missingAgentLabel || ""),
  };
}

export function newFlowViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_new_flow_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    eyebrowTemplate: String(view.eyebrow_template || "").trim(),
    closeLabel: String(view.close_label || "").trim(),
    nameLabel: String(view.name_label || "").trim(),
    namePlaceholder: String(view.name_placeholder || "").trim(),
    startFromLabel: String(view.start_from_label || "").trim(),
    createLabel: String(view.create_label || "").trim(),
  };
  return out.eyebrowTemplate && out.closeLabel && out.nameLabel && out.namePlaceholder
    && out.startFromLabel && out.createLabel
    ? out
    : null;
}

export function newFlowViewForState(newFlowView) {
  const view = newFlowView && typeof newFlowView === "object" ? newFlowView : null;
  return {
    eyebrowTemplate: String(view?.eyebrowTemplate || ""),
    closeLabel: String(view?.closeLabel || ""),
    nameLabel: String(view?.nameLabel || ""),
    namePlaceholder: String(view?.namePlaceholder || ""),
    startFromLabel: String(view?.startFromLabel || ""),
    createLabel: String(view?.createLabel || ""),
  };
}

export function flowRegistryViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_flow_registry_view;
  if (!view || typeof view !== "object") return null;
  const columns = Array.isArray(view.columns)
    ? view.columns.map((column) => ({
      key: String(column?.key || "").trim(),
      label: String(column?.label || "").trim(),
    })).filter((column) => column.key && column.label)
    : [];
  const out = {
    eyebrow: String(view.eyebrow || "").trim(),
    titleSingularSuffix: String(view.title_singular_suffix || "").trim(),
    titlePluralSuffix: String(view.title_plural_suffix || "").trim(),
    createLabel: String(view.create_label || "").trim(),
    createReadyTitle: String(view.create_ready_title || "").trim(),
    createUnavailableTitle: String(view.create_unavailable_title || "").trim(),
    draftsSectionLabel: String(view.drafts_section_label || "").trim(),
    runtimeSectionLabel: String(view.runtime_section_label || "").trim(),
    runtimeReadonlyHint: String(view.runtime_readonly_hint || "").trim(),
    emptyTitle: String(view.empty_title || "").trim(),
    emptyText: String(view.empty_text || "").trim(),
    updatedJustNowLabel: String(view.updated_just_now_label || "").trim(),
    columns,
  };
  return out.eyebrow && out.titleSingularSuffix && out.titlePluralSuffix
    && out.createLabel && out.createReadyTitle && out.createUnavailableTitle
    && out.draftsSectionLabel && out.runtimeSectionLabel && out.runtimeReadonlyHint
    && out.emptyTitle && out.emptyText && out.updatedJustNowLabel
    && out.columns.length === 4
    ? out
    : null;
}

export function flowRegistryViewForState(flowRegistryView) {
  const view = flowRegistryView && typeof flowRegistryView === "object" ? flowRegistryView : null;
  return {
    eyebrow: String(view?.eyebrow || ""),
    titleSingularSuffix: String(view?.titleSingularSuffix || ""),
    titlePluralSuffix: String(view?.titlePluralSuffix || ""),
    createLabel: String(view?.createLabel || ""),
    createReadyTitle: String(view?.createReadyTitle || ""),
    createUnavailableTitle: String(view?.createUnavailableTitle || ""),
    draftsSectionLabel: String(view?.draftsSectionLabel || ""),
    runtimeSectionLabel: String(view?.runtimeSectionLabel || ""),
    runtimeReadonlyHint: String(view?.runtimeReadonlyHint || ""),
    emptyTitle: String(view?.emptyTitle || ""),
    emptyText: String(view?.emptyText || ""),
    updatedJustNowLabel: String(view?.updatedJustNowLabel || ""),
    columns: Array.isArray(view?.columns)
      ? view.columns.map((column) => ({
        key: String(column?.key || ""),
        label: String(column?.label || ""),
      })).filter((column) => column.key && column.label)
      : [],
  };
}

export function schemaViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_schema_view;
  if (!view || typeof view !== "object") return null;
  const headers = view.header_labels && typeof view.header_labels === "object" ? view.header_labels : {};
  const out = {
    eyebrow: String(view.eyebrow || "").trim(),
    descriptionTitle: String(view.description_title || "").trim(),
    descriptionPlaceholder: String(view.description_placeholder || "").trim(),
    fieldsTitlePrefix: String(view.fields_title_prefix || "").trim(),
    fieldsTitleTemplate: String(view.fields_title_template || "").trim(),
    addFieldLabel: String(view.add_field_label || "").trim(),
    authoringOperationUnavailableError: String(view.authoring_operation_unavailable_error || "").trim(),
    schemaOperationFallbackError: String(view.schema_operation_fallback_error || "").trim(),
    fieldAddFallbackError: String(view.field_add_fallback_error || "").trim(),
    headerLabels: {
      name: String(headers.name || "").trim(),
      type: String(headers.type || "").trim(),
      required: String(headers.required || "").trim(),
      description: String(headers.description || "").trim(),
      action: String(headers.action || "").trim(),
    },
    emptyFieldsHint: String(view.empty_fields_hint || "").trim(),
    usedByPrefix: String(view.used_by_prefix || "").trim(),
    usedByTitleTemplate: String(view.used_by_title_template || "").trim(),
    usageSingularTemplate: String(view.usage_singular_template || "").trim(),
    usagePluralTemplate: String(view.usage_plural_template || "").trim(),
    emptyUsedByHint: String(view.empty_used_by_hint || "").trim(),
    deleteLabel: String(view.delete_label || "").trim(),
    deleteBlockedTitle: String(view.delete_blocked_title || "").trim(),
    fieldNamePlaceholder: String(view.field_name_placeholder || "").trim(),
    fieldDescriptionPlaceholder: String(view.field_description_placeholder || "").trim(),
    fieldRemoveTitle: String(view.field_remove_title || "").trim(),
    fieldEnumLabel: String(view.field_enum_label || "").trim(),
    fieldEnumAddLabel: String(view.field_enum_add_label || "").trim(),
    fieldEnumAddValue: String(view.field_enum_add_value || "").trim(),
  };
  return out.eyebrow && out.descriptionTitle && out.fieldsTitlePrefix && out.fieldsTitleTemplate && out.addFieldLabel
    && out.authoringOperationUnavailableError && out.schemaOperationFallbackError && out.fieldAddFallbackError
    && out.headerLabels.name && out.headerLabels.type && out.headerLabels.required && out.headerLabels.description
    && out.emptyFieldsHint && out.usedByPrefix && out.usedByTitleTemplate
    && out.usageSingularTemplate && out.usagePluralTemplate && out.emptyUsedByHint && out.deleteLabel && out.deleteBlockedTitle
    && out.fieldNamePlaceholder && out.fieldDescriptionPlaceholder && out.fieldRemoveTitle
    && out.fieldEnumLabel && out.fieldEnumAddLabel && out.fieldEnumAddValue
    ? out
    : null;
}

export function schemaViewForState(schemaView) {
  const view = schemaView && typeof schemaView === "object" ? schemaView : null;
  return {
    eyebrow: String(view?.eyebrow || ""),
    descriptionTitle: String(view?.descriptionTitle || ""),
    descriptionPlaceholder: String(view?.descriptionPlaceholder || ""),
    fieldsTitlePrefix: String(view?.fieldsTitlePrefix || ""),
    fieldsTitleTemplate: String(view?.fieldsTitleTemplate || ""),
    addFieldLabel: String(view?.addFieldLabel || ""),
    authoringOperationUnavailableError: String(view?.authoringOperationUnavailableError || ""),
    schemaOperationFallbackError: String(view?.schemaOperationFallbackError || ""),
    fieldAddFallbackError: String(view?.fieldAddFallbackError || ""),
    headerLabels: {
      name: String(view?.headerLabels?.name || ""),
      type: String(view?.headerLabels?.type || ""),
      required: String(view?.headerLabels?.required || ""),
      description: String(view?.headerLabels?.description || ""),
      action: String(view?.headerLabels?.action || ""),
    },
    emptyFieldsHint: String(view?.emptyFieldsHint || ""),
    usedByPrefix: String(view?.usedByPrefix || ""),
    usedByTitleTemplate: String(view?.usedByTitleTemplate || ""),
    usageSingularTemplate: String(view?.usageSingularTemplate || ""),
    usagePluralTemplate: String(view?.usagePluralTemplate || ""),
    emptyUsedByHint: String(view?.emptyUsedByHint || ""),
    deleteLabel: String(view?.deleteLabel || ""),
    deleteBlockedTitle: String(view?.deleteBlockedTitle || ""),
    fieldNamePlaceholder: String(view?.fieldNamePlaceholder || ""),
    fieldDescriptionPlaceholder: String(view?.fieldDescriptionPlaceholder || ""),
    fieldRemoveTitle: String(view?.fieldRemoveTitle || ""),
    fieldEnumLabel: String(view?.fieldEnumLabel || ""),
    fieldEnumAddLabel: String(view?.fieldEnumAddLabel || ""),
    fieldEnumAddValue: String(view?.fieldEnumAddValue || ""),
  };
}

export function agentDetailViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_agent_detail_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    usedInLabel: String(view.used_in_label || "").trim(),
    instanceSingular: String(view.instance_singular || "").trim(),
    instancePlural: String(view.instance_plural || "").trim(),
    deleteLabel: String(view.delete_label || "").trim(),
    deleteConfirmIntro: String(view.delete_confirm_intro || "").trim(),
    deleteConfirmPlacedPrefix: String(view.delete_confirm_placed_prefix || "").trim(),
    deleteCancelLabel: String(view.delete_cancel_label || "").trim(),
    cellSingular: String(view.cell_singular || "").trim(),
    cellPlural: String(view.cell_plural || "").trim(),
    deleteConfirmCellsSuffix: String(view.delete_confirm_cells_suffix || "").trim(),
    usageTitlePrefix: String(view.usage_title_prefix || "").trim(),
    emptyUsageHint: String(view.empty_usage_hint || "").trim(),
    agentEyebrowPrefix: String(view.agent_eyebrow_prefix || "").trim(),
    identityTitle: String(view.identity_title || "").trim(),
    profileBindingLabel: String(view.profile_binding_label || "").trim(),
    missingProfileBindingLabel: String(view.missing_profile_binding_label || "").trim(),
    realmProfileLabel: String(view.realm_profile_label || "").trim(),
    realmProfilePlaceholder: String(view.realm_profile_placeholder || "").trim(),
    realmProfileImportHintFallback: String(view.realm_profile_import_hint_fallback || "").trim(),
    realmProfileTitle: String(view.realm_profile_title || "").trim(),
    realmProfileReferenceHintBefore: String(view.realm_profile_reference_hint_before || "").trim(),
    realmProfileReferenceHintAfterFallback: String(view.realm_profile_reference_hint_after_fallback || "").trim(),
    modelLabel: String(view.model_label || "").trim(),
    runtimeModeLabel: String(view.runtime_mode_label || "").trim(),
    runtimeSectionTitle: String(view.runtime_section_title || "").trim(),
    missingRuntimeModeLabel: String(view.missing_runtime_mode_label || "").trim(),
    backendLabel: String(view.backend_label || "").trim(),
    backendDefinitionDefaultLabel: String(view.backend_definition_default_label || "").trim(),
    inlinePeerNotificationsLabel: String(view.inline_peer_notifications_label || "").trim(),
    inlinePeerNotificationsPlaceholder: String(view.inline_peer_notifications_placeholder || "").trim(),
    providerParamsLabel: String(view.provider_params_label || "").trim(),
    providerParamsPlaceholder: String(view.provider_params_placeholder || "").trim(),
    providerParamsRows: Number(view.provider_params_rows || 0),
    providerParamsInvalidJsonLabel: String(view.provider_params_invalid_json_label || "").trim(),
    providerParamsObjectRequiredError: String(view.provider_params_object_required_error || "").trim(),
    systemPromptTitle: String(view.system_prompt_title || "").trim(),
    applySkeletonLabel: String(view.apply_skeleton_label || "").trim(),
    applySkeletonTitle: String(view.apply_skeleton_title || "").trim(),
    systemPromptPlaceholder: String(view.system_prompt_placeholder || "").trim(),
    outputSchemaTitle: String(view.output_schema_title || "").trim(),
    schemaNoneLabel: String(view.schema_none_label || "").trim(),
    schemaRequiredLabel: String(view.schema_required_label || "").trim(),
    editSchemaLabel: String(view.edit_schema_label || "").trim(),
    emptySchemaHint: String(view.empty_schema_hint || "").trim(),
    sourceTitle: String(view.source_title || "").trim(),
    sourceEmptyHint: String(view.source_empty_hint || "").trim(),
    sourceDefinitionLabel: String(view.source_definition_label || "").trim(),
    sourceMobpackLabel: String(view.source_mobpack_label || "").trim(),
    sourceOriginLabel: String(view.source_origin_label || "").trim(),
    sourceDocumentPathLabel: String(view.source_document_path_label || "").trim(),
    sourceSchemaPathLabel: String(view.source_schema_path_label || "").trim(),
    sourceToolsLabel: String(view.source_tools_label || "").trim(),
    sourceSkillsLabel: String(view.source_skills_label || "").trim(),
  };
  return out.usedInLabel && out.instanceSingular && out.instancePlural && out.deleteLabel
    && out.deleteConfirmIntro && out.deleteConfirmPlacedPrefix && out.deleteCancelLabel && out.cellSingular && out.cellPlural
    && out.deleteConfirmCellsSuffix && out.usageTitlePrefix
    && out.emptyUsageHint && out.agentEyebrowPrefix && out.identityTitle && out.profileBindingLabel && out.missingProfileBindingLabel
    && out.realmProfileLabel && out.realmProfilePlaceholder && out.realmProfileImportHintFallback
    && out.realmProfileTitle && out.realmProfileReferenceHintBefore && out.realmProfileReferenceHintAfterFallback
    && out.modelLabel && out.runtimeModeLabel && out.runtimeSectionTitle && out.missingRuntimeModeLabel && out.backendLabel
    && out.backendDefinitionDefaultLabel
    && out.inlinePeerNotificationsLabel && out.inlinePeerNotificationsPlaceholder
    && out.providerParamsLabel && out.providerParamsPlaceholder && Number.isFinite(out.providerParamsRows) && out.providerParamsRows > 0
    && out.providerParamsInvalidJsonLabel && out.providerParamsObjectRequiredError
    && out.systemPromptTitle && out.applySkeletonLabel && out.applySkeletonTitle && out.systemPromptPlaceholder
    && out.outputSchemaTitle && out.schemaNoneLabel && out.schemaRequiredLabel && out.editSchemaLabel && out.emptySchemaHint
    && out.sourceTitle && out.sourceEmptyHint && out.sourceDefinitionLabel && out.sourceMobpackLabel
    && out.sourceOriginLabel && out.sourceDocumentPathLabel && out.sourceSchemaPathLabel
    && out.sourceToolsLabel && out.sourceSkillsLabel
    ? out
    : null;
}

export function agentDetailViewForState(agentDetailView) {
  const view = agentDetailView && typeof agentDetailView === "object" ? agentDetailView : null;
  return {
    usedInLabel: String(view?.usedInLabel || ""),
    instanceSingular: String(view?.instanceSingular || ""),
    instancePlural: String(view?.instancePlural || ""),
    deleteLabel: String(view?.deleteLabel || ""),
    deleteConfirmIntro: String(view?.deleteConfirmIntro || ""),
    deleteConfirmPlacedPrefix: String(view?.deleteConfirmPlacedPrefix || ""),
    deleteCancelLabel: String(view?.deleteCancelLabel || ""),
    cellSingular: String(view?.cellSingular || ""),
    cellPlural: String(view?.cellPlural || ""),
    deleteConfirmCellsSuffix: String(view?.deleteConfirmCellsSuffix || ""),
    usageTitlePrefix: String(view?.usageTitlePrefix || ""),
    emptyUsageHint: String(view?.emptyUsageHint || ""),
    agentEyebrowPrefix: String(view?.agentEyebrowPrefix || ""),
    identityTitle: String(view?.identityTitle || ""),
    profileBindingLabel: String(view?.profileBindingLabel || ""),
    missingProfileBindingLabel: String(view?.missingProfileBindingLabel || ""),
    realmProfileLabel: String(view?.realmProfileLabel || ""),
    realmProfilePlaceholder: String(view?.realmProfilePlaceholder || ""),
    realmProfileImportHintFallback: String(view?.realmProfileImportHintFallback || ""),
    realmProfileTitle: String(view?.realmProfileTitle || ""),
    realmProfileReferenceHintBefore: String(view?.realmProfileReferenceHintBefore || ""),
    realmProfileReferenceHintAfterFallback: String(view?.realmProfileReferenceHintAfterFallback || ""),
    modelLabel: String(view?.modelLabel || ""),
    runtimeModeLabel: String(view?.runtimeModeLabel || ""),
    runtimeSectionTitle: String(view?.runtimeSectionTitle || ""),
    missingRuntimeModeLabel: String(view?.missingRuntimeModeLabel || ""),
    backendLabel: String(view?.backendLabel || ""),
    backendDefinitionDefaultLabel: String(view?.backendDefinitionDefaultLabel || ""),
    inlinePeerNotificationsLabel: String(view?.inlinePeerNotificationsLabel || ""),
    inlinePeerNotificationsPlaceholder: String(view?.inlinePeerNotificationsPlaceholder || ""),
    providerParamsLabel: String(view?.providerParamsLabel || ""),
    providerParamsPlaceholder: String(view?.providerParamsPlaceholder || ""),
    providerParamsRows: Number(view?.providerParamsRows || 0),
    providerParamsInvalidJsonLabel: String(view?.providerParamsInvalidJsonLabel || ""),
    providerParamsObjectRequiredError: String(view?.providerParamsObjectRequiredError || ""),
    systemPromptTitle: String(view?.systemPromptTitle || ""),
    applySkeletonLabel: String(view?.applySkeletonLabel || ""),
    applySkeletonTitle: String(view?.applySkeletonTitle || ""),
    systemPromptPlaceholder: String(view?.systemPromptPlaceholder || ""),
    outputSchemaTitle: String(view?.outputSchemaTitle || ""),
    schemaNoneLabel: String(view?.schemaNoneLabel || ""),
    schemaRequiredLabel: String(view?.schemaRequiredLabel || ""),
    editSchemaLabel: String(view?.editSchemaLabel || ""),
    emptySchemaHint: String(view?.emptySchemaHint || ""),
    sourceTitle: String(view?.sourceTitle || ""),
    sourceEmptyHint: String(view?.sourceEmptyHint || ""),
    sourceDefinitionLabel: String(view?.sourceDefinitionLabel || ""),
    sourceMobpackLabel: String(view?.sourceMobpackLabel || ""),
    sourceOriginLabel: String(view?.sourceOriginLabel || ""),
    sourceDocumentPathLabel: String(view?.sourceDocumentPathLabel || ""),
    sourceSchemaPathLabel: String(view?.sourceSchemaPathLabel || ""),
    sourceToolsLabel: String(view?.sourceToolsLabel || ""),
    sourceSkillsLabel: String(view?.sourceSkillsLabel || ""),
  };
}

export function agentAccessViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_agent_access_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    toolInvalidError: String(view.tool_invalid_error || "").trim(),
    toolEmptyError: String(view.tool_empty_error || "").trim(),
    authoringOperationUnavailableError: String(view.authoring_operation_unavailable_error || "").trim(),
    toolTitle: String(view.tool_title || "").trim(),
    toolHint: String(view.tool_hint || "").trim(),
    toolMissingDescription: String(view.tool_missing_description || "").trim(),
    toolRemoveLabel: String(view.tool_remove_label || "").trim(),
    toolAddSelectPlaceholder: String(view.tool_add_select_placeholder || "").trim(),
    toolSourceLabel: String(view.tool_source_label || "").trim(),
    toolSourcePlaceholder: String(view.tool_source_placeholder || "").trim(),
    toolAddButtonLabel: String(view.tool_add_button_label || "").trim(),
    inlineSkillRealmId: String(view.inline_skill_realm_id || "").trim(),
    inlineSkillRealmLabel: String(view.inline_skill_realm_label || "").trim(),
    inlineSkillRealmSource: String(view.inline_skill_realm_source || "").trim(),
    inlineSkillSource: String(view.inline_skill_source || "").trim(),
    inlineSkillDefaultDescription: String(view.inline_skill_default_description || "").trim(),
    skillDefaultDescription: String(view.skill_default_description || "").trim(),
    skillSelectedCheckLabel: String(view.skill_selected_check_label || "").trim(),
    skillRemoveLabel: String(view.skill_remove_label || "").trim(),
    skillSectionTitle: String(view.skill_section_title || "").trim(),
    skillInlineCancelLabel: String(view.skill_inline_cancel_label || "").trim(),
    skillInlineOpenLabel: String(view.skill_inline_open_label || "").trim(),
    skillHint: String(view.skill_hint || "").trim(),
    skillInlineLabelPlaceholder: String(view.skill_inline_label_placeholder || "").trim(),
    skillInlineContentRows: Number(view.skill_inline_content_rows || 0),
    skillInlineContentPlaceholder: String(view.skill_inline_content_placeholder || "").trim(),
    skillInlineCreateHint: String(view.skill_inline_create_hint || "").trim(),
    skillInlineAddLabel: String(view.skill_inline_add_label || "").trim(),
    skillInlineErrorFallback: String(view.skill_inline_error_fallback || "").trim(),
    skillInlineMissingLabelError: String(view.skill_inline_missing_label_error || "").trim(),
    skillInlineMissingContentError: String(view.skill_inline_missing_content_error || "").trim(),
    skillInlineInvalidIdError: String(view.skill_inline_invalid_id_error || "").trim(),
    skillNoRealmsMessage: String(view.skill_no_realms_message || "").trim(),
    skillRealmLabel: String(view.skill_realm_label || "").trim(),
    skillDefaultRealmSuffix: String(view.skill_default_realm_suffix || ""),
    skillUnavailableHeading: String(view.skill_unavailable_heading || "").trim(),
    skillOutsideRealmHeading: String(view.skill_outside_realm_heading || "").trim(),
  };
  return Object.entries(out).every(([key, value]) => key === "skillInlineContentRows" ? Number.isFinite(value) && value > 0 : !!value)
    ? out
    : null;
}

export function agentAccessViewForState(agentAccessView) {
  const view = agentAccessView && typeof agentAccessView === "object" ? agentAccessView : null;
  return {
    toolInvalidError: String(view?.toolInvalidError || ""),
    toolEmptyError: String(view?.toolEmptyError || ""),
    authoringOperationUnavailableError: String(view?.authoringOperationUnavailableError || ""),
    toolTitle: String(view?.toolTitle || ""),
    toolHint: String(view?.toolHint || ""),
    toolMissingDescription: String(view?.toolMissingDescription || ""),
    toolRemoveLabel: String(view?.toolRemoveLabel || ""),
    toolAddSelectPlaceholder: String(view?.toolAddSelectPlaceholder || ""),
    toolSourceLabel: String(view?.toolSourceLabel || ""),
    toolSourcePlaceholder: String(view?.toolSourcePlaceholder || ""),
    toolAddButtonLabel: String(view?.toolAddButtonLabel || ""),
    inlineSkillRealmId: String(view?.inlineSkillRealmId || ""),
    inlineSkillRealmLabel: String(view?.inlineSkillRealmLabel || ""),
    inlineSkillRealmSource: String(view?.inlineSkillRealmSource || ""),
    inlineSkillSource: String(view?.inlineSkillSource || ""),
    inlineSkillDefaultDescription: String(view?.inlineSkillDefaultDescription || ""),
    skillDefaultDescription: String(view?.skillDefaultDescription || ""),
    skillSelectedCheckLabel: String(view?.skillSelectedCheckLabel || ""),
    skillRemoveLabel: String(view?.skillRemoveLabel || ""),
    skillSectionTitle: String(view?.skillSectionTitle || ""),
    skillInlineCancelLabel: String(view?.skillInlineCancelLabel || ""),
    skillInlineOpenLabel: String(view?.skillInlineOpenLabel || ""),
    skillHint: String(view?.skillHint || ""),
    skillInlineLabelPlaceholder: String(view?.skillInlineLabelPlaceholder || ""),
    skillInlineContentRows: Number(view?.skillInlineContentRows || 0),
    skillInlineContentPlaceholder: String(view?.skillInlineContentPlaceholder || ""),
    skillInlineCreateHint: String(view?.skillInlineCreateHint || ""),
    skillInlineAddLabel: String(view?.skillInlineAddLabel || ""),
    skillInlineErrorFallback: String(view?.skillInlineErrorFallback || ""),
    skillInlineMissingLabelError: String(view?.skillInlineMissingLabelError || ""),
    skillInlineMissingContentError: String(view?.skillInlineMissingContentError || ""),
    skillInlineInvalidIdError: String(view?.skillInlineInvalidIdError || ""),
    skillNoRealmsMessage: String(view?.skillNoRealmsMessage || ""),
    skillRealmLabel: String(view?.skillRealmLabel || ""),
    skillDefaultRealmSuffix: String(view?.skillDefaultRealmSuffix || ""),
    skillUnavailableHeading: String(view?.skillUnavailableHeading || ""),
    skillOutsideRealmHeading: String(view?.skillOutsideRealmHeading || ""),
  };
}

export function deployViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_deploy_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    brandLabel: String(view.brand_label || "").trim(),
    flowTabLabel: String(view.flow_tab_label || "").trim(),
    agentsTabLabel: String(view.agents_tab_label || "").trim(),
    settingsTabLabel: String(view.settings_tab_label || "").trim(),
    mobStatusTitle: String(view.mob_status_title || "").trim(),
    mobFileLabel: String(view.mob_file_label || "").trim(),
    apiErrorLabel: String(view.api_error_label || "").trim(),
    apiReadyLabel: String(view.api_ready_label || "").trim(),
    apiLoadingLabel: String(view.api_loading_label || "").trim(),
    deployPrefixLabel: String(view.deploy_prefix_label || "").trim(),
    flowsCrumbLabel: String(view.flows_crumb_label || "").trim(),
    crumbSeparator: String(view.crumb_separator || "").trim(),
    switcherViewAllLabel: String(view.switcher_view_all_label || "").trim(),
    planTraceLabel: String(view.plan_trace_label || "").trim(),
    importLabel: String(view.import_label || "").trim(),
    validateLabel: String(view.validate_label || "").trim(),
    publishLabel: String(view.publish_label || "").trim(),
    deployPlanLabel: String(view.deploy_plan_label || "").trim(),
    deployLabel: String(view.deploy_label || "").trim(),
    overflowLabel: String(view.overflow_label || "").trim(),
    themeSwitchPrefix: String(view.theme_switch_prefix || "").trim(),
    themeSwitchSuffix: String(view.theme_switch_suffix || "").trim(),
    darkThemeLabel: String(view.dark_theme_label || "").trim(),
    lightThemeLabel: String(view.light_theme_label || "").trim(),
    basicModeTitle: String(view.basic_mode_title || "").trim(),
    basicModeLabel: String(view.basic_mode_label || "").trim(),
    graphModeTitle: String(view.graph_mode_title || "").trim(),
    graphModeLabel: String(view.graph_mode_label || "").trim(),
    validationEyebrow: String(view.validation_eyebrow || "").trim(),
    validationPassedLabel: String(view.validation_passed_label || "").trim(),
    validationWarningsLabel: String(view.validation_warnings_label || "").trim(),
    validationBlockingLabel: String(view.validation_blocking_label || "").trim(),
    closeLabel: String(view.close_label || "").trim(),
    planEyebrow: String(view.plan_eyebrow || "").trim(),
    planUnavailableHead: String(view.plan_unavailable_head || "").trim(),
    planUnavailableBody: String(view.plan_unavailable_body || "").trim(),
    planFirstLabel: String(view.plan_first_label || "").trim(),
    planStepLabel: String(view.plan_step_label || "").trim(),
    planPreviousLabel: String(view.plan_previous_label || "").trim(),
    planNextLabel: String(view.plan_next_label || "").trim(),
  };
  return Object.values(out).every(Boolean) ? out : null;
}

export function deployViewForState(deployView) {
  const view = deployView && typeof deployView === "object" ? deployView : null;
  return {
    brandLabel: String(view?.brandLabel || ""),
    flowTabLabel: String(view?.flowTabLabel || ""),
    agentsTabLabel: String(view?.agentsTabLabel || ""),
    settingsTabLabel: String(view?.settingsTabLabel || ""),
    mobStatusTitle: String(view?.mobStatusTitle || ""),
    mobFileLabel: String(view?.mobFileLabel || ""),
    apiErrorLabel: String(view?.apiErrorLabel || ""),
    apiReadyLabel: String(view?.apiReadyLabel || ""),
    apiLoadingLabel: String(view?.apiLoadingLabel || ""),
    deployPrefixLabel: String(view?.deployPrefixLabel || ""),
    flowsCrumbLabel: String(view?.flowsCrumbLabel || ""),
    crumbSeparator: String(view?.crumbSeparator || ""),
    switcherViewAllLabel: String(view?.switcherViewAllLabel || ""),
    planTraceLabel: String(view?.planTraceLabel || ""),
    importLabel: String(view?.importLabel || ""),
    validateLabel: String(view?.validateLabel || ""),
    publishLabel: String(view?.publishLabel || ""),
    deployPlanLabel: String(view?.deployPlanLabel || ""),
    deployLabel: String(view?.deployLabel || ""),
    overflowLabel: String(view?.overflowLabel || ""),
    themeSwitchPrefix: String(view?.themeSwitchPrefix || ""),
    themeSwitchSuffix: String(view?.themeSwitchSuffix || ""),
    darkThemeLabel: String(view?.darkThemeLabel || ""),
    lightThemeLabel: String(view?.lightThemeLabel || ""),
    basicModeTitle: String(view?.basicModeTitle || ""),
    basicModeLabel: String(view?.basicModeLabel || ""),
    graphModeTitle: String(view?.graphModeTitle || ""),
    graphModeLabel: String(view?.graphModeLabel || ""),
    validationEyebrow: String(view?.validationEyebrow || ""),
    validationPassedLabel: String(view?.validationPassedLabel || ""),
    validationWarningsLabel: String(view?.validationWarningsLabel || ""),
    validationBlockingLabel: String(view?.validationBlockingLabel || ""),
    closeLabel: String(view?.closeLabel || ""),
    planEyebrow: String(view?.planEyebrow || ""),
    planUnavailableHead: String(view?.planUnavailableHead || ""),
    planUnavailableBody: String(view?.planUnavailableBody || ""),
    planFirstLabel: String(view?.planFirstLabel || ""),
    planStepLabel: String(view?.planStepLabel || ""),
    planPreviousLabel: String(view?.planPreviousLabel || ""),
    planNextLabel: String(view?.planNextLabel || ""),
  };
}

export function settingsViewOptionsFromSchema(value) {
  return Array.isArray(value)
    ? value
      .map((option) => ({
        value: String(option?.value || "").trim(),
        label: String(option?.label || "").trim(),
      }))
      .filter((option) => option.value && option.label)
    : [];
}

export function settingsViewLabelMapFromSchema(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  const out = {};
  for (const [key, label] of Object.entries(value)) {
    const optionValue = String(key || "").trim();
    const optionLabel = String(label || "").trim();
    if (optionValue && optionLabel) out[optionValue] = optionLabel;
  }
  return out;
}

export function settingsViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_settings_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    optionSeparator: String(view.option_separator || ""),
    unsupportedLabelSeparator: String(view.unsupported_label_separator || ""),
    unsupportedReasonPrefix: String(view.unsupported_reason_prefix || ""),
    unsupportedReasonSuffix: String(view.unsupported_reason_suffix || ""),
    canvasTitle: String(view.canvas_title || "").trim(),
    edgeStyleLabel: String(view.edge_style_label || "").trim(),
    edgeStyleOptions: settingsViewOptionsFromSchema(view.edge_style_options),
    densityLabel: String(view.density_label || "").trim(),
    densityOptions: settingsViewOptionsFromSchema(view.density_options),
    themeTitle: String(view.theme_title || "").trim(),
    themeModeLabel: String(view.theme_mode_label || "").trim(),
    themeModeOptions: settingsViewOptionsFromSchema(view.theme_mode_options),
    mobTitle: String(view.mob_title || "").trim(),
    orchestratorLabel: String(view.orchestrator_label || "").trim(),
    profileNoneLabel: String(view.profile_none_label || "").trim(),
    autoWireLabel: String(view.auto_wire_label || "").trim(),
    autoWireOptions: settingsViewOptionsFromSchema(view.auto_wire_options),
    roleWiringLabel: String(view.role_wiring_label || "").trim(),
    roleWiringAddLabel: String(view.role_wiring_add_label || "").trim(),
    defaultBackendLabel: String(view.default_backend_label || "").trim(),
    externalBaseLabel: String(view.external_base_label || "").trim(),
    externalBasePlaceholder: String(view.external_base_placeholder || "").trim(),
    advancedLabel: String(view.advanced_label || "").trim(),
    advancedObjectRequiredError: String(view.advanced_object_required_error || "").trim(),
    advancedInvalidJsonError: String(view.advanced_invalid_json_error || "").trim(),
    deployTitle: String(view.deploy_title || "").trim(),
    surfaceLabel: String(view.surface_label || "").trim(),
    deploySurfaceContractLabel: String(view.deploy_surface_contract_label || "").trim(),
    deploySurfaceLabels: settingsViewLabelMapFromSchema(view.deploy_surface_labels),
    trustLabel: String(view.trust_label || "").trim(),
    trustPolicyContractLabel: String(view.trust_policy_contract_label || "").trim(),
    trustPolicyLabels: settingsViewLabelMapFromSchema(view.trust_policy_labels),
    modelLabel: String(view.model_label || "").trim(),
    modelDefaultLabel: String(view.model_default_label || "").trim(),
    modelVendorFallback: String(view.model_vendor_fallback || "").trim(),
    durationLabel: String(view.duration_label || "").trim(),
    durationPlaceholder: String(view.duration_placeholder || "").trim(),
    toolCallsLabel: String(view.tool_calls_label || "").trim(),
    toolCallsMin: Number(view.tool_calls_min),
    toolCallsMax: Number(view.tool_calls_max),
    tokensLabel: String(view.tokens_label || "").trim(),
    tokensMin: Number(view.tokens_min),
    tokensMax: Number(view.tokens_max),
    realmLabel: String(view.realm_label || "").trim(),
    realmOptions: settingsViewOptionsFromSchema(view.realm_options),
    realmIdLabel: String(view.realm_id_label || "").trim(),
    realmIdPlaceholder: String(view.realm_id_placeholder || "").trim(),
    backendLabel: String(view.backend_label || "").trim(),
    realmBackendContractLabel: String(view.realm_backend_contract_label || "").trim(),
    realmBackendLabels: settingsViewLabelMapFromSchema(view.realm_backend_labels),
    promptLabel: String(view.prompt_label || "").trim(),
    promptPlaceholder: String(view.prompt_placeholder || "").trim(),
    commandLabel: String(view.command_label || "").trim(),
    commandFallback: String(view.command_fallback || "").trim(),
    inspectorTitle: String(view.inspector_title || "").trim(),
    inspectorLayoutLabel: String(view.inspector_layout_label || "").trim(),
    inspectorLayoutOptions: settingsViewOptionsFromSchema(view.inspector_layout_options),
  };
  const numericOk = [out.toolCallsMin, out.toolCallsMax, out.tokensMin, out.tokensMax].every(Number.isFinite);
  const optionsOk = out.edgeStyleOptions.length && out.densityOptions.length && out.themeModeOptions.length
    && out.autoWireOptions.length && out.realmOptions.length && out.inspectorLayoutOptions.length
    && Object.keys(out.deploySurfaceLabels).length && Object.keys(out.trustPolicyLabels).length
    && Object.keys(out.realmBackendLabels).length;
  const stringsOk = Object.entries(out).every(([key, value]) => {
    if (Array.isArray(value) || typeof value === "number") return true;
    return key === "optionSeparator" ? value.length > 0 : !!value;
  });
  return numericOk && optionsOk && stringsOk ? out : null;
}

export function settingsViewForState(settingsView) {
  const view = settingsView && typeof settingsView === "object" ? settingsView : null;
  return {
    optionSeparator: String(view?.optionSeparator || ""),
    unsupportedLabelSeparator: String(view?.unsupportedLabelSeparator || ""),
    unsupportedReasonPrefix: String(view?.unsupportedReasonPrefix || ""),
    unsupportedReasonSuffix: String(view?.unsupportedReasonSuffix || ""),
    canvasTitle: String(view?.canvasTitle || ""),
    edgeStyleLabel: String(view?.edgeStyleLabel || ""),
    edgeStyleOptions: Array.isArray(view?.edgeStyleOptions) ? view.edgeStyleOptions : [],
    densityLabel: String(view?.densityLabel || ""),
    densityOptions: Array.isArray(view?.densityOptions) ? view.densityOptions : [],
    themeTitle: String(view?.themeTitle || ""),
    themeModeLabel: String(view?.themeModeLabel || ""),
    themeModeOptions: Array.isArray(view?.themeModeOptions) ? view.themeModeOptions : [],
    mobTitle: String(view?.mobTitle || ""),
    orchestratorLabel: String(view?.orchestratorLabel || ""),
    profileNoneLabel: String(view?.profileNoneLabel || ""),
    autoWireLabel: String(view?.autoWireLabel || ""),
    autoWireOptions: Array.isArray(view?.autoWireOptions) ? view.autoWireOptions : [],
    roleWiringLabel: String(view?.roleWiringLabel || ""),
    roleWiringAddLabel: String(view?.roleWiringAddLabel || ""),
    defaultBackendLabel: String(view?.defaultBackendLabel || ""),
    externalBaseLabel: String(view?.externalBaseLabel || ""),
    externalBasePlaceholder: String(view?.externalBasePlaceholder || ""),
    advancedLabel: String(view?.advancedLabel || ""),
    advancedObjectRequiredError: String(view?.advancedObjectRequiredError || ""),
    advancedInvalidJsonError: String(view?.advancedInvalidJsonError || ""),
    deployTitle: String(view?.deployTitle || ""),
    surfaceLabel: String(view?.surfaceLabel || ""),
    deploySurfaceContractLabel: String(view?.deploySurfaceContractLabel || ""),
    deploySurfaceLabels: settingsViewLabelMapFromSchema(view?.deploySurfaceLabels),
    trustLabel: String(view?.trustLabel || ""),
    trustPolicyContractLabel: String(view?.trustPolicyContractLabel || ""),
    trustPolicyLabels: settingsViewLabelMapFromSchema(view?.trustPolicyLabels),
    modelLabel: String(view?.modelLabel || ""),
    modelDefaultLabel: String(view?.modelDefaultLabel || ""),
    modelVendorFallback: String(view?.modelVendorFallback || ""),
    durationLabel: String(view?.durationLabel || ""),
    durationPlaceholder: String(view?.durationPlaceholder || ""),
    toolCallsLabel: String(view?.toolCallsLabel || ""),
    toolCallsMin: Number(view?.toolCallsMin ?? NaN),
    toolCallsMax: Number(view?.toolCallsMax ?? NaN),
    tokensLabel: String(view?.tokensLabel || ""),
    tokensMin: Number(view?.tokensMin ?? NaN),
    tokensMax: Number(view?.tokensMax ?? NaN),
    realmLabel: String(view?.realmLabel || ""),
    realmOptions: Array.isArray(view?.realmOptions) ? view.realmOptions : [],
    realmIdLabel: String(view?.realmIdLabel || ""),
    realmIdPlaceholder: String(view?.realmIdPlaceholder || ""),
    backendLabel: String(view?.backendLabel || ""),
    realmBackendContractLabel: String(view?.realmBackendContractLabel || ""),
    realmBackendLabels: settingsViewLabelMapFromSchema(view?.realmBackendLabels),
    promptLabel: String(view?.promptLabel || ""),
    promptPlaceholder: String(view?.promptPlaceholder || ""),
    commandLabel: String(view?.commandLabel || ""),
    commandFallback: String(view?.commandFallback || ""),
    inspectorTitle: String(view?.inspectorTitle || ""),
    inspectorLayoutLabel: String(view?.inspectorLayoutLabel || ""),
    inspectorLayoutOptions: Array.isArray(view?.inspectorLayoutOptions) ? view.inspectorLayoutOptions : [],
  };
}

export function basicViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_basic_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    startLabel: String(view.start_label || "").trim(),
    loopBadge: String(view.loop_badge || "").trim(),
    tipsTitle: String(view.tips_title || "").trim(),
    emptyPanelTitle: String(view.empty_panel_title || "").trim(),
    emptyPanelSubtitleParts: basicViewPartsFromSchema(view.empty_panel_subtitle_parts),
    sourceToggleLabel: String(view.source_toggle_label || "").trim(),
    authoringOperationUnavailableError: String(view.authoring_operation_unavailable_error || "").trim(),
    authoringOperationFallbackError: String(view.authoring_operation_fallback_error || "").trim(),
    memberStepPanelTitleFallback: String(view.member_step_panel_title_fallback || "").trim(),
    memberStepPanelSubFallback: String(view.member_step_panel_sub_fallback || "").trim(),
    memberStepMemberLabel: String(view.member_step_member_label || "").trim(),
    memberStepMemberPlaceholder: String(view.member_step_member_placeholder || "").trim(),
    memberStepRuntimeDefaultLabel: String(view.member_step_runtime_default_label || "").trim(),
    memberStepInstructionLabel: String(view.member_step_instruction_label || "").trim(),
    memberStepInstructionPlaceholder: String(view.member_step_instruction_placeholder || "").trim(),
    memberStepDispatchLabel: String(view.member_step_dispatch_label || "").trim(),
    memberStepCollectionLabel: String(view.member_step_collection_label || "").trim(),
    memberStepQuorumLabel: String(view.member_step_quorum_label || "").trim(),
    memberStepQuorumPlaceholder: String(view.member_step_quorum_placeholder || "").trim(),
    memberStepTimeoutLabel: String(view.member_step_timeout_label || "").trim(),
    memberStepDependencyLabel: String(view.member_step_dependency_label || "").trim(),
    memberStepOutputFormatLabel: String(view.member_step_output_format_label || "").trim(),
    memberStepAllowedToolsLabel: String(view.member_step_allowed_tools_label || "").trim(),
    memberStepAllowedToolsEmptyLabel: String(view.member_step_allowed_tools_empty_label || "").trim(),
    memberStepBlockedToolsLabel: String(view.member_step_blocked_tools_label || "").trim(),
    memberStepBlockedToolsEmptyLabel: String(view.member_step_blocked_tools_empty_label || "").trim(),
    memberStepSchemaHintPrefix: String(view.member_step_schema_hint_prefix || ""),
    memberStepSchemaHintToolsPrefix: String(view.member_step_schema_hint_tools_prefix || ""),
    memberStepSchemaHintEmptyToolsLabel: String(view.member_step_schema_hint_empty_tools_label || "").trim(),
    toolScopeNotInCatalogReason: String(view.tool_scope_not_in_catalog_reason || "").trim(),
    toolScopeNotEnabledReason: String(view.tool_scope_not_enabled_reason || "").trim(),
    toolScopeToolDescriptionFallback: String(view.tool_scope_tool_description_fallback || "").trim(),
    toolScopeRemoveLabel: String(view.tool_scope_remove_label || "").trim(),
    toolScopeSelectMemberPlaceholder: String(view.tool_scope_select_member_placeholder || "").trim(),
    toolScopeBlockCatalogPlaceholder: String(view.tool_scope_block_catalog_placeholder || "").trim(),
    toolScopeAddProfilePlaceholder: String(view.tool_scope_add_profile_placeholder || "").trim(),
    inputPanelIcon: String(view.input_panel_icon || "").trim(),
    inputPanelTitle: String(view.input_panel_title || "").trim(),
    inputPanelSub: String(view.input_panel_sub || "").trim(),
    inputTaskLabel: String(view.input_task_label || "").trim(),
    inputTaskPlaceholder: String(view.input_task_placeholder || "").trim(),
    inputParamsTitlePrefix: String(view.input_params_title_prefix || "").trim(),
    inputAddParamLabel: String(view.input_add_param_label || "").trim(),
    inputParamSourceLabel: String(view.input_param_source_label || "").trim(),
    inputParamHeaderLabels: {
      name: String(view.input_param_header_labels?.name || "").trim(),
      type: String(view.input_param_header_labels?.type || "").trim(),
      required: String(view.input_param_header_labels?.required || "").trim(),
      description: String(view.input_param_header_labels?.description || "").trim(),
      action: String(view.input_param_header_labels?.action || ""),
    },
    inputParamNamePlaceholder: String(view.input_param_name_placeholder || "").trim(),
    inputParamDescriptionPlaceholder: String(view.input_param_description_placeholder || "").trim(),
    inputParamRemoveTitle: String(view.input_param_remove_title || "").trim(),
    inputParamEnumLabel: String(view.input_param_enum_label || "").trim(),
    inputParamEnumAddLabel: String(view.input_param_enum_add_label || "").trim(),
    inputParamEnumAddValue: String(view.input_param_enum_add_value || "").trim(),
    inputEmptyParamsParts: basicViewPartsFromSchema(view.input_empty_params_parts),
    inputTips: Array.isArray(view.input_tips)
      ? view.input_tips.map((tip) => String(tip || "").trim()).filter(Boolean)
      : [],
    branchPanelTitle: String(view.branch_panel_title || "").trim(),
    branchPanelSub: String(view.branch_panel_sub || "").trim(),
    parallelPanelTitle: String(view.parallel_panel_title || "").trim(),
    parallelPanelSub: String(view.parallel_panel_sub || "").trim(),
    branchRouteMemberLabel: String(view.branch_route_member_label || "").trim(),
    parallelJoinMemberLabel: String(view.parallel_join_member_label || "").trim(),
    branchControllerPlaceholderLabel: String(view.branch_controller_placeholder_label || "").trim(),
    branchEmptyControllerHint: String(view.branch_empty_controller_hint || "").trim(),
    branchConditionTitle: String(view.branch_condition_title || "").trim(),
    branchConditionIntro: String(view.branch_condition_intro || "").trim(),
    branchConditionRowTitlePrefix: String(view.branch_condition_row_title_prefix || "").trim(),
    branchConditionEmptyHint: String(view.branch_condition_empty_hint || "").trim(),
    branchConditionSourcePlaceholder: String(view.branch_condition_source_placeholder || "").trim(),
    branchConditionFieldPlaceholder: String(view.branch_condition_field_placeholder || "").trim(),
    branchConditionNoSchemaLabel: String(view.branch_condition_no_schema_label || "").trim(),
    branchConditionPreviewPrefix: String(view.branch_condition_preview_prefix || "").trim(),
    branchConditionPreviewFallback: String(view.branch_condition_preview_fallback || "").trim(),
    branchFallbackTitle: String(view.branch_fallback_title || "").trim(),
    branchFallbackHint: String(view.branch_fallback_hint || "").trim(),
    addBranchLabel: String(view.add_branch_label || "").trim(),
    addParallelBranchLabel: String(view.add_parallel_branch_label || "").trim(),
    parallelDispatchLabel: String(view.parallel_dispatch_label || "").trim(),
    parallelCollectionLabel: String(view.parallel_collection_label || "").trim(),
    parallelQuorumLabel: String(view.parallel_quorum_label || "").trim(),
    parallelQuorumPlaceholder: String(view.parallel_quorum_placeholder || "").trim(),
    branchDependencyLabel: String(view.branch_dependency_label || "").trim(),
    repeatPanelTitle: String(view.repeat_panel_title || "").trim(),
    repeatPanelSub: String(view.repeat_panel_sub || "").trim(),
    repeatLoopIdLabel: String(view.repeat_loop_id_label || "").trim(),
    repeatLoopIdPlaceholder: String(view.repeat_loop_id_placeholder || "").trim(),
    repeatConditionTitle: String(view.repeat_condition_title || "").trim(),
    repeatConditionIntro: String(view.repeat_condition_intro || "").trim(),
    repeatEmptyBodyHint: String(view.repeat_empty_body_hint || "").trim(),
    repeatMemberPlaceholderLabel: String(view.repeat_member_placeholder_label || "").trim(),
    repeatConditionFieldPlaceholder: String(view.repeat_condition_field_placeholder || "").trim(),
    repeatConditionNoSchemaLabel: String(view.repeat_condition_no_schema_label || "").trim(),
    repeatPreviewLabel: String(view.repeat_preview_label || "").trim(),
    repeatPreviewFallback: String(view.repeat_preview_fallback || "").trim(),
    repeatIterationInputLabel: String(view.repeat_iteration_input_label || "").trim(),
    repeatMaxIterationsLabel: String(view.repeat_max_iterations_label || "").trim(),
    repeatMaxIterationsPlaceholder: String(view.repeat_max_iterations_placeholder || "").trim(),
    repeatTips: Array.isArray(view.repeat_tips)
      ? view.repeat_tips.map((tip) => String(tip || "").trim()).filter(Boolean)
      : [],
    repeatCanvasWhileLabel: String(view.repeat_canvas_while_label || "").trim(),
    repeatCanvasNotLabel: String(view.repeat_canvas_not_label || "").trim(),
    repeatCanvasMissingMaxIterationsLabel: String(view.repeat_canvas_missing_max_iterations_label || "").trim(),
    repeatCanvasMaxIterationsPrefix: String(view.repeat_canvas_max_iterations_prefix || ""),
    repeatCanvasLoopBackPrefix: String(view.repeat_canvas_loop_back_prefix || ""),
    repeatCanvasExitPrefix: String(view.repeat_canvas_exit_prefix || ""),
    repeatCanvasExitFallback: String(view.repeat_canvas_exit_fallback || "").trim(),
    repeatIterationRuntimeDefaultLabel: String(view.repeat_iteration_runtime_default_label || "").trim(),
    repeatIterationCarryLabel: String(view.repeat_iteration_carry_label || "").trim(),
    repeatIterationReuseUnsupportedLabel: String(view.repeat_iteration_reuse_unsupported_label || "").trim(),
    repeatIterationFeedsUnsupportedPrefix: String(view.repeat_iteration_feeds_unsupported_prefix || ""),
    repeatIterationUnsupportedPrefix: String(view.repeat_iteration_unsupported_prefix || ""),
    addStepTitle: String(view.add_step_title || "").trim(),
    inputStepCardTitle: String(view.input_step_card_title || "").trim(),
    inputStepCardDescFallback: String(view.input_step_card_desc_fallback || "").trim(),
    branchStepCardTitle: String(view.branch_step_card_title || "").trim(),
    branchStepCardDesc: String(view.branch_step_card_desc || "").trim(),
    parallelStepCardTitle: String(view.parallel_step_card_title || "").trim(),
    parallelStepCardDescPrefix: String(view.parallel_step_card_desc_prefix || ""),
    parallelStepCardCollectionFallback: String(view.parallel_step_card_collection_fallback || "").trim(),
    repeatStepCardTitle: String(view.repeat_step_card_title || "").trim(),
    repeatStepCardDescPrefix: String(view.repeat_step_card_desc_prefix || ""),
    repeatStepCardDescFallback: String(view.repeat_step_card_desc_fallback || "").trim(),
    memberStepCardTitleFallback: String(view.member_step_card_title_fallback || "").trim(),
    pickerKickoffTitle: String(view.picker_kickoff_title || "").trim(),
    pickerKickoffSub: String(view.picker_kickoff_sub || "").trim(),
    pickerKickoffHint: String(view.picker_kickoff_hint || "").trim(),
    pickerTitle: String(view.picker_title || "").trim(),
    pickerSub: String(view.picker_sub || "").trim(),
    pickerSearchIcon: String(view.picker_search_icon || "").trim(),
    pickerSearchPlaceholder: String(view.picker_search_placeholder || "").trim(),
    pickerMembersLabel: String(view.picker_members_label || "").trim(),
    pickerFlowLabel: String(view.picker_flow_label || "").trim(),
    pickerEmptyMembersHint: String(view.picker_empty_members_hint || "").trim(),
    pickerNewBadgeLabel: String(view.picker_new_badge_label || "").trim(),
    adaptiveDefaults: basicAdaptiveDefaultsFromSchema(view.adaptive_defaults),
    adaptiveLimitRows: basicAdaptiveLimitRowsFromSchema(view.adaptive_limit_rows),
    adaptiveTargetSurfaces: Array.isArray(view.adaptive_target_surfaces)
      ? view.adaptive_target_surfaces.map((surface) => String(surface || "").trim()).filter(Boolean)
      : [],
    adaptivePanelTitle: String(view.adaptive_panel_title || "").trim(),
    adaptivePanelSub: String(view.adaptive_panel_sub || "").trim(),
    adaptiveFlowmasterLabel: String(view.adaptive_flowmaster_label || "").trim(),
    adaptiveFlowmasterPlaceholderLabel: String(view.adaptive_flowmaster_placeholder_label || "").trim(),
    adaptivePromptLabel: String(view.adaptive_prompt_label || "").trim(),
    adaptivePromptPlaceholder: String(view.adaptive_prompt_placeholder || "").trim(),
    adaptiveObjectiveLabel: String(view.adaptive_objective_label || "").trim(),
    adaptiveObjectivePlaceholder: String(view.adaptive_objective_placeholder || "").trim(),
    adaptiveResultSchemaLabel: String(view.adaptive_result_schema_label || "").trim(),
    adaptiveResultSchemaPlaceholderLabel: String(view.adaptive_result_schema_placeholder_label || "").trim(),
    adaptiveProfilesTitle: String(view.adaptive_profiles_title || "").trim(),
    adaptiveProfilesHint: String(view.adaptive_profiles_hint || "").trim(),
    adaptiveProfilesAgentsHint: String(view.adaptive_profiles_agents_hint || "").trim(),
    adaptiveProfilesEmptyHint: String(view.adaptive_profiles_empty_hint || "").trim(),
    adaptiveSurfacesLabel: String(view.adaptive_surfaces_label || "").trim(),
    adaptiveInlineProfilesLabel: String(view.adaptive_inline_profiles_label || "").trim(),
    adaptiveLimitsTitle: String(view.adaptive_limits_title || "").trim(),
    adaptiveLimitsAdvancedLabel: String(view.adaptive_limits_advanced_label || "").trim(),
    adaptiveTips: Array.isArray(view.adaptive_tips)
      ? view.adaptive_tips.map((tip) => String(tip || "").trim()).filter(Boolean)
      : [],
    adaptiveStepCardTitle: String(view.adaptive_step_card_title || "").trim(),
    adaptiveBlockHeadPrefix: String(view.adaptive_block_head_prefix || ""),
    adaptiveBlockFlowmasterKicker: String(view.adaptive_block_flowmaster_kicker || "").trim(),
    adaptiveBlockFlowmasterTitleFallback: String(view.adaptive_block_flowmaster_title_fallback || "").trim(),
    adaptiveBlockFlowmasterDesc: String(view.adaptive_block_flowmaster_desc || "").trim(),
    adaptiveBlockPlanConnector: String(view.adaptive_block_plan_connector || "").trim(),
    adaptiveBlockLayerKickerPrefix: String(view.adaptive_block_layer_kicker_prefix || ""),
    adaptiveBlockLayerKickerSuffix: String(view.adaptive_block_layer_kicker_suffix || ""),
    adaptiveBlockChipSuffix: String(view.adaptive_block_chip_suffix || "").trim(),
    adaptiveBlockEmptyProfilesLabel: String(view.adaptive_block_empty_profiles_label || "").trim(),
    adaptiveBlockCollectConnectorPrefix: String(view.adaptive_block_collect_connector_prefix || ""),
    adaptiveBlockCollectFallback: String(view.adaptive_block_collect_fallback || "").trim(),
    adaptiveBlockLoopBackPrefix: String(view.adaptive_block_loop_back_prefix || ""),
    adaptiveBlockLoopBackSuffix: String(view.adaptive_block_loop_back_suffix || ""),
    flowPrimitiveRows: basicFlowPrimitiveRowsFromSchema(view.flow_primitive_rows),
  };
  return Object.entries(out).every(([key, value]) => {
    if (key === "inputParamHeaderLabels") {
      return value.name && value.type && value.required && value.description;
    }
    return Array.isArray(value) ? value.length : !!value;
  })
    ? out
    : null;
}

export function basicViewPartsFromSchema(parts) {
  if (!Array.isArray(parts)) return [];
  return parts
    .map((part, index) => {
      if (!part || typeof part !== "object") return null;
      const kind = String(part.kind || "text").trim();
      const text = String(part.text || "");
      if (!text) return null;
      return {
        key: String(part.key || `${kind}-${index}`),
        kind: kind === "code" || kind === "strong" ? kind : "text",
        text,
      };
    })
    .filter(Boolean);
}

export function basicFlowPrimitiveRowsFromSchema(rows) {
  if (!Array.isArray(rows)) return [];
  return rows
    .map((row) => {
      if (!row || typeof row !== "object") return null;
      const id = String(row.id || "").trim();
      const glyph = String(row.glyph || "").trim();
      const tint = String(row.tint || "").trim();
      const label = String(row.label || "").trim();
      const sub = String(row.sub || "").trim();
      if (!id || !glyph || !tint || !label || !sub) return null;
      const disabledReason = String(row.disabled_reason || "").trim();
      return {
        id,
        glyph,
        tint,
        label,
        sub,
        isNew: Boolean(row.is_new),
        disabled: Boolean(row.disabled),
        disabledReason,
      };
    })
    .filter(Boolean);
}

// Server-authoritative default payload for adaptive flow steps
// (editor_basic_view.adaptive_defaults). The client never hardcodes adaptive
// defaults — inserts and normalization hydrate from this object. prompt and
// objectiveClass are free text and carry through verbatim (never trimmed).
export function basicAdaptiveDefaultsFromSchema(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  if (String(value.type || "") !== "adaptive") return null;
  const limits = value.limits && typeof value.limits === "object" && !Array.isArray(value.limits)
    ? value.limits
    : null;
  if (!limits || !Object.keys(limits).length) return null;
  return {
    type: "adaptive",
    flowmasterId: String(value.flowmasterId ?? ""),
    prompt: typeof value.prompt === "string" ? value.prompt : "",
    objectiveClass: typeof value.objectiveClass === "string" ? value.objectiveClass : "",
    resultSchema: String(value.resultSchema ?? ""),
    profileTemplateIds: Array.isArray(value.profileTemplateIds)
      ? value.profileTemplateIds.map((id) => String(id || "").trim()).filter(Boolean)
      : [],
    targetSurfaces: Array.isArray(value.targetSurfaces)
      ? value.targetSurfaces.map((surface) => String(surface || "").trim()).filter(Boolean)
      : [],
    allowInlineProfiles: value.allowInlineProfiles === true,
    limits: { ...limits },
  };
}

// Ordered limit rows (editor_basic_view.adaptive_limit_rows): the 12
// AdaptiveLimitRecord entries in field order, each {key, toml_key, label,
// primary} — primary flags the three design-prominent limits.
export function basicAdaptiveLimitRowsFromSchema(rows) {
  if (!Array.isArray(rows)) return [];
  return rows
    .map((row) => {
      if (!row || typeof row !== "object") return null;
      const key = String(row.key || "").trim();
      const tomlKey = String(row.toml_key || "").trim();
      const label = String(row.label || "").trim();
      if (!key || !tomlKey || !label) return null;
      return { key, tomlKey, label, primary: Boolean(row.primary) };
    })
    .filter(Boolean);
}

export function viewStringMapFromSchema(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value)
      .map(([key, label]) => [String(key || "").trim(), String(label || "").trim()])
      .filter(([key, label]) => key && label),
  );
}

export function launchViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_launch_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    launchTitle: String(view.launch_title || "").trim(),
    graphLaunchTitle: String(view.graph_launch_title || "").trim(),
    resumeSessionLabel: String(view.resume_session_label || "").trim(),
    resumeSessionPlaceholder: String(view.resume_session_placeholder || "").trim(),
    forkSourceLabel: String(view.fork_source_label || "").trim(),
    forkContextLabel: String(view.fork_context_label || "").trim(),
    graphForkContextLabel: String(view.graph_fork_context_label || "").trim(),
    unsupportedLabelSeparator: String(view.unsupported_label_separator || ""),
    unsupportedReasonPrefix: String(view.unsupported_reason_prefix || ""),
    unsupportedReasonSuffix: String(view.unsupported_reason_suffix || ""),
    launchModesContractLabel: String(view.launch_modes_contract_label || "").trim(),
    forkContextsContractLabel: String(view.fork_contexts_contract_label || "").trim(),
    launchModeLabels: viewStringMapFromSchema(view.launch_mode_labels),
    forkContextLabels: viewStringMapFromSchema(view.fork_context_labels),
  };
  const stringsOk = Object.entries(out).every(([key, value]) => {
    if (typeof value === "number") return Number.isFinite(value) && value > 0;
    if (value && typeof value === "object") return Object.keys(value).length > 0;
    return !!value;
  });
  return stringsOk ? out : null;
}

export function launchViewForState(launchView) {
  const view = launchView && typeof launchView === "object" ? launchView : null;
  return {
    launchTitle: String(view?.launchTitle || ""),
    graphLaunchTitle: String(view?.graphLaunchTitle || ""),
    resumeSessionLabel: String(view?.resumeSessionLabel || ""),
    resumeSessionPlaceholder: String(view?.resumeSessionPlaceholder || ""),
    forkSourceLabel: String(view?.forkSourceLabel || ""),
    forkContextLabel: String(view?.forkContextLabel || ""),
    graphForkContextLabel: String(view?.graphForkContextLabel || ""),
    unsupportedLabelSeparator: String(view?.unsupportedLabelSeparator || ""),
    unsupportedReasonPrefix: String(view?.unsupportedReasonPrefix || ""),
    unsupportedReasonSuffix: String(view?.unsupportedReasonSuffix || ""),
    launchModesContractLabel: String(view?.launchModesContractLabel || ""),
    forkContextsContractLabel: String(view?.forkContextsContractLabel || ""),
    launchModeLabels: view?.launchModeLabels && typeof view.launchModeLabels === "object" ? view.launchModeLabels : {},
    forkContextLabels: view?.forkContextLabels && typeof view.forkContextLabels === "object" ? view.forkContextLabels : {},
  };
}

export function graphTemplateViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_graph_template_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    templateEyebrow: String(view.template_eyebrow || "").trim(),
    summaryTitle: String(view.summary_title || "").trim(),
    triggersTitle: String(view.triggers_title || "").trim(),
    triggerLabelsLabel: String(view.trigger_labels_label || "").trim(),
    triggerDefaultLabel: String(view.trigger_default_label || "").trim(),
    defaultYesLabel: String(view.default_yes_label || "").trim(),
    defaultNoLabel: String(view.default_no_label || "").trim(),
    summaryMembersLabel: String(view.summary_members_label || "").trim(),
    summaryInstancesLabel: String(view.summary_instances_label || "").trim(),
    summaryTerminalsLabel: String(view.summary_terminals_label || "").trim(),
    summaryEdgesLabel: String(view.summary_edges_label || "").trim(),
    summaryFramesLabel: String(view.summary_frames_label || "").trim(),
    summaryMembersValueTemplate: String(view.summary_members_value_template || "").trim(),
    quickStartTitle: String(view.quick_start_title || "").trim(),
    quickStartRows: graphTemplateQuickStartRowsFromSchema(view.quick_start_rows),
  };
  return out.templateEyebrow && out.summaryTitle && out.triggersTitle && out.triggerLabelsLabel
    && out.triggerDefaultLabel && out.defaultYesLabel && out.defaultNoLabel
    && out.summaryMembersLabel && out.summaryInstancesLabel && out.summaryTerminalsLabel
    && out.summaryEdgesLabel && out.summaryFramesLabel && out.summaryMembersValueTemplate
    && out.quickStartTitle && out.quickStartRows.length
    ? out
    : null;
}

export function graphTemplateQuickStartRowsFromSchema(rows) {
  if (!Array.isArray(rows)) return [];
  return rows
    .map((row, rowIndex) => ({
      key: `quick-start-${rowIndex}`,
      parts: basicViewPartsFromSchema(row),
    }))
    .filter((row) => row.parts.length);
}

export function graphTemplateViewForState(templateView) {
  const view = templateView && typeof templateView === "object" ? templateView : null;
  return {
    templateEyebrow: String(view?.templateEyebrow || ""),
    summaryTitle: String(view?.summaryTitle || ""),
    triggersTitle: String(view?.triggersTitle || ""),
    triggerLabelsLabel: String(view?.triggerLabelsLabel || ""),
    triggerDefaultLabel: String(view?.triggerDefaultLabel || ""),
    defaultYesLabel: String(view?.defaultYesLabel || ""),
    defaultNoLabel: String(view?.defaultNoLabel || ""),
    summaryMembersLabel: String(view?.summaryMembersLabel || ""),
    summaryInstancesLabel: String(view?.summaryInstancesLabel || ""),
    summaryTerminalsLabel: String(view?.summaryTerminalsLabel || ""),
    summaryEdgesLabel: String(view?.summaryEdgesLabel || ""),
    summaryFramesLabel: String(view?.summaryFramesLabel || ""),
    summaryMembersValueTemplate: String(view?.summaryMembersValueTemplate || ""),
    quickStartTitle: String(view?.quickStartTitle || ""),
    quickStartRows: Array.isArray(view?.quickStartRows) ? view.quickStartRows : [],
  };
}

export function graphViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_graph_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    zoomOutTitle: String(view.zoom_out_title || "").trim(),
    fitTitle: String(view.fit_title || "").trim(),
    zoomInTitle: String(view.zoom_in_title || "").trim(),
    portDragTitle: String(view.port_drag_title || "").trim(),
    addNodeSearchIcon: String(view.add_node_search_icon || "").trim(),
    addNodeSearchPlaceholder: String(view.add_node_search_placeholder || "").trim(),
    addNodeCloseLabel: String(view.add_node_close_label || "").trim(),
    addNodeCloseTitle: String(view.add_node_close_title || "").trim(),
    addNodeAgentsLabel: String(view.add_node_agents_label || "").trim(),
    addNodeControlsLabel: String(view.add_node_controls_label || "").trim(),
    addNodeTerminalsLabel: String(view.add_node_terminals_label || "").trim(),
    addNodeEmptyPrefix: String(view.add_node_empty_prefix || ""),
    addNodeEmptySuffix: String(view.add_node_empty_suffix || ""),
    addNodeJumpLabel: String(view.add_node_jump_label || "").trim(),
    authoringOperationUnavailableError: String(view.authoring_operation_unavailable_error || "").trim(),
    authoringOperationFallbackError: String(view.authoring_operation_fallback_error || "").trim(),
    gatePaletteRows: graphGatePaletteRowsFromSchema(view.gate_palette_rows),
    terminalPaletteRows: graphTerminalPaletteRowsFromSchema(view.terminal_palette_rows),
    gateKindLabels: viewStringMapFromSchema(view.graph_gate_kind_labels),
    terminalKindLabels: viewStringMapFromSchema(view.graph_terminal_kind_labels),
    frameKindLabels: viewStringMapFromSchema(view.graph_frame_kind_labels),
    edgeKindLabels: viewStringMapFromSchema(view.graph_edge_kind_labels),
    inspectorDeleteLabel: String(view.inspector_delete_label || "").trim(),
    inspectorLabelTitle: String(view.inspector_label_title || "").trim(),
    inspectorKindTitle: String(view.inspector_kind_title || "").trim(),
    inspectorRuntimeDefaultLabel: String(view.inspector_runtime_default_label || "").trim(),
    instanceEyebrow: String(view.instance_eyebrow || "").trim(),
    instanceIdLineTemplate: String(view.instance_id_line_template || "").trim(),
    instanceMemberRoleTemplate: String(view.instance_member_role_template || "").trim(),
    instanceEditMemberLabel: String(view.instance_edit_member_label || "").trim(),
    instanceModelLabel: String(view.instance_model_label || "").trim(),
    instanceSchemaLabel: String(view.instance_schema_label || "").trim(),
    instanceToolsLabel: String(view.instance_tools_label || "").trim(),
    instanceMemberHint: String(view.instance_member_hint || "").trim(),
    instancePositionTitle: String(view.instance_position_title || "").trim(),
    instancePositionStageLabel: String(view.instance_position_stage_label || "").trim(),
    instancePositionSlotLabel: String(view.instance_position_slot_label || "").trim(),
    instanceOutputTitleTemplate: String(view.instance_output_title_template || "").trim(),
    instanceOutputRequiredLabel: String(view.instance_output_required_label || "").trim(),
    instanceOutputHint: String(view.instance_output_hint || "").trim(),
    instanceOutputOpenMemberLabel: String(view.instance_output_open_member_label || "").trim(),
    gateEyebrowTemplate: String(view.gate_eyebrow_template || "").trim(),
    gateIdLineTemplate: String(view.gate_id_line_template || "").trim(),
    gateQuorumIncomingTemplate: String(view.gate_quorum_incoming_template || "").trim(),
    gateMemberOptionTemplate: String(view.gate_member_option_template || "").trim(),
    terminalEyebrowTemplate: String(view.terminal_eyebrow_template || "").trim(),
    terminalIdLineTemplate: String(view.terminal_id_line_template || "").trim(),
    terminalAuthoringLockedTitle: String(view.terminal_authoring_locked_title || "").trim(),
    terminalAuthoringLockedHint: String(view.terminal_authoring_locked_hint || "").trim(),
    adaptiveAuthoringLockedTitle: String(view.adaptive_authoring_locked_title || "").trim(),
    adaptiveAuthoringLockedHint: String(view.adaptive_authoring_locked_hint || "").trim(),
    edgeEyebrowTemplate: String(view.edge_eyebrow_template || "").trim(),
    edgeTitleTemplate: String(view.edge_title_template || "").trim(),
    edgeIdLineTemplate: String(view.edge_id_line_template || "").trim(),
    edgeFieldPlaceholder: String(view.edge_field_placeholder || "").trim(),
    edgeFieldNoSchemaPlaceholder: String(view.edge_field_no_schema_placeholder || "").trim(),
    gateCollectionTitle: String(view.gate_collection_title || "").trim(),
    gateJoinMemberLabel: String(view.gate_join_member_label || "").trim(),
    gateJoinMemberPlaceholder: String(view.gate_join_member_placeholder || "").trim(),
    gateJoinMemberHint: String(view.gate_join_member_hint || "").trim(),
    gateDispatchTitle: String(view.gate_dispatch_title || "").trim(),
    gateDispatchHint: String(view.gate_dispatch_hint || "").trim(),
    gateConditionsTitle: String(view.gate_conditions_title || "").trim(),
    gateEmptyBranchHint: String(view.gate_empty_branch_hint || "").trim(),
    gateWiringTitle: String(view.gate_wiring_title || "").trim(),
    gateIncomingLabel: String(view.gate_incoming_label || "").trim(),
    gateOutgoingLabel: String(view.gate_outgoing_label || "").trim(),
    branchConditionModeConditionLabel: String(view.branch_condition_mode_condition_label || "").trim(),
    branchConditionModeFallbackLabel: String(view.branch_condition_mode_fallback_label || "").trim(),
    branchConditionTargetPrefix: String(view.branch_condition_target_prefix || "").trim(),
    graphConditionTargetMissingLabel: String(view.graph_condition_target_missing_label || "").trim(),
    graphConditionOwnerOptionTemplate: String(view.graph_condition_owner_option_template || "").trim(),
    graphConditionFieldOptionTemplate: String(view.graph_condition_field_option_template || "").trim(),
    graphInputParamSourceLabel: String(view.branch_input_param_source_label || "").trim(),
    sourceFileLabel: String(view.source_file_label || "").trim(),
    sourceFileAriaLabel: String(view.source_file_aria_label || "").trim(),
    sourceFileGlyph: String(view.source_file_glyph || "").trim(),
    sourceFileRoleLabel: String(view.source_file_role_label || "").trim(),
    sourceFileNodeId: String(view.source_file_node_id || "").trim(),
    sourceFileNodeKind: String(view.source_file_node_kind || "").trim(),
    sourceFileNodeColOffset: Number(view.source_file_node_col_offset || 0),
    sourceFileNodeRowOffset: Number(view.source_file_node_row_offset || 0),
    sourceFileActivationHash: String(view.source_file_activation_hash || "").trim(),
    sourceFileActivationSelector: String(view.source_file_activation_selector || "").trim(),
    branchConditionFieldPlaceholder: String(view.branch_condition_field_placeholder || "").trim(),
    branchConditionNoOptionsHint: String(view.branch_condition_no_options_hint || "").trim(),
    edgeConditionTitle: String(view.edge_condition_title || "").trim(),
    edgeNoConditionOptionsHint: String(view.edge_no_condition_options_hint || "").trim(),
    edgeOwnerPlaceholder: String(view.edge_owner_placeholder || "").trim(),
    edgeFromTitle: String(view.edge_from_title || "").trim(),
    edgeToTitle: String(view.edge_to_title || "").trim(),
    edgeRowInstanceLabel: String(view.edge_row_instance_label || "").trim(),
    edgeRowMemberLabel: String(view.edge_row_member_label || "").trim(),
    edgeRowSchemaLabel: String(view.edge_row_schema_label || "").trim(),
    edgeRowMissingValue: String(view.edge_row_missing_value || "").trim(),
    edgeTerminalMemberValue: String(view.edge_terminal_member_value || "").trim(),
  };
  return out.zoomOutTitle && out.fitTitle && out.zoomInTitle && out.portDragTitle
    && out.addNodeSearchIcon && out.addNodeSearchPlaceholder && out.addNodeCloseLabel
    && out.addNodeCloseTitle && out.addNodeAgentsLabel && out.addNodeControlsLabel
    && out.addNodeTerminalsLabel && out.addNodeEmptyPrefix && out.addNodeEmptySuffix && out.addNodeJumpLabel
    && out.authoringOperationUnavailableError && out.authoringOperationFallbackError
    && out.gatePaletteRows.length && out.terminalPaletteRows.length
    && Object.keys(out.gateKindLabels).length
    && Object.keys(out.terminalKindLabels).length
    && Object.keys(out.frameKindLabels).length
    && Object.keys(out.edgeKindLabels).length
    && out.inspectorDeleteLabel && out.inspectorLabelTitle && out.inspectorKindTitle
    && out.inspectorRuntimeDefaultLabel && out.instanceEyebrow && out.instanceIdLineTemplate
    && out.instanceMemberRoleTemplate && out.instanceEditMemberLabel && out.instanceModelLabel
    && out.instanceSchemaLabel && out.instanceToolsLabel && out.instanceMemberHint
    && out.instancePositionTitle && out.instancePositionStageLabel && out.instancePositionSlotLabel
    && out.instanceOutputTitleTemplate && out.instanceOutputRequiredLabel && out.instanceOutputHint
    && out.instanceOutputOpenMemberLabel && out.gateEyebrowTemplate && out.gateIdLineTemplate
    && out.gateQuorumIncomingTemplate && out.gateMemberOptionTemplate
    && out.terminalEyebrowTemplate && out.terminalIdLineTemplate
    && out.terminalAuthoringLockedTitle && out.terminalAuthoringLockedHint
    && out.adaptiveAuthoringLockedTitle && out.adaptiveAuthoringLockedHint
    && out.edgeEyebrowTemplate
    && out.edgeTitleTemplate && out.edgeIdLineTemplate && out.edgeFieldPlaceholder
    && out.edgeFieldNoSchemaPlaceholder
    && out.gateCollectionTitle
    && out.gateJoinMemberLabel && out.gateJoinMemberPlaceholder && out.gateJoinMemberHint
    && out.gateDispatchTitle && out.gateDispatchHint && out.gateConditionsTitle
    && out.gateEmptyBranchHint && out.gateWiringTitle && out.gateIncomingLabel
    && out.gateOutgoingLabel && out.branchConditionModeConditionLabel
    && out.branchConditionModeFallbackLabel && out.branchConditionTargetPrefix
    && out.graphConditionTargetMissingLabel && out.graphConditionOwnerOptionTemplate
    && out.graphConditionFieldOptionTemplate
    && out.graphInputParamSourceLabel && out.sourceFileLabel
    && out.sourceFileAriaLabel && out.sourceFileGlyph && out.sourceFileRoleLabel
    && out.sourceFileNodeId && out.sourceFileNodeKind
    && Number.isFinite(out.sourceFileNodeColOffset) && Number.isFinite(out.sourceFileNodeRowOffset)
    && out.sourceFileActivationHash && out.sourceFileActivationSelector
    && out.branchConditionFieldPlaceholder && out.branchConditionNoOptionsHint
    && out.edgeConditionTitle && out.edgeNoConditionOptionsHint && out.edgeOwnerPlaceholder
    && out.edgeFromTitle && out.edgeToTitle && out.edgeRowInstanceLabel
    && out.edgeRowMemberLabel && out.edgeRowSchemaLabel && out.edgeRowMissingValue
    && out.edgeTerminalMemberValue
    ? out
    : null;
}

export function graphGatePaletteRowsFromSchema(rows) {
  if (!Array.isArray(rows)) return [];
  return rows
    .map((row) => {
      if (!row || typeof row !== "object") return null;
      const id = String(row.id || "").trim();
      const glyph = String(row.glyph || "").trim();
      const label = String(row.label || "").trim();
      const meta = String(row.meta || "").trim();
      if (!id || !glyph || !label || !meta) return null;
      return { id, glyph, label, meta };
    })
    .filter(Boolean);
}

export function graphTerminalPaletteRowsFromSchema(rows) {
  if (!Array.isArray(rows)) return [];
  return rows
    .map((row) => {
      if (!row || typeof row !== "object") return null;
      const id = String(row.id || "").trim();
      const glyph = String(row.glyph || "").trim();
      const label = String(row.label || "").trim();
      const meta = String(row.meta || "").trim();
      if (!id || !glyph || !label || !meta) return null;
      return { id, glyph, label, meta };
    })
    .filter(Boolean);
}
