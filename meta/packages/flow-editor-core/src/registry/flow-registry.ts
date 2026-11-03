// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the flow-registry functions move byte-verbatim as plain JS, and
// their `options = {}` / destructured `= {}` parameter defaults
// (flowCatalogBootstrapState, flowRegistryViewState, newFlowTemplateOptions,
// flowRegistryPersistOutcomeProjection, and friends) raise TS2339 under .ts
// semantics. Source-contract pins this exact text, so suppression must live
// at file level, not in the moved bodies. Resolution/linkage stays guarded
// behaviorally: the projection suite and export-keys test load the bundle
// and exercise these functions, so a missed import or re-export still fails
// the gate as a ReferenceError.
//
// Flow-registry plane for the Flow Editor controller: sample/blank mobpack
// catalog hydration, the catalog bootstrap projection, registry row
// normalization and view/selection state, draft guards, document/outcome
// persistence projections, row patches, draft-id allocation, the newFlow*
// modal family, and agent definition normalizers. Moved verbatim from the
// controller.js flow-registry range. normalizeAgentDefinitionRows was
// seeded here in S12 (needed by editors/agent-editor.ts ahead of this
// slice) and keeps its original cluster-tail position.
import { slug } from "../domain/tool-skill-access";
import {
  normalizeMaxInlinePeerNotifications,
  normalizeProfileBackend,
  normalizeProviderParams,
} from "../shared/normalize";
import {
  deployViewForState,
  flowRegistryViewForState,
  newFlowViewForState,
} from "../views/view-config";

export function sampleFlowsFromCatalogs(schema) {
  return (schema?.sample_mobpacks || [])
    .filter((sample) => sample && typeof sample === "object" && sample.document)
    .map((sample) => {
      const source = typeof sample.source === "string" ? sample.source.trim() : "";
      if (!source) return null;
      const id = String(sample.id || "").trim();
      const name = String(sample.name || "").trim();
      const stage = String(sample.stage || "").trim();
      if (!id || !name || !stage) return null;
      return {
        id,
        name,
        version: String(sample.version || sample.document?.schema_version || ""),
        stage,
        trigger: String(sample.trigger || source),
        source,
        document: sample.document,
        validation: sample.validation || null,
        ...(sample.deployability ? { deployability: sample.deployability } : {}),
        ...(sample.provenance ? { provenance: sample.provenance } : {}),
      };
    })
    .filter(Boolean);
}

// The library is home: bootstrap only projects the catalog/template and
// registry rows. Nothing hydrates at startup — opening or creating a mob
// is the only way into the editor sections.
export function flowCatalogBootstrapState(catalogPayload, options = {}) {
  const sampleFlows = sampleFlowsFromCatalogs(catalogPayload);
  const registryFlows = flowRegistryRowsFromBackend(options.registryRows || options.registryResult?.rows);
  const runtimeFlows = flowRegistryRowsFromBackend(catalogPayload?.runtime_flows);
  const existingIds = new Set(runtimeFlows.map((row) => row.id));
  const flows = [
    ...runtimeFlows,
    ...registryFlows.filter((row) => !existingIds.has(row.id)),
  ];
  return {
    templates: sampleFlows,
    flows,
  };
}

export function flowRegistryRowsFromBackend(rows = []) {
  return (Array.isArray(rows) ? rows : [])
    .map((row) => {
      if (!row || typeof row !== "object") return null;
      if (!row.document) return flowRegistryRowFromSummary(row);
      return flowRegistryRowFromDocument({
        id: row.id,
        document: row.document,
        validation: row.validation ?? null,
        stage: row.stage,
        trigger: row.trigger,
        source: row.source,
        flowRow: row,
      });
    })
    .filter(Boolean);
}

// Draft replies list drafts as summaries, without their documents: the
// library row keeps what the summary carries (revision and etag included),
// and opening the draft fetches its document with meta/mobpacks/get.
function flowRegistryRowFromSummary(row) {
  const id = String(row?.id || "").trim();
  if (!id) return null;
  return {
    ...row,
    id,
    name: String(row.name || id),
    version: String(row.version || ""),
    stage: String(row.stage || "draft"),
    trigger: String(row.trigger || ""),
    source: String(row.source || ""),
    validation: null,
  };
}

export function blankMobpackFromCatalogs(schema) {
  const blank = schema?.blank_mobpack;
  if (!blank || typeof blank !== "object" || !blank.document) return null;
  const source = typeof blank.source === "string" ? blank.source.trim() : "";
  const id = String(blank.id || "").trim();
  const name = String(blank.name || "").trim();
  const stage = String(blank.stage || "").trim();
  if (!id || !name || !source || !stage) return null;
  return {
    id,
    name,
    version: String(blank.version || blank.document?.schema_version || ""),
    stage,
    trigger: String(blank.trigger || source),
    source,
    document: blank.document,
    validation: blank.validation || null,
    ...(blank.deployability ? { deployability: blank.deployability } : {}),
    ...(blank.provenance ? { provenance: blank.provenance } : {}),
  };
}

export function graphTemplateSeedFromBlankMobpack(blankMobpack) {
  if (!blankMobpack || typeof blankMobpack !== "object") return null;
  const name = String(blankMobpack.name || "").trim();
  const repo = String(blankMobpack.source || "").trim();
  const version = String(blankMobpack.version || "").trim();
  const trigger = String(blankMobpack.trigger || "").trim();
  if (!name || !repo || !version) return null;
  return {
    name,
    repo,
    version,
    triggers: {
      labels: trigger ? [trigger] : [],
      default: false,
    },
  };
}

export function flowRegistryMarkDraftPatch(rows, currentFlowId) {
  const list = Array.isArray(rows) ? rows : [];
  if (!currentFlowId) return list;
  let changed = false;
  const next = list.map((row) => {
    if (!row || row.id !== currentFlowId) return row;
    if (row.stage === "draft" && row.validation == null) return row;
    changed = true;
    return { ...row, stage: "draft", validation: null };
  });
  return changed ? next : list;
}

// Relative "updated" label for library rows. Pure: the caller supplies the
// clock (options.nowUnixMs) so projections stay deterministic in tests.
export function flowRegistryUpdatedLabel(updatedAtUnixMs, nowUnixMs, justNowLabel = "") {
  const updated = Number(updatedAtUnixMs || 0);
  const now = Number(nowUnixMs || 0);
  if (!updated || !now || now < updated) return "";
  const seconds = Math.floor((now - updated) / 1000);
  if (seconds < 60) return justNowLabel;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function flowRegistryRowDescription(row) {
  const flowDescription = row?.document?.flow?.description;
  if (typeof flowDescription === "string" && flowDescription.trim()) return flowDescription.trim();
  const rowDescription = row?.description;
  if (typeof rowDescription === "string" && rowDescription.trim()) return rowDescription.trim();
  // Projected documents carry their human description as the input step's
  // task text rather than a flow.description key.
  const steps = row?.document?.flow?.steps;
  if (Array.isArray(steps)) {
    const input = steps.find((step) => step?.type === "input");
    const task = input?.task;
    if (typeof task === "string" && task.trim()) return task.trim();
  }
  return "";
}

export function flowRegistryViewState(rows, currentFlowId, options = {}) {
  const list = Array.isArray(rows) ? rows : [];
  const view = flowRegistryViewForState(options.flowRegistryView);
  const suffix = list.length === 1 ? view.titleSingularSuffix : view.titlePluralSuffix;
  const nowUnixMs = Number(options.nowUnixMs || 0);
  const projectRow = (row) => {
    const id = String(row?.id || "");
    const stage = String(row?.stage || "draft");
    return {
      id,
      className: "flows-list__row" + (id && id === currentFlowId ? " is-current" : ""),
      name: String(row?.name || ""),
      description: flowRegistryRowDescription(row),
      updated: flowRegistryUpdatedLabel(row?.updated_at_unix_ms, nowUnixMs, view.updatedJustNowLabel),
      stage,
    };
  };
  const draftRows = list.filter((row) => !row?.runtime_projection);
  const runtimeRows = list.filter((row) => !!row?.runtime_projection);
  const sections = [];
  // An empty library is communicated by the empty state alone; section
  // chrome only appears once there is at least one row anywhere.
  if (draftRows.length || (list.length && !runtimeRows.length)) {
    sections.push({
      key: "drafts",
      label: view.draftsSectionLabel,
      hint: "",
      rows: draftRows.map(projectRow),
    });
  }
  if (runtimeRows.length) {
    sections.push({
      key: "runtime",
      label: view.runtimeSectionLabel,
      hint: view.runtimeReadonlyHint,
      rows: runtimeRows.map(projectRow),
    });
  }
  return {
    eyebrow: view.eyebrow,
    title: `${list.length} ${suffix}`.trim(),
    createLabel: view.createLabel,
    createDisabled: !options.canCreate,
    createTitle: options.canCreate ? view.createReadyTitle : view.createUnavailableTitle,
    columns: view.columns,
    empty: list.length === 0 ? { title: view.emptyTitle, text: view.emptyText } : null,
    sections,
  };
}

// Quick-switch dropdown on the breadcrumb mob name: draft registry rows
// (runtime projections are read-only and stay library-only) plus the
// schema-labelled "view all" affordance that returns to the library.
export function mobSwitcherState(rows, currentFlowId, options = {}) {
  const view = deployViewForState(options.deployView);
  const current = String(currentFlowId || "");
  return {
    rows: (Array.isArray(rows) ? rows : [])
      .filter((row) => row && row.id && !flowRegistryRowIsRuntimeProjection(row))
      .map((row) => {
        const id = String(row.id || "");
        return {
          id,
          name: String(row.name || ""),
          stage: String(row.stage || ""),
          className: "crumb-switcher__item" + (id && id === current ? " is-current" : ""),
        };
      }),
    viewAllLabel: view.switcherViewAllLabel,
  };
}

export function flowRegistrySelectionState(rows, id) {
  const selectedId = String(id || "");
  const row = (Array.isArray(rows) ? rows : []).find((candidate) => candidate?.id === selectedId) || null;
  if (!row) {
    return {
      found: false,
      id: selectedId,
      row: null,
      hasDocument: false,
      hydration: null,
      fallback: null,
    };
  }
  if (row.document && typeof row.document === "object") {
    return {
    found: true,
    id: selectedId,
    row,
    hasDocument: true,
    hydration: {
      result: {
        document: row.document,
        validation: row.validation ?? null,
      },
      options: {
        id: selectedId,
        flowRow: row,
        addToRegistry: false,
      },
    },
    fallback: null,
  };
  }
  // A draft listed as a summary opens with its document from
  // meta/mobpacks/get. A runtime projection always carries its document.
  if (!flowRegistryRowIsRuntimeProjection(row)) {
    return {
      found: true,
      id: selectedId,
      row,
      hasDocument: false,
      hydration: null,
      fallback: null,
      fetch: { id: selectedId },
    };
  }
  return {
    found: true,
    id: selectedId,
    row,
    hasDocument: false,
    hydration: null,
    fallback: null,
    error: "missing_registry_document",
  };
}

export function flowRegistryRowFromDocument({
  id,
  document,
  validation = null,
  stage,
  trigger = "",
  source = "",
  sourceLabel = "",
  flowRow = null,
  fallbackName = "",
  fallbackVersion = "",
} = {}) {
  const rowId = String(id || "").trim();
  if (!rowId || !document || typeof document !== "object") return null;
  const name = String(
    flowRow?.name ||
    document.name ||
    document.flow?.name ||
    document.mob_id ||
    fallbackName
  );
  const version = String(flowRow?.version || document.schema_version || fallbackVersion);
  return {
    id: rowId,
    name,
    version,
    stage: String(stage || (validation?.ok ? "valid" : "draft")),
    trigger: String(flowRow?.trigger || trigger || sourceLabel || ""),
    source: String(flowRow?.source || source || ""),
    document,
    validation: validation ?? null,
    // Carry the optimistic-concurrency fields through. Without them, rows
    // hydrated from the backend list (bootstrap) lose their revision/etag,
    // so `flowRegistryDraftGuard` returns `{}` (no `expected_revision`) and
    // every save/apply_operation/undo/deploy runs UNGUARDED until the first
    // save response replaces the row — two sessions opening the same draft
    // would last-write-win over each other for the whole first-edit span.
    ...(flowRegistryRowRevision(flowRow) !== null
      ? { revision: flowRegistryRowRevision(flowRow) }
      : {}),
    ...(Number(flowRow?.draft_revision) >= 0 && flowRow?.draft_revision != null
      ? { draft_revision: Number(flowRow.draft_revision) }
      : {}),
    ...(flowRow?.draft_etag ? { draft_etag: String(flowRow.draft_etag) } : {}),
    ...(flowRow?.etag ? { etag: String(flowRow.etag) } : {}),
    ...(Number(flowRow?.updated_at_unix_ms) > 0
      ? { updated_at_unix_ms: Number(flowRow.updated_at_unix_ms) }
      : {}),
    ...(flowRow?.registry_source ? { registry_source: String(flowRow.registry_source) } : {}),
    ...(flowRow?.document_kind ? { document_kind: String(flowRow.document_kind) } : {}),
    ...(flowRow?.runtime_projection === true ? { runtime_projection: true } : {}),
    ...(flowRow?.runtime_mob_id ? { runtime_mob_id: String(flowRow.runtime_mob_id) } : {}),
    ...(flowRow?.runtime_flow_id ? { runtime_flow_id: String(flowRow.runtime_flow_id) } : {}),
    ...(flowRow?.deployability ? { deployability: flowRow.deployability } : {}),
    ...(flowRow?.provenance ? { provenance: flowRow.provenance } : {}),
  };
}

export function flowRegistryRowIsRuntimeProjection(row) {
  return row?.runtime_projection === true
    || row?.document_kind === "runtime_projection"
    || row?.source === "meta/runtime/flow_projection"
    || row?.registry_source === "meta/runtime/flow_projection";
}

export function flowRegistryRememberDocumentPatch(rows, {
  currentFlowId,
  document,
  validation = null,
  stage = "draft",
} = {}) {
  const list = Array.isArray(rows) ? rows : [];
  if (!currentFlowId || !document || typeof document !== "object") return list;
  let changed = false;
  const next = list.map((row) => {
    if (!row || row.id !== currentFlowId) return row;
    changed = true;
    return {
      ...row,
      name: document.name || document.flow?.name || row.name,
      version: document.schema_version || row.version,
      stage,
      document,
      validation: validation ?? null,
    };
  });
  return changed ? next : list;
}

export function flowRegistryRowRevision(row) {
  const value = row?.revision ?? row?.draft_revision;
  const revision = Number(value);
  return Number.isFinite(revision) && revision >= 0 ? revision : null;
}

/// Refresh ONLY the optimistic-concurrency guard fields (revision/etag) of the
/// matching local row from an authoritative server row, preserving the local
/// document (the in-progress draft edits). Used by the save-conflict handler:
/// a concurrent writer bumped the server revision, so the conflicting local
/// row must adopt the server's revision before the draft is re-persisted —
/// otherwise the re-save carries the same stale `expected_revision` and
/// conflicts forever (unbounded autosave retry loop).
export function flowRegistryRefreshGuardPatch(rows, currentFlowId, serverRow) {
  const list = Array.isArray(rows) ? rows : [];
  const id = String(currentFlowId || serverRow?.id || "").trim();
  if (!id || !serverRow || typeof serverRow !== "object") return list;
  const revision = flowRegistryRowRevision(serverRow);
  const etag = flowRegistryRowEtag(serverRow);
  if (revision === null && !etag) return list;
  let changed = false;
  const next = list.map((row) => {
    if (!row || row.id !== id) return row;
    changed = true;
    return {
      ...row,
      ...(revision !== null ? { revision, draft_revision: revision } : {}),
      ...(etag ? { draft_etag: etag, etag } : {}),
    };
  });
  return changed ? next : list;
}

export function flowRegistryRowEtag(row) {
  const value = row?.draft_etag ?? row?.etag;
  return value ? String(value) : "";
}

export function flowRegistryDraftGuard(row, currentFlowId = "") {
  const id = String(row?.id || currentFlowId || "").trim();
  const expectedRevision = flowRegistryRowRevision(row);
  const expectedEtag = flowRegistryRowEtag(row);
  if (!id || expectedRevision === null) return {};
  return {
    id,
    expected_revision: expectedRevision,
    ...(expectedEtag ? { expected_etag: expectedEtag } : {}),
  };
}

export function flowRegistryDocumentPersistence({
  currentFlowId,
  document,
  validation = null,
  stage = "draft",
  previousSignature = "",
  skipIfUnchanged = false,
  expectedRevision = null,
  expectedEtag = "",
} = {}) {
  if (!currentFlowId || !document || typeof document !== "object") {
    return {
      ok: false,
      changed: false,
      signature: String(previousSignature || ""),
      rowPatch: null,
    };
  }
  const signature = `${currentFlowId}\n${JSON.stringify(document)}`;
  if (skipIfUnchanged && signature === previousSignature) {
    return {
      ok: true,
      changed: false,
      signature,
      rowPatch: null,
    };
  }
  const nextStage = validation == null && stage === "published" ? "draft" : stage;
  return {
    ok: true,
    changed: true,
    signature,
    rowPatch: {
      currentFlowId,
      document,
      validation,
      stage: nextStage,
      ...(expectedRevision !== null && expectedRevision !== undefined ? { expectedRevision } : {}),
      ...(expectedEtag ? { expectedEtag } : {}),
    },
  };
}

export function flowRegistryPersistDocumentProjection(rows, options = {}) {
  const sourceRows = Array.isArray(rows) ? rows : [];
  const currentRow = sourceRows.find((row) => row?.id === options.currentFlowId) || null;
  if (flowRegistryRowIsRuntimeProjection(currentRow)) {
    return {
      ok: false,
      changed: false,
      reason: "runtime_projection_read_only",
      signature: String(options.previousSignature || ""),
      rowPatch: null,
      rows: sourceRows,
    };
  }
  const persistence = flowRegistryDocumentPersistence({
    expectedRevision: flowRegistryRowRevision(currentRow),
    expectedEtag: flowRegistryRowEtag(currentRow),
    ...options,
  });
  if (!persistence.ok || !persistence.rowPatch) {
    return {
      ...persistence,
      rows: sourceRows,
    };
  }
  return {
    ...persistence,
    rows: flowRegistryRememberDocumentPatch(sourceRows, persistence.rowPatch),
  };
}

export function flowRegistryPersistOutcomeProjection(rows, { currentFlowId, outcome, previousSignature = "", skipIfUnchanged = false } = {}) {
  const sourceOutcome = outcome && typeof outcome === "object" ? outcome : {};
  const persistence = flowRegistryPersistDocumentProjection(rows, {
    currentFlowId,
    document: sourceOutcome.document,
    validation: sourceOutcome.validation,
    stage: sourceOutcome.stage,
    previousSignature,
    skipIfUnchanged,
  });
  return {
    ...sourceOutcome,
    persistence,
    rows: persistence.rows,
    signature: persistence.signature,
    changed: persistence.changed,
    ok: persistence.ok,
  };
}

export function flowRegistryAppendRowPatch(rows, row) {
  const list = Array.isArray(rows) ? rows : [];
  if (!row || typeof row !== "object" || !row.id) return list;
  return [...list, row];
}

export function flowRegistryUpsertRowPatch(rows, row) {
  const list = Array.isArray(rows) ? rows : [];
  if (!row || typeof row !== "object" || !row.id) return list;
  let replaced = false;
  const next = list.map((candidate) => {
    if (!candidate || candidate.id !== row.id) return candidate;
    replaced = true;
    return row;
  });
  return replaced ? next : [...list, row];
}

export function flowDraftIdFromSpec(spec, existingRows = []) {
  const draftSpec = spec && typeof spec === "object" ? spec : {};
  const base = slug(draftSpec.name || draftSpec.template || "meta_flow", "meta_flow");
  const prefix = base.startsWith("f_") ? base : `f_${base}`;
  const used = new Set((Array.isArray(existingRows) ? existingRows : [])
    .map((row) => String(row?.id || "").trim())
    .filter(Boolean));
  if (!used.has(prefix)) return prefix;
  let index = 2;
  while (used.has(`${prefix}_${index}`)) index += 1;
  return `${prefix}_${index}`;
}

function templateOptionDescription(sample) {
  const flowDescription = sample?.document?.flow?.description;
  if (typeof flowDescription === "string" && flowDescription.trim()) return flowDescription.trim();
  return String(sample?.source || "");
}

export function newFlowTemplateOptions(templates = [], { canCreateBlank = false, blankTemplate = null } = {}) {
  const hasBlankDocument = !!blankTemplate?.document;
  const options = [{
    id: "blank",
    label: hasBlankDocument ? String(blankTemplate.name || "") : "Blank",
    sub: hasBlankDocument
      ? templateOptionDescription(blankTemplate)
      : "Waiting for Meta blank mobpack",
    tier: hasBlankDocument ? String(blankTemplate.stage || "") : "",
    disabled: !canCreateBlank || !hasBlankDocument,
  }];
  for (const sample of Array.isArray(templates) ? templates : []) {
    if (!sample || typeof sample !== "object") continue;
    const id = String(sample.id || "").trim();
    const label = String(sample.name || "").trim();
    if (!id || !label) continue;
    options.push({
      id,
      label,
      sub: templateOptionDescription(sample),
      tier: String(sample.stage || ""),
      disabled: false,
    });
  }
  return options;
}

// options.template is the optional deep-link preselect
// (?intent=new&template=<id>): known ids — the blank template or any row in
// options.templates — win; unknown ids fall back to the blank template.
export function newFlowInitialState({ blankTemplate = null, template = "", templates = [] } = {}) {
  const hasBlankDocument = !!blankTemplate?.document;
  const fallback = hasBlankDocument ? String(blankTemplate.id || "") : "";
  const requested = String(template || "").trim();
  const known = !!requested && (
    requested === fallback
    || (Array.isArray(templates) ? templates : []).some((row) => String(row?.id || "") === requested)
  );
  return {
    name: "",
    template: known ? requested : fallback,
  };
}

export function newFlowModalState(state = {}, templateOptions = [], newFlowView = null) {
  const view = newFlowViewForState(newFlowView);
  const name = String(state.name || "");
  const template = String(state.template || "");
  const options = (Array.isArray(templateOptions) ? templateOptions : []).map((option) => {
    const id = String(option?.id || "");
    return {
      id,
      label: String(option?.label || ""),
      sub: String(option?.sub || ""),
      tier: String(option?.tier || ""),
      disabled: !!option?.disabled,
      className: "template-card" + (id && id === template ? " is-selected" : ""),
    };
  });
  const selectedTemplate = options.find((option) => option.id === template) || null;
  return {
    eyebrow: view.eyebrowTemplate,
    closeLabel: view.closeLabel,
    nameLabel: view.nameLabel,
    namePlaceholder: view.namePlaceholder,
    startFromLabel: view.startFromLabel,
    createLabel: view.createLabel,
    name,
    template,
    options,
    createDisabled: !name.trim() || !selectedTemplate || !!selectedTemplate.disabled,
  };
}

export function newFlowModalPatch(state = {}, patch = {}) {
  const source = state && typeof state === "object" ? state : {};
  const rawPatch = patch && typeof patch === "object" ? patch : {};
  const next = { ...source, ...rawPatch };
  next.name = String(next.name || "");
  next.template = String(next.template || "");
  return next;
}

export function newFlowModalFieldPatch(state = {}, field, value) {
  const key = String(field || "").trim();
  if (!key) return newFlowModalPatch(state);
  if (!["name", "template"].includes(key)) return newFlowModalPatch(state);
  return newFlowModalPatch(state, { [key]: value });
}

export function newFlowModalCreateSpec(state = {}) {
  const source = newFlowModalPatch(state);
  return {
    name: source.name,
    template: source.template,
  };
}

export function agentDefinitionsFromCatalogs(schema) {
  const definitions = Array.isArray(schema?.agent_definitions) ? schema.agent_definitions : [];
  return normalizeAgentDefinitionsFromCatalog(definitions);
}

export function sampleAgentDefinitionsFromCatalogs(schema) {
  const definitions = Array.isArray(schema?.sample_agent_definitions) ? schema.sample_agent_definitions : [];
  return normalizeAgentDefinitionsFromCatalog(definitions);
}

export function normalizeAgentDefinitionsFromCatalog(definitions) {
  return definitions
    .filter((template) => template && typeof template === "object")
    .filter((template) => String(template.definitionType || template.definition_type || "") === "meta/profile-member")
    .filter((template) => String(template.source || "").trim())
    .filter((template) => String(template.sourceMobpack || template.source_mobpack || "").trim())
    .filter((template) => String(template.sourceOrigin || template.source_origin || "").trim())
    .filter((template) => String(template.profileBinding || template.profile_binding || "").trim())
    .filter((template) => String(template.runtimeMode || template.runtime_mode || "").trim())
    .filter((template) => String(template.model || "").trim())
    .map((template) => {
      const id = String(template.id || "").trim();
      const role = String(template.role || "").trim();
      const name = String(template.name || template.label || "").trim();
      const model = String(template.model || "").trim();
      const definitionKind = String(template.definitionKind || template.definition_kind || "").trim();
      const sourceKind = String(template.sourceKind || template.source_kind || "").trim();
      if (!id || !role || !name) return null;
      return {
        id,
        role,
        label: String(template.label || name),
        name,
        model,
        schema: String(template.schema || ""),
        schemaDefinition: normalizeAgentSchemaDefinition(template.schemaDefinition || template.schema_definition),
        schemaSourceDocumentPath: String(template.schemaSourceDocumentPath || template.schema_source_document_path || ""),
        skills: Array.isArray(template.skills) ? [...template.skills] : [],
        skillDefinitions: normalizeAgentDefinitionRows(template.skillDefinitions || template.skill_definitions),
        tools: Array.isArray(template.tools) ? [...template.tools] : [],
        toolDefinitions: normalizeAgentDefinitionRows(template.toolDefinitions || template.tool_definitions),
        profileBinding: String(template.profileBinding || template.profile_binding || ""),
        realmProfile: String(template.realmProfile || template.realm_profile || ""),
        runtimeMode: String(template.runtimeMode || template.runtime_mode || ""),
        externalAddressable: !!template.externalAddressable,
        backend: normalizeProfileBackend(template.backend),
        maxInlinePeerNotifications: normalizeMaxInlinePeerNotifications(template.maxInlinePeerNotifications ?? template.max_inline_peer_notifications),
        systemPrompt: String(template.systemPrompt || template.system_prompt || ""),
        providerParams: normalizeProviderParams(template.providerParams || template.provider_params),
        definitionType: String(template.definitionType || template.definition_type),
        ...(definitionKind ? { definitionKind } : {}),
        ...(sourceKind ? { sourceKind } : {}),
        source: template.source || "",
        sourceMobpack: template.sourceMobpack || template.source_mobpack || "",
        sourceMobpackName: template.sourceMobpackName || template.source_mobpack_name || "",
        sourceOrigin: template.sourceOrigin || template.source_origin || "",
        sourceDocumentPath: template.sourceDocumentPath || template.source_document_path || "",
        ...(template.deployability ? { deployability: template.deployability } : {}),
        ...(template.provenance ? { provenance: template.provenance } : {}),
      };
    })
    .filter(Boolean);
}

export function normalizeAgentSchemaDefinition(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const id = String(value.id || "").trim();
  const fields = Array.isArray(value.fields) ? value.fields : [];
  if (!id || !fields.length) return null;
  return JSON.parse(JSON.stringify(value));
}

export function normalizeAgentDefinitionRows(value) {
  if (!Array.isArray(value)) return [];
  return value
    .filter((row) => row && typeof row === "object" && !Array.isArray(row))
    .map((row) => JSON.parse(JSON.stringify(row)));
}
