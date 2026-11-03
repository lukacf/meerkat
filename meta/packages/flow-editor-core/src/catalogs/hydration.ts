// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the catalogs-hydration functions move byte-verbatim as plain
// JS, and their `options = {}` / `boot = {}` / `result = {}` parameter
// defaults (emptyMetaCatalogs, metaCatalogsFromSchema,
// hydrateMobpackDocumentState, authoringProjectionFromMetaDocument, and
// friends) raise TS2339 under .ts semantics. Source-contract pins this
// exact text, so suppression must live at file level, not in the moved
// bodies. Resolution/linkage stays guarded behaviorally: the projection
// suite and export-keys test load the bundle and exercise these functions,
// so a missed import or re-export still fails the gate as a ReferenceError.
//
// Catalog hydration plane for the Flow Editor controller: Meta catalog
// projections (models, tool catalog, skill realms, deploy defaults, the
// per-surface *ViewFromSchema aggregation in metaCatalogsFromSchema),
// mobpack document hydration into editor state, authoring projections from
// Meta documents/operation results, and imported-flow id allocation.
// Moved verbatim from the controller.js catalogs-hydration range. This
// slice re-homes deploySettingsForUi from the residue, retiring the last
// _residue-bridge wrapper (flow/reconcile.ts now imports it relatively).
import { mobDefaultsFromSchema, mobSettingsForUi } from "../drafts/mob-settings";
import { emptyAuthoringFlowState } from "../flow/step-tree";
import {
  agentDefinitionsFromCatalogs,
  blankMobpackFromCatalogs,
  flowDraftIdFromSpec,
  flowRegistryRowFromDocument,
  flowRegistryRowsFromBackend,
  graphTemplateSeedFromBlankMobpack,
  sampleAgentDefinitionsFromCatalogs,
} from "../registry/flow-registry";
import { authoringOperationsFromSchema } from "../rpc/client";
import {
  conditionViewFromSchema,
  errorViewForState,
  errorViewFromSchema,
} from "../schema/field-edit";
import { EMPTY_DEPLOY_SETTINGS } from "../shared/constants";
import { diagnosticsToRows } from "../shell/outcomes";
import { sourceViewFromSchema } from "../source/view";
import {
  agentAccessViewFromSchema,
  agentDetailViewFromSchema,
  agentViewFromSchema,
  basicViewFromSchema,
  deployViewFromSchema,
  flowRegistryViewFromSchema,
  graphTemplateViewFromSchema,
  graphViewFromSchema,
  launchViewFromSchema,
  newFlowViewFromSchema,
  schemaViewFromSchema,
  settingsViewFromSchema,
} from "../views/view-config";

export function deploySettingsForUi(deploy) {
  if (!deploy || typeof deploy !== "object") return { ...EMPTY_DEPLOY_SETTINGS };
  return {
    ...EMPTY_DEPLOY_SETTINGS,
    command: deploy.command || "",
    surface: deploy.surface || "",
    trustPolicy: deploy.trust_policy || deploy.trustPolicy || "",
    model: deploy.model || "",
    maxDuration: deploy.max_duration || deploy.maxDuration || "",
    maxToolCalls: deploy.max_tool_calls ?? deploy.maxToolCalls ?? null,
    maxTotalTokens: deploy.max_total_tokens ?? deploy.maxTotalTokens ?? null,
    isolated: deploy.isolated ?? false,
    realm: deploy.realm || "",
    instance: deploy.instance || "",
    realmBackend: deploy.realm_backend || deploy.realmBackend || "",
    contextRoot: deploy.context_root || deploy.contextRoot || "",
    stateRoot: deploy.state_root || deploy.stateRoot || "",
    userConfigRoot: deploy.user_config_root || deploy.userConfigRoot || "",
    prompt: deploy.prompt || "",
  };
}

export function deployDefaultsFromSchema(schema) {
  return deploySettingsForUi(schema?.deploy_settings?.defaults);
}

export function modelCatalogFromCatalogs(schema) {
  return (schema?.models || [])
    .filter((model) => model && typeof model === "object" && model.id && model.label && (model.vendor || model.provider))
    .map((model) => ({
      id: String(model.id),
      label: String(model.label),
      vendor: String(model.vendor || model.provider),
      ...(model.deployability ? { deployability: model.deployability } : {}),
      ...(model.provenance ? { provenance: model.provenance } : {}),
      profile: model.profile || null,
    }));
}

export function toolCatalogFromCatalogs(schema) {
  return (Array.isArray(schema?.tool_catalog) ? schema.tool_catalog : [])
    .filter((tool) => tool && typeof tool === "object" && tool.id && tool.label && tool.desc && tool.kind && tool.source)
    .map((tool) => ({
      id: String(tool.id),
      label: String(tool.label),
      desc: String(tool.desc),
      kind: String(tool.kind),
      source: String(tool.source),
      tagClass: String(tool.tag_class || ""),
      raw: tool,
    }));
}

export function emptyMetaCatalogs(boot = {}) {
  return {
    models: [],
    toolCatalog: [],
    agentDefinitions: [],
    sampleAgentDefinitions: [],
    skillRealms: [],
    blankMobpack: null,
    catalogSnapshot: null,
    deployDefaults: deployDefaultsFromSchema(null),
    mobDefaults: mobDefaultsFromSchema(null),
    mobDefinition: null,
    sourceView: null,
    agentView: null,
    newFlowView: null,
    flowRegistryView: null,
    agentDetailView: null,
    agentAccessView: null,
    deployView: null,
    settingsView: null,
    launchView: null,
    schemaView: null,
    basicView: null,
    graphView: null,
    graphTemplateView: null,
    conditionView: null,
    errorView: null,
    authoringOperations: {},
    runtimeFlows: [],
    validationSource: "",
    contractMeta: {
      loaded: false,
      schemaVersion: "",
      mediaType: "",
      validationSource: "",
    },
    grid: boot.grid || null,
    cellXY: boot.cellXY || null,
    template: null,
  };
}

export function metaCatalogsFromSchema(schema, boot = {}, catalogPayload = null) {
  const catalogSource = catalogPayload && typeof catalogPayload === "object" ? catalogPayload : {};
  const agentDefinitions = agentDefinitionsFromCatalogs(catalogSource);
  const sampleAgentDefinitions = sampleAgentDefinitionsFromCatalogs(catalogSource);
  const blankMobpack = blankMobpackFromCatalogs(catalogSource);
  return {
    models: modelCatalogFromCatalogs(catalogSource),
    toolCatalog: toolCatalogFromCatalogs(catalogSource),
    agentDefinitions,
    sampleAgentDefinitions,
    runtimeFlows: flowRegistryRowsFromBackend(catalogSource.runtime_flows),
    skillRealms: skillRealmsFromCatalogs(catalogSource),
    blankMobpack,
    catalogSnapshot: catalogSource.catalog_snapshot || null,
    deployDefaults: deployDefaultsFromSchema(schema),
    mobDefaults: mobDefaultsFromSchema(schema),
    mobDefinition: schema?.mob_definition || null,
    sourceView: sourceViewFromSchema(schema),
    agentView: agentViewFromSchema(schema),
    newFlowView: newFlowViewFromSchema(schema),
    flowRegistryView: flowRegistryViewFromSchema(schema),
    agentDetailView: agentDetailViewFromSchema(schema),
    agentAccessView: agentAccessViewFromSchema(schema),
    deployView: deployViewFromSchema(schema),
    settingsView: settingsViewFromSchema(schema),
    launchView: launchViewFromSchema(schema),
    schemaView: schemaViewFromSchema(schema),
    basicView: basicViewFromSchema(schema),
    graphView: graphViewFromSchema(schema),
    graphTemplateView: graphTemplateViewFromSchema(schema),
    conditionView: conditionViewFromSchema(schema),
    errorView: errorViewFromSchema(schema),
    authoringOperations: authoringOperationsFromSchema(schema),
    validationSource: schema?.validation_source || "",
    contractMeta: {
      loaded: true,
      schemaVersion: schema?.schema_version || "",
      mediaType: schema?.media_type || "",
      validationSource: schema?.validation_source || "",
    },
    grid: boot.grid || null,
    cellXY: boot.cellXY || null,
    template: graphTemplateSeedFromBlankMobpack(blankMobpack),
  };
}

export function skillRealmsFromCatalogs(schema) {
  const skillRealms = schema?.skill_realms || [];
  return Array.isArray(skillRealms) ? skillRealms : [];
}

export function mergeSkillRealms(documentRealms, contractRealms) {
  const merged = [];
  const seenSkillIds = new Set();
  for (const realm of [...(documentRealms || []), ...(contractRealms || [])]) {
    if (!realm || typeof realm !== "object") continue;
    const id = String(realm.id || realm.label || "").trim();
    if (!id) continue;
    const uniqueSkills = [];
    for (const skill of realm.skills || []) {
      const skillId = String(skill?.id || "").trim();
      if (!skillId || seenSkillIds.has(skillId)) continue;
      seenSkillIds.add(skillId);
      uniqueSkills.push(skill);
    }
    const existing = merged.find((candidate) => candidate.id === id);
    if (existing) {
      existing.skills = [...(existing.skills || []), ...uniqueSkills];
      continue;
    }
    if (!uniqueSkills.length) continue;
    merged.push({
      ...realm,
      id,
      skills: uniqueSkills,
      default: merged.length === 0 ? !!realm.default : false,
    });
  }
  return merged;
}

export function catalogSkillRealmsPatch(catalogs, skillRealms) {
  return {
    ...(catalogs || {}),
    skillRealms: Array.isArray(skillRealms) ? skillRealms : [],
  };
}

export function flowFromHydratedDocument(document) {
  if (document?.flow && typeof document.flow === "object" && Array.isArray(document.flow.steps)) {
    return document.flow;
  }
  return null;
}

export function graphProjectionForDocument(document, members, contract) {
  const storedFrames = Array.isArray(document?.frames) ? document.frames : [];
  return {
    instances: Array.isArray(document?.instances) ? document.instances : [],
    edges: Array.isArray(document?.edges) ? document.edges : [],
    frames: storedFrames,
  };
}

export function graphProjectionFromMetaResult(result) {
  const source = result?.graph_projection || result?.graphProjection || result;
  if (!source || typeof source !== "object") return null;
  if (!Array.isArray(source.instances) || !Array.isArray(source.edges) || !Array.isArray(source.frames)) return null;
  return {
    instances: source.instances,
    edges: source.edges,
    frames: source.frames,
    source: String(source.source || ""),
    validation: source.validation || null,
  };
}

export function hydrateMobpackDocumentState(result, options = {}) {
  const document = result?.document && typeof result.document === "object" ? result.document : {};
  const members = Array.isArray(document.members) ? document.members : [];
  const schemas = Array.isArray(document.schemas) ? document.schemas : [];
  const id = String(options.id || flowImportedIdFromDocument(document, result, options.existingRows)).trim();
  const flow = flowFromHydratedDocument(document);
  const errorView = errorViewForState(options.errorView);
  if (!flow) {
    return {
      ok: false,
      id,
      document,
      members,
      schemas,
      flow: null,
      skillRealms: mergeSkillRealms(document.skill_realms, options.contractSkillRealms || []),
      graphProjection: null,
      deploySettings: deploySettingsForUi(options.deployDefaults),
      mobSettings: mobSettingsForUi(options.mobDefaults),
      registryRow: null,
      addToRegistry: false,
      openEditor: false,
      validation: null,
      validationRows: [{
        kind: "crit",
        glyph: errorView.criticalGlyph,
        head: errorView.missingEditorFlowHead,
        sub: errorView.missingEditorFlowSub,
        meta: errorView.missingEditorFlowMeta,
      }],
      stage: "draft",
      error: errorView.missingEditorFlowMeta,
    };
  }
  const skillRealms = mergeSkillRealms(document.skill_realms, options.contractSkillRealms || []);
  const graphProjection = graphProjectionFromMetaResult(result)
    || graphProjectionForDocument({ ...document, flow }, members, options.contract);
  const hasDeploySettings = document.deploy && typeof document.deploy === "object" && !Array.isArray(document.deploy);
  const hasMobSettings = document.mob_settings && typeof document.mob_settings === "object" && !Array.isArray(document.mob_settings);
  const validation = result?.validation || null;
  const validationRows = diagnosticsToRows(validation);
  const stage = validation?.ok ? "valid" : "draft";
  const registryRow = flowRegistryRowFromDocument({
    id,
    document,
    validation,
    stage,
    sourceLabel: result?.source_label || "",
    source: result?.source || "",
    flowRow: options.flowRow || null,
  });
  return {
    id,
    document,
    members,
    schemas,
    flow,
    skillRealms,
    graphProjection,
    deploySettings: deploySettingsForUi(hasDeploySettings ? document.deploy : options.deployDefaults),
    mobSettings: mobSettingsForUi(hasMobSettings ? document.mob_settings : options.mobDefaults),
    registryRow,
    addToRegistry: options.addToRegistry !== false,
    openEditor: options.openEditor !== false,
    validation,
    validationRows,
    stage,
  };
}

export function authoringProjectionFromMetaDocument(document, options = {}) {
  const source = document && typeof document === "object" ? document : {};
  const flow = flowFromHydratedDocument(source) || emptyAuthoringFlowState();
  return {
    document: source,
    flow,
    members: Array.isArray(source.members) ? source.members : [],
    schemas: Array.isArray(source.schemas) ? source.schemas : [],
    skillRealms: Array.isArray(source.skill_realms) ? source.skill_realms : [],
    instances: Array.isArray(source.instances) ? source.instances : [],
    edges: Array.isArray(source.edges) ? source.edges : [],
    frames: Array.isArray(source.frames) ? source.frames : [],
    deploySettings: deploySettingsForUi(source.deploy || options.deployDefaults),
    mobSettings: mobSettingsForUi(source.mob_settings || options.mobDefaults),
  };
}

export function authoringProjectionFromOperationResult(result, options = {}) {
  const document = result?.document && typeof result.document === "object" ? result.document : null;
  if (!document) return null;
  const projection = authoringProjectionFromMetaDocument(document, options);
  const graphProjection = graphProjectionFromMetaResult(result);
  if (graphProjection) {
    projection.instances = graphProjection.instances;
    projection.edges = graphProjection.edges;
    projection.frames = graphProjection.frames;
  }
  return projection;
}

export function flowImportedIdFromDocument(document, result = {}, existingRows = []) {
  const source = result?.source_name || result?.sourceName || result?.filename || result?.source;
  const name = document?.name || document?.mob_id || document?.flow?.name || source || "";
  if (!String(name || "").trim()) return "";
  return flowDraftIdFromSpec({
    name,
  }, existingRows);
}
