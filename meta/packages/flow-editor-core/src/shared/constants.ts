// Shared constants for the Flow Editor controller plane: schema/RPC method
// tables and empty-settings skeletons, plus graph node geometry. Moved
// verbatim from the controller.js IIFE head (and graph-editor range for
// GRAPH_NODE_W/H).
export const SCHEMA_VERSION = "0.1.0";
export const RPC_METHODS = {
  schema: "meta/mobpacks/schema",
  catalogs: "meta/mobpacks/catalogs",
  validate: "meta/mobpacks/validate",
  source: "meta/mobpacks/source",
  export: "meta/mobpacks/export",
  import: "meta/mobpacks/import",
  list: "meta/mobpacks/list",
  get: "meta/mobpacks/get",
  create: "meta/mobpacks/create",
  save: "meta/mobpacks/save",
  delete: "meta/mobpacks/delete",
  undo: "meta/mobpacks/undo",
  redo: "meta/mobpacks/redo",
  applyOperation: "meta/mobpacks/apply_operation",
  graphProjection: "meta/mobpacks/graph_projection",
  graphToFlow: "meta/mobpacks/graph_to_flow",
  deployCommand: "meta/mobpacks/deploy_command",
  deploy: "meta/mobpacks/deploy",
};
export const SCHEMA_COMMAND_KEYS = {
  schema: "schema",
  catalogs: "catalogs",
  validate: "validate",
  source: "source",
  export: "export",
  import: "import",
  list: "list",
  get: "get",
  create: "create",
  save: "save",
  delete: "delete",
  undo: "undo",
  redo: "redo",
  applyOperation: "apply_operation",
  graphProjection: "graph_projection",
  graphToFlow: "graph_to_flow",
  deployCommand: "deploy_command",
  deploy: "deploy_rpc",
};
export const EMPTY_DEPLOY_SETTINGS = {
  command: "",
  surface: "",
  trustPolicy: "",
  model: "",
  maxDuration: "",
  maxToolCalls: null,
  maxTotalTokens: null,
  isolated: false,
  realm: "",
  instance: "",
  realmBackend: "",
  contextRoot: "",
  stateRoot: "",
  userConfigRoot: "",
  prompt: "",
};
export const EMPTY_MOB_SETTINGS = {
  orchestrator: "",
  autoWireOrchestrator: false,
  roleWiring: [],
  backendDefault: "",
  externalAddressBase: "",
  advanced: {
    topology: null,
    supervisor: null,
    limits: null,
    spawnPolicy: null,
    eventRouter: null,
  },
};
export const MOB_SETTINGS_PATCH_KEYS = new Set(Object.keys(EMPTY_MOB_SETTINGS));

export const GRAPH_NODE_W = 200;
export const GRAPH_NODE_H = 156;
