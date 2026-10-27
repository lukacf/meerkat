// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the schema-field-edit functions move byte-verbatim as plain JS,
// and their destructured `= {}` parameter defaults raise TS2339 under .ts
// semantics (e.g. the cascade patches' `{ schema, schemas, flow, ... } = {}`
// and schemaFieldRowControlState's `overrides = {}`). Source-contract pins
// this exact text, so suppression must live at file level, not in the moved
// bodies. Resolution/linkage stays guarded behaviorally: the projection
// suite and export-keys test load the bundle and exercise these functions,
// so a missed import or re-export still fails the gate as a ReferenceError.
//
// Schema field editing for the Flow Editor controller plane. Moved verbatim
// from the controller.js schema-field-edit range: condition/error view
// hydration and state, conditionValueControl, input-param and schema-field
// naming, enum value patches, schema-like field type/required/description
// patches and control states, and the schema-field update/rename/delete
// cascade patches.
//
// The basicEditorViewState straggler edge went through the lazy
// _residue-bridge until editors/basic-editor.ts landed in S11; it is now a
// relative import (a runtime-only cycle with basic-editor's
// uniqueInputParamName/normalizeSchemaLikeFieldPatch imports — no
// module-init cross-calls). reconcileConditionFieldAvailability and
// reconcileSchemaFieldReferences landed in flow/reconcile.ts in S8 and are
// imported relatively (a runtime-only cycle with reconcile's
// enumValuesForField import — no module-init cross-calls), like
// contractDefaultValue and schemaFieldTypeOptions from contract/options.ts
// in S6 and the two facade-internal stragglers no bridge could reach —
// contractStringValues and editorSchemaFieldNameFallback — which were
// co-moved into their design-destined homes (contract/options.ts,
// drafts/mob-settings.ts) in S5.
import {
  contractDefaultValue,
  contractStringValues,
  schemaFieldTypeOptions,
} from "../contract/options";
import { editorSchemaFieldNameFallback } from "../drafts/mob-settings";
import { basicEditorViewState } from "../editors/basic-editor";
import {
  reconcileConditionFieldAvailability,
  reconcileSchemaFieldReferences,
} from "../flow/reconcile";
import { schemaViewForState, viewStringMapFromSchema } from "../views/view-config";

export function conditionViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_condition_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    emptyValueLabel: String(view.empty_value_label || "").trim(),
    textValuePlaceholder: String(view.text_value_placeholder || "").trim(),
  };
  return Object.values(out).every(Boolean) ? out : null;
}

export function conditionViewForState(conditionView) {
  const view = conditionView && typeof conditionView === "object" ? conditionView : null;
  return {
    emptyValueLabel: String(view?.emptyValueLabel || ""),
    textValuePlaceholder: String(view?.textValuePlaceholder || ""),
  };
}

export function errorViewFromSchema(schema) {
  const view = schema?.mob_definition?.editor_error_view;
  if (!view || typeof view !== "object") return null;
  const out = {
    criticalGlyph: String(view.critical_glyph || "").trim(),
    genericErrorHead: String(view.generic_error_head || "").trim(),
    deployFailedHead: String(view.deploy_failed_head || "").trim(),
    deployPlanFailedHead: String(view.deploy_plan_failed_head || "").trim(),
    deployErrorMeta: String(view.deploy_error_meta || "").trim(),
    sourceFailedHead: String(view.source_failed_head || "").trim(),
    sourceErrorMeta: String(view.source_error_meta || "").trim(),
    validationApiFailedHead: String(view.validation_api_failed_head || "").trim(),
    rpcErrorMeta: String(view.rpc_error_meta || "").trim(),
    authoringOperationFailedHead: String(view.authoring_operation_failed_head || "").trim(),
    authoringOperationMeta: String(view.authoring_operation_meta || "").trim(),
    authoringOperationFallbackHeads: viewStringMapFromSchema(view.authoring_operation_fallback_heads),
    authoringOperationStaleError: String(view.authoring_operation_stale_error || "").trim(),
    authoringOperationMissingDocumentError: String(view.authoring_operation_missing_document_error || "").trim(),
    exportFailedHead: String(view.export_failed_head || "").trim(),
    importFailedHead: String(view.import_failed_head || "").trim(),
    bootOpenMissingHead: String(view.boot_open_missing_head || "").trim(),
    bootOpenMissingMeta: String(view.boot_open_missing_meta || "").trim(),
    missingEditorFlowHead: String(view.missing_editor_flow_head || "").trim(),
    missingEditorFlowSub: String(view.missing_editor_flow_sub || "").trim(),
    missingEditorFlowMeta: String(view.missing_editor_flow_meta || "").trim(),
  };
  return Object.values(out).every(Boolean) ? out : null;
}

export function errorViewForState(errorView) {
  const view = errorView && typeof errorView === "object" ? errorView : null;
  return {
    criticalGlyph: String(view?.criticalGlyph || ""),
    genericErrorHead: String(view?.genericErrorHead || ""),
    deployFailedHead: String(view?.deployFailedHead || ""),
    deployPlanFailedHead: String(view?.deployPlanFailedHead || ""),
    deployErrorMeta: String(view?.deployErrorMeta || ""),
    sourceFailedHead: String(view?.sourceFailedHead || ""),
    sourceErrorMeta: String(view?.sourceErrorMeta || ""),
    validationApiFailedHead: String(view?.validationApiFailedHead || ""),
    rpcErrorMeta: String(view?.rpcErrorMeta || ""),
    authoringOperationFailedHead: String(view?.authoringOperationFailedHead || ""),
    authoringOperationMeta: String(view?.authoringOperationMeta || ""),
    authoringOperationFallbackHeads: view?.authoringOperationFallbackHeads && typeof view.authoringOperationFallbackHeads === "object"
      ? view.authoringOperationFallbackHeads
      : {},
    authoringOperationStaleError: String(view?.authoringOperationStaleError || ""),
    authoringOperationMissingDocumentError: String(view?.authoringOperationMissingDocumentError || ""),
    exportFailedHead: String(view?.exportFailedHead || ""),
    importFailedHead: String(view?.importFailedHead || ""),
    bootOpenMissingHead: String(view?.bootOpenMissingHead || ""),
    bootOpenMissingMeta: String(view?.bootOpenMissingMeta || ""),
    missingEditorFlowHead: String(view?.missingEditorFlowHead || ""),
    missingEditorFlowSub: String(view?.missingEditorFlowSub || ""),
    missingEditorFlowMeta: String(view?.missingEditorFlowMeta || ""),
  };
}

export function conditionValueControl(field, rawValue = "", conditionView = null) {
  const view = conditionViewForState(conditionView);
  const type = String(field?.type || "").trim();
  const value = rawValue == null ? "" : String(rawValue);
  const optionRows = (values) => [
    { value: "", label: view.emptyValueLabel },
    ...values.map((candidate) => ({ value: candidate, label: candidate })),
  ];
  if (type === "enum" && Array.isArray(field?.enumValues) && field.enumValues.length) {
    const values = field.enumValues.map(String);
    return { kind: "enum", values, value, optionRows: optionRows(values), placeholder: "" };
  }
  if (type === "boolean" || type === "bool") {
    const values = ["true", "false"];
    return { kind: "boolean", values, value, optionRows: optionRows(values), placeholder: "" };
  }
  return { kind: "text", values: [], value, optionRows: [], placeholder: view.textValuePlaceholder };
}

export function inputParamName(raw, fallback = "field") {
  return String(raw || fallback)
    .trim()
    .replace(/[^A-Za-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "")
    .replace(/^[0-9]/, "_$&") || fallback;
}

export function uniqueInputParamName(params, raw, currentId = null, fallback = "param") {
  const base = inputParamName(raw, fallback);
  const taken = new Set((params || [])
    .filter((param) => param?.id !== currentId)
    .map((param) => String(param?.name || "").trim())
    .filter(Boolean));
  if (!taken.has(base)) return base;
  let i = 2;
  while (taken.has(`${base}_${i}`)) i += 1;
  return `${base}_${i}`;
}

export function schemaFieldName(raw, fallback = "field") {
  return inputParamName(raw, fallback);
}

export function uniqueSchemaFieldName(fields, raw, currentId = null, fallback = "field") {
  const base = schemaFieldName(raw, fallback);
  const taken = new Set((fields || [])
    .filter((field) => field?.id !== currentId)
    .map((field) => String(field?.name || "").trim())
    .filter(Boolean));
  if (!taken.has(base)) return base;
  let i = 2;
  while (taken.has(`${base}_${i}`)) i += 1;
  return `${base}_${i}`;
}

export function schemaDescriptionPatch(rawDescription) {
  return { description: String(rawDescription || "") };
}

export function enumValuesForField(field) {
  return Array.isArray(field?.enumValues) ? field.enumValues : [];
}

export function uniqueEnumValue(values, raw, index = null) {
  const base = String(raw || "value").trim() || "value";
  const taken = new Set((Array.isArray(values) ? values : [])
    .filter((_, i) => i !== index)
    .map((value) => String(value || "").trim())
    .filter(Boolean));
  if (!taken.has(base)) return base;
  let i = 2;
  while (taken.has(`${base}_${i}`)) i += 1;
  return `${base}_${i}`;
}

export function schemaFieldTypeAllowedSet(contract) {
  return new Set(contractStringValues(contract?.mob_definition?.editor_schema_field_types));
}

export function schemaLikeFieldTypePatch(field, rawType, contract) {
  const type = String(rawType || "").trim();
  const allowedTypes = schemaFieldTypeAllowedSet(contract);
  if (!type || !allowedTypes.has(type)) {
    return {};
  }
  const values = enumValuesForField(field);
  return {
    type,
    enumValues: type === "enum" ? (values.length ? values : ["value"]) : [],
  };
}

export function schemaLikeFieldRequiredPatch(rawValue) {
  return { required: !!rawValue };
}

export function schemaLikeFieldDescriptionPatch(rawValue) {
  return { description: String(rawValue ?? "") };
}

export function normalizeSchemaLikeFieldPatch(current, patch = {}, contract) {
  const source = current && typeof current === "object" ? current : {};
  const rawPatch = patch && typeof patch === "object" ? patch : {};
  let nextPatch = { ...rawPatch };
  if ("type" in nextPatch) {
    const typePatch = schemaLikeFieldTypePatch(source, nextPatch.type, contract);
    delete nextPatch.type;
    delete nextPatch.enumValues;
    nextPatch = { ...nextPatch, ...typePatch };
  }
  if ("enumValues" in nextPatch) {
    const type = String(nextPatch.type || source.type || "").trim();
    nextPatch.enumValues = type === "enum"
      ? enumValuesForField(nextPatch).map((value) => String(value || "").trim()).filter(Boolean)
      : [];
  }
  return nextPatch;
}

export function schemaLikeFieldTypeControlState(field, contract) {
  const defaultType = contractDefaultValue(contract, "schema_field_type");
  const type = String(field?.type || defaultType || "").trim();
  const typeOptions = schemaFieldTypeOptions(contract, type);
  return {
    type,
    typeOptions,
    selectedType: typeOptions.find((option) => option.value === type) || null,
  };
}

export function schemaFieldRowControlState(field, contract, schemaView = null, overrides = {}) {
  const view = schemaViewForState(schemaView);
  const typeState = schemaLikeFieldTypeControlState(field, contract);
  return {
    namePlaceholder: overrides.namePlaceholder || view.fieldNamePlaceholder,
    descriptionPlaceholder: overrides.descriptionPlaceholder || view.fieldDescriptionPlaceholder,
    removeTitle: overrides.removeTitle || view.fieldRemoveTitle,
    enumLabel: overrides.enumLabel || view.fieldEnumLabel,
    enumAddLabel: overrides.enumAddLabel || view.fieldEnumAddLabel,
    enumAddValue: overrides.enumAddValue || view.fieldEnumAddValue,
    enumValues: enumValuesForField(field),
    typeState,
  };
}

export function inputParamFieldControlState(param, contract, basicView = null) {
  const view = basicEditorViewState(basicView);
  return schemaFieldRowControlState(param, contract, null, {
    namePlaceholder: view.inputParamNamePlaceholder,
    descriptionPlaceholder: view.inputParamDescriptionPlaceholder,
    removeTitle: view.inputParamRemoveTitle,
    enumLabel: view.inputParamEnumLabel,
    enumAddLabel: view.inputParamEnumAddLabel,
    enumAddValue: view.inputParamEnumAddValue,
  });
}

export function enumValueDraftPatch(field, index, rawValue) {
  const values = enumValuesForField(field);
  const i = Number(index);
  if (!Number.isInteger(i) || i < 0 || i >= values.length) return { enumValues: values };
  const next = [...values];
  next[i] = String(rawValue ?? "");
  return { enumValues: next };
}

export function enumValueCommitPatch(field, index, rawValue) {
  const values = enumValuesForField(field);
  const i = Number(index);
  if (!Number.isInteger(i) || i < 0 || i >= values.length) return { enumValues: values };
  const next = [...values];
  next[i] = uniqueEnumValue(values, rawValue, i);
  return { enumValues: next };
}

export function enumValueDeletePatch(field, index) {
  const values = enumValuesForField(field);
  const i = Number(index);
  if (!Number.isInteger(i) || i < 0 || i >= values.length) return { enumValues: values };
  return { enumValues: values.filter((_, j) => j !== i) };
}

export function enumValueAddPatch(field, rawValue = "value") {
  const values = enumValuesForField(field);
  return { enumValues: [...values, uniqueEnumValue(values, rawValue)] };
}

export function schemaFieldUpdatePatch(schema, fieldId, patch = {}, contract) {
  const fields = Array.isArray(schema?.fields) ? schema.fields : [];
  const current = fields.find((field) => field?.id === fieldId) || null;
  if (!current) return { fields };
  const normalized = normalizeSchemaLikeFieldPatch(current, patch, contract);
  if (Object.prototype.hasOwnProperty.call(normalized, "name")) {
    normalized.name = uniqueSchemaFieldName(fields, normalized.name, fieldId, editorSchemaFieldNameFallback(contract));
  }
  return { fields: fields.map((field) => field?.id === fieldId ? { ...field, ...normalized } : field) };
}

export function schemaFieldUpdateCascadePatch({ schema, schemas, flow, edges, members, instances } = {}, fieldId, patch = {}, contract) {
  const currentSchemaId = String(schema?.id || "").trim();
  const updatePatch = schemaFieldUpdatePatch(schema, fieldId, patch, contract);
  const nextSchema = { ...(schema || {}), ...updatePatch };
  const list = Array.isArray(schemas) ? schemas : [];
  const nextSchemas = currentSchemaId
    ? list.map((candidate) => candidate?.id === currentSchemaId ? nextSchema : candidate)
    : list;
  const reconciled = reconcileConditionFieldAvailability({
    flow,
    edges,
    members,
    instances,
    schemas: nextSchemas,
  });
  return {
    patch: updatePatch,
    schema: nextSchema,
    schemas: nextSchemas,
    flow: reconciled.flow,
    edges: reconciled.edges,
  };
}

export function schemaFieldRenameCascadePatch({ schema, schemas, flow, edges, members, instances } = {}, fieldId, rawName, oldName, contract) {
  const currentSchemaId = String(schema?.id || "").trim();
  const updatePatch = schemaFieldUpdatePatch(schema, fieldId, { name: rawName }, contract);
  const nextSchema = { ...(schema || {}), ...updatePatch };
  const list = Array.isArray(schemas) ? schemas : [];
  const nextSchemas = currentSchemaId
    ? list.map((candidate) => candidate?.id === currentSchemaId ? nextSchema : candidate)
    : list;
  const nextField = (nextSchema.fields || []).find((field) => field?.id === fieldId) || null;
  const previousName = String(oldName || "").trim();
  const nextName = String(nextField?.name || "").trim();
  const reconciled = previousName && previousName !== nextName
    ? reconcileSchemaFieldReferences({
      flow,
      edges,
      members,
      instances,
      schemaId: currentSchemaId,
      oldName: previousName,
      newName: nextName,
    })
    : { flow, edges };
  return {
    patch: updatePatch,
    schema: nextSchema,
    schemas: nextSchemas,
    flow: reconciled.flow,
    edges: reconciled.edges,
  };
}

export function schemaFieldDeletePatch(schema, fieldId) {
  const fields = Array.isArray(schema?.fields) ? schema.fields : [];
  const removed = fields.find((field) => field?.id === fieldId) || null;
  return { removed, patch: { fields: fields.filter((field) => field?.id !== fieldId) } };
}

export function schemaFieldDeleteCascadePatch({ schema, schemas, flow, edges, members, instances } = {}, fieldId) {
  const deleteResult = schemaFieldDeletePatch(schema, fieldId);
  const currentSchemaId = String(schema?.id || "").trim();
  if (!currentSchemaId) {
    return {
      removed: deleteResult.removed,
      patch: deleteResult.patch,
      schemas: Array.isArray(schemas) ? schemas : [],
      flow,
      edges,
    };
  }
  const list = Array.isArray(schemas) ? schemas : [];
  const nextSchema = { ...(schema || {}), ...deleteResult.patch };
  const nextSchemas = list.map((candidate) => candidate?.id === currentSchemaId ? nextSchema : candidate);
  const removedName = String(deleteResult.removed?.name || "").trim();
  const reconciled = removedName
    ? reconcileSchemaFieldReferences({
      flow,
      edges,
      members,
      instances,
      schemaId: currentSchemaId,
      oldName: removedName,
      newName: "",
    })
    : { flow, edges };
  return {
    removed: deleteResult.removed,
    patch: deleteResult.patch,
    schema: nextSchema,
    schemas: nextSchemas,
    flow: reconciled.flow,
    edges: reconciled.edges,
  };
}
