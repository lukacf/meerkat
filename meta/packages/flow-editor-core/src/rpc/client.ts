// @ts-nocheck — migration window only (removed by the S23 strictness
// ratchet): the rpc-core functions move byte-verbatim as plain JS, and their
// `param = {}` defaults plus the controllerConfig.authoringOperations expando
// raise TS2339 under .ts semantics. Source-contract pins this exact text
// (e.g. callRpc's signature and `signal: options.signal`), so suppression
// must live at file level, not in the moved bodies. Resolution/linkage stays
// guarded behaviorally: the projection suite and export-keys test load the
// bundle and exercise every function here, so a missed import or re-export
// fails the gate as a ReferenceError.
//
// RPC client core for the Flow Editor controller plane. Moved verbatim from
// the controller.js rpc-core range: endpoint/method configuration, schema
// driven authoring operation metadata, and the JSON-RPC transport (callRpc).
// controllerConfig and requestId are module-level mutable singletons — one
// copy per bundle, the same semantics as the legacy one-copy-per-IIFE.
// callRpc intentionally calls the bare global `fetch` at call time (never
// captured, never imported) so test harnesses can stub global.fetch.
import { RPC_METHODS, SCHEMA_COMMAND_KEYS } from "../shared/constants";

const controllerConfig = {
  rpcUrl: "/flow-editor/rpc",
  rpcMethods: { ...RPC_METHODS },
};

export function configure(options) {
  const rpcUrl = String(options?.rpcUrl || "").trim();
  if (rpcUrl) {
    controllerConfig.rpcUrl = rpcUrl;
  }
}

export function rpcPath() {
  return controllerConfig.rpcUrl || "/flow-editor/rpc";
}

export function operationErrorText(result = null, fallback = "") {
  if (!result || typeof result !== "object") return "";
  const validationRow = result?.validation?.display_rows?.[0] || null;
  const validationMessage = String(validationRow?.sub || "").trim();
  if (validationMessage) return validationMessage;
  const validationHead = String(validationRow?.head || "").trim();
  if (validationHead) return validationHead;
  const error = String(result?.error || "").trim();
  if (error) return error;
  return result.ok === false ? String(fallback || "").trim() : "";
}

export function rpcMethod(name) {
  return controllerConfig.rpcMethods?.[name] || RPC_METHODS[name] || "";
}

export function authoringRpcMethodsFromSchema(schema) {
  const commands = schema?.commands;
  if (!commands || typeof commands !== "object") return {};
  const out = {};
  for (const [name, commandKey] of Object.entries(SCHEMA_COMMAND_KEYS)) {
    const value = String(commands[commandKey] || "").trim();
    if (value) out[name] = value;
  }
  return out;
}

export function authoringOperationsFromSchema(schema) {
  const operations = Array.isArray(schema?.operations) ? schema.operations : [];
  const out = {};
  for (const operation of operations) {
    if (!operation || typeof operation !== "object") continue;
    const type = String(operation.type || "").trim();
    if (!type) continue;
    out[type] = {
      type,
      plane: String(operation.plane || ""),
      authority: String(operation.authority || ""),
      requires: Array.isArray(operation.requires) ? operation.requires.map((item) => String(item || "")).filter(Boolean) : [],
      mutates: Array.isArray(operation.mutates) ? operation.mutates.map((item) => String(item || "")).filter(Boolean) : [],
      projectionDocumentSupported: !!operation.projection_document_supported || !!operation.projectionDocumentSupported,
      raw: operation,
    };
  }
  return out;
}

export function authoringOperationAvailability(operations, type) {
  const operationType = String(type || "").trim();
  const entry = operations && typeof operations === "object" ? operations[operationType] : null;
  return {
    type: operationType,
    supported: !!entry,
    operation: entry || null,
    error: entry || !operationType ? "" : `Meta authoring operation unavailable: ${operationType}`,
  };
}

export function authoringOperationFromIntent(request = {}) {
  const input = request && typeof request === "object" ? request : {};
  if (input.type) return input;
  const intent = String(input.intent || "").trim();
  switch (intent) {
    case "system.syncGraphToFlow":
      return { type: "sync_graph_to_flow", reason: input.reason || "sync_graph_to_flow", selection: input.selection || null };
    case "system.reconcileMembers":
      return { type: "reconcile_members", reason: input.reason || "reconcile_members", selection: input.selection || null };
    case "system.reconcileConditionFields":
      return { type: "reconcile_condition_fields", reason: input.reason || "reconcile_condition_fields", selection: input.selection || null };
    case "system.reconcileContractRefs":
      return { type: "reconcile_contract_refs", reason: input.reason || "reconcile_contract_refs", selection: input.selection || null };
    case "agent.addDefinition":
      return { type: "add_agent_definition", definition_id: input.definitionId };
    case "agent.updateMember":
      return { type: "update_member", member_id: input.memberId, patch: input.patch || {} };
    case "agent.addTool":
      return { type: "add_member_tool", member_id: input.memberId, tool_id: input.toolId };
    case "agent.removeTool":
      return { type: "remove_member_tool", member_id: input.memberId, tool_id: input.toolId };
    case "agent.toggleSkill":
      return { type: "toggle_member_skill", member_id: input.memberId, skill_id: input.skillId };
    case "agent.removeSkill":
      return { type: "remove_member_skill", member_id: input.memberId, skill_id: input.skillId };
    case "agent.createInlineSkill":
      return { type: "create_inline_skill", member_id: input.memberId, label: input.label, content: input.content };
    case "agent.assignSchema":
      return { type: "assign_member_schema", member_id: input.memberId, schema_id: input.schemaId };
    case "agent.deleteMember":
      return { type: "delete_member", member_id: input.memberId };
    case "schema.add":
      return { type: "add_schema" };
    case "schema.update":
      return { type: "update_schema", schema_id: input.schemaId, patch: input.patch || {} };
    case "schema.rename":
      return { type: "rename_schema", schema_id: input.schemaId, new_id: input.newId };
    case "schema.delete":
      return { type: "delete_schema", schema_id: input.schemaId };
    case "schema.addField":
      return { type: "add_schema_field", schema_id: input.schemaId };
    case "schema.updateField":
      return { type: "update_schema_field", schema_id: input.schemaId, field_id: input.fieldId, patch: input.patch || {} };
    case "schema.renameField":
      return { type: "rename_schema_field", schema_id: input.schemaId, field_id: input.fieldId, new_name: input.newName };
    case "schema.deleteField":
      return { type: "delete_schema_field", schema_id: input.schemaId, field_id: input.fieldId };
    case "settings.updateDeploy":
      return { type: "unsupported_settings_replace", selection: input.selection || null };
    case "settings.updateDeployField":
      return { type: "update_deploy_settings", field: input.field, value: input.value, selection: input.selection || null };
    case "settings.updateMob":
      return { type: "unsupported_settings_replace", selection: input.selection || null };
    case "settings.updateMobField":
      return { type: "update_mob_settings", field: input.field, value: input.value, selection: input.selection || null };
    case "settings.updateRoleWiring":
      return { type: "unsupported_settings_replace", selection: input.selection || null };
    case "settings.editRoleWiring":
      return { type: "update_role_wiring", action: input.action, index: input.index, field: input.field, value: input.value, selection: input.selection || null };
    case "basic.updateStep":
      return { type: "update_flow_step", step_id: input.stepId, patch: input.patch || {} };
    case "basic.editStep":
      return { type: "apply_flow_step_edit", step_id: input.stepId, action: input.action, ...(input.payload || {}) };
    case "basic.insertStep":
      return { type: "insert_flow_step", pick: input.pick, lane_ref: input.laneRef };
    case "basic.deleteStep":
      return { type: "delete_flow_step", step_id: input.stepId };
    case "basic.addInputParam":
      return { type: "add_input_param", step_id: input.stepId };
    case "basic.updateInputParam":
      return { type: "update_input_param", step_id: input.stepId, param_id: input.paramId, patch: input.patch || {} };
    case "basic.renameInputParam":
      return { type: "rename_input_param", step_id: input.stepId, param_id: input.paramId, new_name: input.newName };
    case "basic.deleteInputParam":
      return { type: "delete_input_param", step_id: input.stepId, param_id: input.paramId };
    case "graph.insertNode":
      return { type: "insert_graph_node", ...(input.operation || { pick: input.pick, cell: input.cell }) };
    case "graph.editNode":
      return { type: "apply_graph_node_edit", instance_id: input.instanceId, action: input.action, ...(input.payload || {}) };
    case "graph.moveNode":
      return { type: "move_graph_node", instance_id: input.instanceId, cell: input.cell, original_cell: input.originalCell };
    case "graph.deleteNode":
      return { type: "delete_graph_node", instance_id: input.instanceId };
    case "graph.connectNodes":
      return { type: "connect_graph_nodes", ...(input.operation || { from_id: input.fromId, to_id: input.toId }) };
    case "graph.editEdge":
      return { type: "apply_graph_edge_edit", edge_id: input.edgeId, action: input.action, ...(input.payload || {}) };
    case "graph.deleteEdge":
      return { type: "delete_graph_edge", edge_id: input.edgeId };
    default:
      return input;
  }
}

export function inlineSkillRealmIdFromOperationResult(result) {
  const skillId = String(result?.selection?.skill_id || result?.skill_id || "").trim();
  const realms = Array.isArray(result?.document?.skill_realms) ? result.document.skill_realms : [];
  if (!skillId) return "";
  const realm = realms.find(candidate => {
    return Array.isArray(candidate?.skills) && candidate.skills.some(skill => String(skill?.id || "").trim() === skillId);
  });
  return String(realm?.id || "").trim();
}

export function configureAuthoringMethodsFromSchema(schema) {
  const methods = authoringRpcMethodsFromSchema(schema);
  controllerConfig.rpcMethods = { ...RPC_METHODS, ...methods };
  controllerConfig.authoringOperations = authoringOperationsFromSchema(schema);
  return { ...controllerConfig.rpcMethods };
}

let requestId = 0;
export async function callRpc(method, params, options = {}) {
  const response = await fetch(rpcPath(), {
    method: "POST",
    headers: { "content-type": "application/json" },
    ...(options.signal ? { signal: options.signal } : {}),
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: ++requestId,
      method,
      params: params || {},
    }),
  });
  if (!response.ok) {
    throw new Error(`Meta API ${response.status}`);
  }
  const payload = await response.json();
  if (payload.error) {
    throw new Error(payload.error.message || "Meta API error");
  }
  return payload.result;
}
