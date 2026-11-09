#!/usr/bin/env node

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

global.window = { __META_FLOW_CONTROLLER_TEST__: true };
global.document = { querySelector: () => null };
global.Blob = class Blob {};
global.URL.createObjectURL = global.URL.createObjectURL || (() => "blob:meta-live-test");
global.URL.revokeObjectURL = global.URL.revokeObjectURL || (() => {});
// The controller links exactly like the projection suite: the core package
// IIFE plus the window.MetaFlowController bootstrap shim, emitted by
// `node build.cjs --test-bundle` (the test:rkat-e2e scripts run it first).
require("../.tmp/controller-under-test.cjs");

const rpcUrl = process.env.META_FLOW_EDITOR_RPC_URL || "http://127.0.0.1:4191/flow-editor/rpc";
const sampleId = process.env.META_FLOW_EDITOR_SAMPLE_ID || "sample_docs_only";
const runDeploy = process.argv.includes("--deploy") || process.env.META_FLOW_EDITOR_RUN_DEPLOY === "1";
const hostDeployExpectation = process.env.META_FLOW_EDITOR_EXPECT_HOST_DEPLOY;
if (hostDeployExpectation && !["0", "1"].includes(hostDeployExpectation)) {
  throw new Error("META_FLOW_EDITOR_EXPECT_HOST_DEPLOY must be 0 or 1 when set");
}
const hasExplicitHostDeployExpectation = hostDeployExpectation === "0" || hostDeployExpectation === "1";
const expectHostDeploy = hostDeployExpectation === "1";
const controller = global.window.MetaFlowController;
let contractSchema = null;
const testDeploySettings = () => controller.deployDefaultsFromSchema(contractSchema);
const testMobSettings = () => controller.mobDefaultsFromSchema(contractSchema);

function sha256(bytes) {
  return crypto.createHash("sha256").update(bytes).digest("hex");
}

async function rpc(method, params) {
  const requestParams = method === "meta/mobpacks/validate"
    ? { ...(params || {}), rkat_validate: true }
    : (params || {});
  const response = await fetch(rpcUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: Math.floor(Math.random() * 1e9),
      method,
      params: requestParams,
    }),
  });
  if (!response.ok) throw new Error(`${method} HTTP ${response.status}`);
  const payload = await response.json();
  if (payload.error) throw new Error(`${method}: ${payload.error.message}`);
  return payload.result;
}

async function rpcExpectError(method, params) {
  const response = await fetch(rpcUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: Math.floor(Math.random() * 1e9),
      method,
      params: params || {},
    }),
  });
  if (!response.ok) throw new Error(`${method} HTTP ${response.status}`);
  const payload = await response.json();
  if (!payload.error) throw new Error(`${method} unexpectedly succeeded: ${JSON.stringify(payload.result)}`);
  return payload.error;
}

async function assertAuthoringCapabilities() {
  const capabilities = await rpc("meta/capabilities", {});
  const authoring = capabilities.authoring_capabilities || {};
  const expectedMethods = [
    "meta/mobpacks/schema",
    "meta/mobpacks/catalogs",
    "meta/tools/catalog",
    "meta/skills/catalog",
    "meta/agent_definitions/list",
    "meta/mobpacks/templates",
    "meta/mobpacks/validate",
    "meta/mobpacks/source",
    "meta/mobpacks/export",
    "meta/mobpacks/import",
    "meta/mobpacks/list",
    "meta/mobpacks/get",
    "meta/mobpacks/create",
    "meta/mobpacks/save",
    "meta/mobpacks/delete",
    "meta/mobpacks/apply_operation",
    "meta/mobpacks/undo",
    "meta/mobpacks/redo",
    "meta/mobpacks/graph_projection",
    "meta/mobpacks/graph_to_flow",
    "meta/mobpacks/deploy_command",
    "meta/mobpacks/deploy",
  ];
  if (authoring.domain !== "mobpack_authoring") {
    throw new Error(`flow editor capabilities expose wrong authoring domain: ${JSON.stringify(authoring)}`);
  }
  if (authoring.runtime_mutation !== false) {
    throw new Error(`flow editor authoring capabilities must not mutate runtime: ${JSON.stringify(authoring)}`);
  }
  if (authoring.host_mutation_methods?.["meta/mobpacks/deploy"] !== "when execute=true, writes a mobpack archive and runs rkat mob run on the host") {
    throw new Error(`flow editor capabilities must disclose deploy host mutation: ${JSON.stringify(authoring)}`);
  }
  if (authoring.host_mutation_allowed !== authoring.deploy_execute_allowed) {
    throw new Error(`flow editor host deploy capability flags disagree: ${JSON.stringify(authoring)}`);
  }
  const hostDeployAvailable = authoring.host_mutation_allowed === true;
  if (hasExplicitHostDeployExpectation && hostDeployAvailable !== expectHostDeploy) {
    const mode = expectHostDeploy ? "host deploy opt-in" : "safe standalone";
    throw new Error(`${mode} flow editor exposed wrong host deploy capability: ${JSON.stringify(authoring)}`);
  }
  const expectedAuthMode = hostDeployAvailable ? "standalone_host_deploy" : "none";
  if (capabilities.authenticated !== false || capabilities.auth?.mode !== expectedAuthMode) {
    throw new Error(`standalone flow editor must not claim authenticated runtime access: ${JSON.stringify(capabilities)}`);
  }
  if (authoring.deploy_command !== "rkat mob run") {
    throw new Error(`flow editor deploy command must be rkat mob run: ${JSON.stringify(authoring)}`);
  }
  const operations = array(authoring.operations, "authoring.operations");
  for (const operationType of [
    "delete_member",
    "add_member_tool",
    "remove_member_tool",
    "toggle_member_skill",
    "remove_member_skill",
    "create_inline_skill",
    "rename_schema_field",
    "delete_schema",
    "add_input_param",
    "rename_input_param",
    "delete_input_param",
    "insert_flow_step",
    "update_flow_step",
    "delete_flow_step",
    "insert_graph_node",
    "apply_graph_node_edit",
    "move_graph_node",
    "delete_graph_node",
    "connect_graph_nodes",
    "apply_graph_edge_edit",
    "delete_graph_edge",
    "update_deploy_settings",
  ]) {
    const operation = operations.find((candidate) => candidate?.type === operationType);
    if (!operation) {
      throw new Error(`flow editor authoring capabilities missing operation ${operationType}: ${JSON.stringify(operations)}`);
    }
    if (operation.authority !== "meta") {
      throw new Error(`flow editor operation ${operationType} must be Meta-authoritative: ${JSON.stringify(operation)}`);
    }
  }
  for (const method of expectedMethods) {
    if (!array(authoring.methods, "authoring.methods").includes(method)) {
      throw new Error(`flow editor authoring capabilities missing ${method}: ${JSON.stringify(authoring.methods)}`);
    }
    if (!array(capabilities.methods, "capabilities.methods").includes(method)) {
      throw new Error(`flow editor methods missing ${method}: ${JSON.stringify(capabilities.methods)}`);
    }
  }
  const allowedStandaloneMethods = new Set(["meta/capabilities", ...expectedMethods]);
  for (const method of array(capabilities.methods, "capabilities.methods")) {
    if (!allowedStandaloneMethods.has(method)) {
      throw new Error(`standalone flow editor exposed non-authoring RPC method ${method}`);
    }
  }
  return {
    domain: authoring.domain,
    deployCommand: authoring.deploy_command,
    hostDeployAvailable,
    methods: authoring.methods,
    operations: operations.map((operation) => operation.type).filter(Boolean),
  };
}

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  const output = [result.stdout, result.stderr].filter(Boolean).join("").trim();
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(" ")} failed with ${result.status}\n${output}`);
  }
  return output;
}

function array(value, label) {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
}

function assertRoundTrip(imported, sourceDocument) {
  const document = imported?.document;
  if (!document || typeof document !== "object") throw new Error("imported mobpack did not return document");
  if (imported.validation?.ok !== true) {
    throw new Error(`imported mobpack validation failed: ${JSON.stringify(imported.validation?.diagnostics)}`);
  }

  const members = array(document.members, "imported document.members");
  const flowSteps = array(document.flow?.steps, "imported document.flow.steps");
  const instances = array(document.instances, "imported document.instances");
  const edges = array(document.edges, "imported document.edges");
  const frames = array(document.frames, "imported document.frames");
  const launchModes = array(document.launch_modes, "imported document.launch_modes");

  if (members.length === 0) throw new Error("imported document has no real member definitions");
  if (!flowSteps.some((step) => step.type === "member")) throw new Error("imported flow has no member turns");
  if (!instances.some((instance) => instance.memberId && !instance.isGate)) throw new Error("imported graph has no member instances");
  if (edges.length === 0) throw new Error("imported graph has no edges");
  if (launchModes.length === 0) throw new Error("imported document has no launch modes");

  const sourceSchemaIds = new Set(array(sourceDocument.schemas || [], "source document.schemas").map((schema) => schema.id).filter(Boolean));
  const importedSchemaIds = new Set(array(document.schemas || [], "imported document.schemas").map((schema) => schema.id).filter(Boolean));
  for (const schemaId of sourceSchemaIds) {
    if (!importedSchemaIds.has(schemaId)) throw new Error(`imported document dropped schema ${schemaId}`);
  }

  const sourceFrameKinds = new Set(array(sourceDocument.frames || [], "source document.frames").map((frame) => frame.kind).filter(Boolean));
  if (sourceFrameKinds.size > 0 && frames.length === 0) throw new Error("imported document dropped flow frames");

  return {
    mob_id: document.mob_id,
    members: members.length,
    flowSteps: flowSteps.length,
    instances: instances.length,
    edges: edges.length,
    frames: frames.length,
    schemas: importedSchemaIds.size,
    launchModes: launchModes.length,
  };
}

function assertDeployPlanTrace(result, label) {
  const trace = array(result.plan_trace, `${label}.plan_trace`);
  const heads = trace.map((row) => String(row?.head || ""));
  for (const prefix of ["MOBPACK ·", "PROFILE ·", "FLOW ·", "STEP ·", "VALIDATION ·"]) {
    if (!heads.some((head) => head.startsWith(prefix))) {
      throw new Error(`${label} deploy plan_trace missing ${prefix} row: ${JSON.stringify(trace)}`);
    }
  }
  const firstBody = String(trace[0]?.body || "");
  if (!firstBody.includes("source: meta/mob.toml") || !/command: .*rkat mob run/.test(firstBody)) {
    throw new Error(`${label} deploy plan_trace did not describe the Meta deploy source/command: ${JSON.stringify(trace[0])}`);
  }
  if (!trace.some((row) => String(row?.body || "").includes("skills:") && String(row?.body || "").includes("tools:"))) {
    throw new Error(`${label} deploy plan_trace did not include profile tools/skills from the parsed Meta definition: ${JSON.stringify(trace)}`);
  }
  return {
    rows: trace.length,
    heads: heads.slice(0, 5),
  };
}

function buildGraphBranchShapeDocument() {
  const members = [
    {
      id: "m_writer",
      name: "writer",
      role: "writer",
      model: "gpt-5.5",
      systemPrompt: "Write the selected work item.",
      tools: ["builtins", "comms"],
      skills: [],
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
    {
      id: "m_reviewer",
      name: "reviewer",
      role: "reviewer",
      model: "gpt-5.5",
      systemPrompt: "Review the fallback work item.",
      tools: ["builtins", "comms"],
      skills: [],
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
  ];
  const previousFlow = {
    name: "graph-branch-shape",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Route the graph branch.",
      fields: "",
      inputParams: [{
        id: "p_route",
        name: "route",
        type: "enum",
        required: true,
        description: "Graph branch route.",
        enumValues: ["a", "fallback"],
      }],
    }, {
      id: "branch_writer",
      type: "member",
      role: "m_writer",
      instruction: "Write the route A branch output.",
    }, {
      id: "branch_reviewer",
      type: "member",
      role: "m_reviewer",
      instruction: "Review the fallback branch output.",
    }],
  };
  const instances = [
    { id: "g_branch_route", isGate: true, gateKind: "branch", label: "branch", col: 0, row: 0 },
    { id: "branch_writer", memberId: "m_writer", col: 1, row: 0, lane: "route = a", launchMode: { kind: "Fresh" } },
    { id: "branch_reviewer", memberId: "m_reviewer", col: 1, row: 1, lane: "fallback", launchMode: { kind: "Fresh" } },
    { id: "j_branch_route", isGate: true, gateKind: "join", label: "join · branch paths", collection: "any", controllerRole: "m_reviewer", col: 2, row: 0 },
  ];
  const edges = [
    { id: "e_gate_writer", from: "g_branch_route", to: "branch_writer", kind: "cond", label: "route == \"a\"", cond: { var: "params.route", op: "==", val: "a" } },
    { id: "e_gate_reviewer", from: "g_branch_route", to: "branch_reviewer", kind: "next", label: "fallback" },
    { id: "e_writer_join", from: "branch_writer", to: "j_branch_route", kind: "next", label: "" },
    { id: "e_reviewer_join", from: "branch_reviewer", to: "j_branch_route", kind: "next", label: "" },
  ];
  const flow = controller.graphToFlow({
    previousFlow,
    members,
    instances,
    edges,
    contract: contractSchema,
  });
  const branch = flow.steps.find((step) => step.type === "branch");
  if (!branch) throw new Error("graph branch shape did not compile to a branch step");
  if (branch.controllerRole !== "m_reviewer") throw new Error(`graph branch shape dropped real join member: ${JSON.stringify(branch)}`);
  if (branch.branches.length !== 1) throw new Error(`expected one conditional branch, got ${branch.branches.length}`);
  if (branch.fallback.length !== 1) throw new Error(`expected one fallback step, got ${branch.fallback.length}`);
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas: [],
      instances,
      edges,
      frames: [],
      skillRealms: [],
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "graph-branch-shape" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateGraphBranchShape(dir) {
  const document = buildGraphBranchShapeDocument();
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`graph branch shape failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "graph-branch-shape.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`graph branch shape export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  if (!String(exported.mob_toml || "").includes("Join branch paths.")) {
    throw new Error(`graph branch export did not render a real branch join step:\n${exported.mob_toml}`);
  }
  const packPath = path.join(dir, exported.filename || "graph-branch-shape.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const branch = imported.document?.flow?.steps?.find((step) => step.type === "branch");
  if (!branch) throw new Error("imported graph branch shape dropped branch step");
  if (branch.controllerRole !== "m_reviewer") throw new Error(`imported graph branch shape dropped join member: ${JSON.stringify(branch)}`);
  if (!branch.branches?.[0]?.cond || branch.branches[0].cond.field !== "route") {
    throw new Error(`imported graph branch shape dropped route condition: ${JSON.stringify(branch.branches?.[0])}`);
  }
  if (!Array.isArray(branch.fallback) || branch.fallback.length !== 1) {
    throw new Error(`imported graph branch shape dropped fallback: ${JSON.stringify(branch.fallback)}`);
  }
  return {
    validate,
    branchCount: branch.branches.length,
    joinMember: branch.controllerRole,
    fallbackCount: branch.fallback.length,
    frameKinds: (imported.document.frames || []).map((frame) => frame.kind),
    edgeKinds: (imported.document.edges || []).map((edge) => edge.kind),
  };
}

function buildGraphParallelShapeDocument() {
  const members = [
    {
      id: "m_writer",
      name: "writer",
      role: "writer",
      model: "gpt-5.5",
      systemPrompt: "Write one side of the parallel result.",
      tools: ["builtins", "comms"],
      skills: [],
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
    {
      id: "m_reviewer",
      name: "reviewer",
      role: "reviewer",
      model: "gpt-5.5",
      systemPrompt: "Review the other side of the parallel result.",
      tools: ["builtins", "comms"],
      skills: [],
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
  ];
  const previousFlow = {
    name: "graph-parallel-shape",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Run the graph parallel lanes.",
      fields: "",
      inputParams: [],
    }, {
      id: "parallel_writer",
      type: "member",
      role: "m_writer",
      instruction: "Write the first parallel lane result.",
    }, {
      id: "parallel_reviewer",
      type: "member",
      role: "m_reviewer",
      instruction: "Review the second parallel lane result.",
    }],
  };
  const instances = [
    { id: "g_parallel_work", isGate: true, gateKind: "fork", label: "fan_out", dispatch: "fan_out", col: 0, row: 0 },
    { id: "parallel_writer", memberId: "m_writer", col: 1, row: 0, lane: "lane 1", launchMode: { kind: "Fresh" } },
    { id: "parallel_reviewer", memberId: "m_reviewer", col: 1, row: 1, lane: "lane 2", launchMode: { kind: "Fresh" } },
    { id: "j_parallel_work", isGate: true, gateKind: "join", label: "join · all", collection: "all", col: 2, row: 0 },
  ];
  const edges = [
    { id: "e_gate_writer", from: "g_parallel_work", to: "parallel_writer", kind: "fanout", label: "" },
    { id: "e_gate_reviewer", from: "g_parallel_work", to: "parallel_reviewer", kind: "fanout", label: "" },
    { id: "e_writer_join", from: "parallel_writer", to: "j_parallel_work", kind: "next", label: "" },
    { id: "e_reviewer_join", from: "parallel_reviewer", to: "j_parallel_work", kind: "next", label: "" },
  ];
  const flow = controller.graphToFlow({
    previousFlow,
    members,
    instances,
    edges,
    contract: contractSchema,
  });
  const parallel = flow.steps.find((step) => step.type === "parallel");
  if (!parallel) throw new Error("graph parallel shape did not compile to a parallel step");
  if (parallel.branches.length !== 2) throw new Error(`expected two parallel branches, got ${parallel.branches.length}`);
  if (parallel.dispatch !== "fan_out") throw new Error(`expected fan_out dispatch, got ${parallel.dispatch}`);
  if (parallel.collection !== "all") throw new Error(`expected all collection, got ${parallel.collection}`);
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas: [],
      instances,
      edges,
      frames: [],
      skillRealms: [],
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "graph-parallel-shape" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateGraphParallelShape(dir) {
  const document = buildGraphParallelShapeDocument();
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`graph parallel shape failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "graph-parallel-shape.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`graph parallel shape export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const packPath = path.join(dir, exported.filename || "graph-parallel-shape.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const parallel = imported.document?.flow?.steps?.find((step) => step.type === "parallel");
  if (!parallel) throw new Error("imported graph parallel shape dropped parallel step");
  if (parallel.branches?.length !== 2) {
    throw new Error(`imported graph parallel shape dropped branches: ${JSON.stringify(parallel.branches)}`);
  }
  if (parallel.dispatch !== "fan_out" || parallel.collection !== "all") {
    throw new Error(`imported graph parallel shape changed dispatch/collection: ${JSON.stringify(parallel)}`);
  }
  return {
    validate,
    branchCount: parallel.branches.length,
    dispatch: parallel.dispatch,
    collection: parallel.collection,
    frameKinds: (imported.document.frames || []).map((frame) => frame.kind),
    edgeKinds: (imported.document.edges || []).map((edge) => edge.kind),
  };
}

function buildGraphLoopShapeDocument() {
  const members = [
    {
      id: "m_coder",
      name: "coder",
      role: "coder",
      model: "gpt-5.5",
      systemPrompt: "Implement the current iteration.",
      tools: ["builtins", "comms"],
      skills: [],
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
    {
      id: "m_reviewer",
      name: "reviewer",
      role: "reviewer",
      model: "gpt-5.5",
      systemPrompt: "Review the iteration and emit a verdict.",
      tools: ["builtins", "comms"],
      skills: [],
      schema: "ReviewArtifact",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
    },
  ];
  const schemas = [{
    id: "ReviewArtifact",
    description: "Review output for graph loop proof.",
    fields: [{
      id: "f_verdict",
      name: "verdict",
      type: "enum",
      required: true,
      description: "Whether the loop can exit.",
      enumValues: ["green", "red"],
    }],
  }];
  const previousFlow = {
    name: "graph-loop-shape",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Run the graph loop until review is green.",
      fields: "",
      inputParams: [],
    }, {
      id: "quality_loop",
      type: "repeat",
      loopId: "quality_loop",
      maxIterations: 4,
      iterationInput: "carry",
      cond: { stepId: "loop_reviewer", field: "verdict", op: "==", val: "green" },
      steps: [{
        id: "loop_coder",
        type: "member",
        role: "m_coder",
        instruction: "Implement the next loop iteration.",
      }, {
        id: "loop_reviewer",
        type: "member",
        role: "m_reviewer",
        instruction: "Review the loop iteration and emit the verdict.",
      }],
    }],
  };
  const instances = [
    { id: "loop_coder", memberId: "m_coder", col: 0, row: 0, lane: "implement", launchMode: { kind: "Fresh" } },
    { id: "loop_reviewer", memberId: "m_reviewer", col: 1, row: 0, lane: "review", launchMode: { kind: "Fresh" } },
  ];
  const edges = [
    { id: "e_coder_reviewer", from: "loop_coder", to: "loop_reviewer", kind: "next", label: "" },
    { id: "e_reviewer_coder", from: "loop_reviewer", to: "loop_coder", kind: "cond", label: "until green", cond: { var: "steps.loop_reviewer.verdict", op: "==", val: "green" } },
  ];
  const flow = controller.graphToFlow({
    previousFlow,
    members,
    instances,
    edges,
    contract: contractSchema,
  });
  const repeat = flow.steps.find((step) => step.type === "repeat");
  if (!repeat) throw new Error("graph loop shape did not compile to a repeat step");
  if (repeat.steps.length !== 2) throw new Error(`expected two repeat body steps, got ${repeat.steps.length}`);
  if (repeat.maxIterations !== 4 || repeat.iterationInput !== "carry") {
    throw new Error(`graph loop shape did not preserve authored repeat metadata: ${JSON.stringify(repeat)}`);
  }
  if (repeat.cond?.stepId !== "loop_reviewer" || repeat.cond?.field !== "verdict") {
    throw new Error(`graph loop shape changed repeat condition: ${JSON.stringify(repeat.cond)}`);
  }
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas,
      instances,
      edges,
      frames: [],
      skillRealms: [],
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "graph-loop-shape" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateGraphLoopShape(dir) {
  const document = buildGraphLoopShapeDocument();
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`graph loop shape failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "graph-loop-shape.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`graph loop shape export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const packPath = path.join(dir, exported.filename || "graph-loop-shape.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const repeat = imported.document?.flow?.steps?.find((step) => step.type === "repeat");
  if (!repeat) throw new Error("imported graph loop shape dropped repeat step");
  if (repeat.cond?.field !== "verdict" || repeat.cond?.val !== "green") {
    throw new Error(`imported graph loop shape changed repeat condition: ${JSON.stringify(repeat.cond)}`);
  }
  return {
    validate,
    bodyCount: repeat.steps.length,
    condition: repeat.cond,
    frameKinds: (imported.document.frames || []).map((frame) => frame.kind),
    edgeKinds: (imported.document.edges || []).map((edge) => edge.kind),
    schemas: (imported.document.schemas || []).map((schema) => schema.id),
  };
}

function buildEditedAgentDefinitionDocument() {
  const members = [{
    id: "m_quality_agent",
    name: "quality_agent",
    role: "quality_agent",
    model: "gpt-5.5",
    systemPrompt: "Inspect the requested change and emit a structured quality verdict.",
    tools: ["builtins", "shell", "comms"],
    skills: ["mob.editor.quality"],
    schema: "QualityVerdict",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    backend: "session",
    maxInlinePeerNotifications: 4,
    // meerkat 0.7 provider params are typed (ProviderParamsOverride,
    // deny_unknown_fields): thinking_budget -> thinking_budget_tokens;
    // top_k is not a per-turn knob. Integer knobs only — f32 fields like
    // top_p do not round-trip exactly through the TOML projection check.
    providerParams: { thinking_budget_tokens: 4096, max_output_tokens: 2048 },
  }];
  const schemas = [{
    id: "QualityVerdict",
    description: "Structured verdict emitted by the edited quality agent.",
    fields: [
      {
        id: "f_verdict",
        name: "verdict",
        type: "enum",
        required: true,
        description: "Whether the change is accepted.",
        enumValues: ["green", "red"],
      },
      {
        id: "f_findings",
        name: "findings",
        type: "string[]",
        required: true,
        description: "Blocking findings or an empty list.",
      },
    ],
  }];
  const skillRealms = [{
    id: "meta/editor-inline",
    label: "This mobpack",
    source: "editor",
    default: true,
    skills: [{
      id: "mob.editor.quality",
      label: "mob.editor.quality",
      source: "inline",
      content: "Inspect behavior, tools, and schema evidence before returning a quality verdict.",
      desc: "Inline quality review skill stored in this mobpack.",
    }],
  }];
  const flow = {
    name: "edited-agent-definition",
    steps: [
      {
        id: "input_1",
        type: "input",
        task: "Inspect the edited agent definition.",
        fields: "",
        inputParams: [],
      },
      {
        id: "quality_turn",
        type: "member",
        role: "m_quality_agent",
        instruction: "Run the quality agent and emit a QualityVerdict.",
        launchMode: { kind: "Fresh" },
        allowedTools: ["builtins", "shell"],
        blockedTools: ["comms"],
        outputFormat: "json",
      },
    ],
  };
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas,
      instances: [],
      edges: [],
      frames: [],
      skillRealms,
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "edited-agent-definition" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateEditedAgentDefinition(dir) {
  const document = buildEditedAgentDefinitionDocument();
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`edited agent definition failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "edited-agent-definition.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`edited agent definition export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const mobToml = exported.mob_toml || "";
  for (const required of [
    "[profiles.quality_agent]",
    "skills = [\"mob.editor.quality\"]",
    "[profiles.quality_agent.tools]",
    "builtins = true",
    "shell = true",
    "comms = true",
    "[profiles.quality_agent.output_schema]",
    "[skills.\"mob.editor.quality\"]",
    "source = \"inline\"",
  ]) {
    if (!mobToml.includes(required)) {
      throw new Error(`edited agent mob.toml missing ${required}\n${mobToml}`);
    }
  }
  const packPath = path.join(dir, exported.filename || "edited-agent-definition.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const member = imported.document?.members?.find((candidate) => candidate.id === "m_quality_agent");
  if (!member) throw new Error("imported edited agent definition dropped quality agent member");
  if (!["builtins", "shell", "comms"].every((tool) => member.tools?.includes(tool))) {
    throw new Error(`imported edited agent definition dropped tool refs: ${JSON.stringify(member.tools)}`);
  }
  if (!member.skills?.includes("mob.editor.quality")) {
    throw new Error(`imported edited agent definition dropped inline skill ref: ${JSON.stringify(member.skills)}`);
  }
  if (member.schema !== "QualityVerdict") {
    throw new Error(`imported edited agent definition changed schema: ${JSON.stringify(member.schema)}`);
  }
  const importedSkill = (imported.document.skill_realms || [])
    .flatMap((realm) => realm.skills || [])
    .find((skill) => skill.id === "mob.editor.quality");
  if (!importedSkill || importedSkill.source !== "inline" || !importedSkill.content?.includes("quality verdict")) {
    throw new Error(`imported edited agent definition dropped inline skill body: ${JSON.stringify(importedSkill)}`);
  }
  const flowStep = imported.document?.flow?.steps?.find((step) => step.id === "quality_turn");
  if (!flowStep || flowStep.allowedTools?.length !== 2 || flowStep.blockedTools?.[0] !== "comms") {
    throw new Error(`imported edited agent definition changed step tool limits: ${JSON.stringify(flowStep)}`);
  }
  return {
    validate,
    member: {
      model: member.model,
      tools: member.tools,
      skills: member.skills,
      schema: member.schema,
      runtimeMode: member.runtimeMode,
      backend: member.backend,
      maxInlinePeerNotifications: member.maxInlinePeerNotifications,
      providerParams: member.providerParams,
    },
    schemaIds: (imported.document.schemas || []).map((schema) => schema.id),
    skillIds: (imported.document.skill_realms || []).flatMap((realm) => (realm.skills || []).map((skill) => skill.id)),
  };
}

function buildFilesystemSkillDocument(skillPath) {
  const members = [{
    id: "m_platform_agent",
    name: "platform_agent",
    role: "platform_agent",
    model: "gpt-5.5",
    systemPrompt: "Use the selected Meta platform skill to answer from the real contract.",
    tools: ["builtins", "comms"],
    skills: ["mob.platform"],
    profileBinding: "inline",
    runtimeMode: "turn_driven",
  }];
  const skillRealms = [{
    id: "local/filesystem",
    label: "Local filesystem",
    source: "filesystem",
    skills: [{
      id: "mob.platform",
      label: "Meta Platform",
      source: "path",
      origin: "filesystem",
      path: skillPath,
      desc: "Filesystem skill packed into the deployable mobpack.",
    }],
  }];
  const flow = {
    name: "filesystem-skill-definition",
    steps: [
      {
        id: "input_1",
        type: "input",
        task: "Run the filesystem skill proof.",
        fields: "",
        inputParams: [],
      },
      {
        id: "platform_turn",
        type: "member",
        role: "m_platform_agent",
        instruction: "Use the Meta platform skill and cite the packed contract.",
        launchMode: { kind: "Fresh" },
      },
    ],
  };
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas: [],
      instances: [],
      edges: [],
      frames: [],
      skillRealms,
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "filesystem-skill-definition" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateFilesystemSkillPacking(dir) {
  const skillPath = path.join(dir, "SKILL.md");
  const skillContent = "Use the real Meta platform contract from this packed filesystem skill.";
  fs.writeFileSync(skillPath, skillContent);

  const document = buildFilesystemSkillDocument(skillPath);
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`filesystem skill definition failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "filesystem-skill-definition.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`filesystem skill export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const mobToml = exported.mob_toml || "";
  for (const required of [
    "[skills.\"mob.platform\"]",
    "source = \"path\"",
    "path = \"skills/mob-platform.md\"",
  ]) {
    if (!mobToml.includes(required)) {
      throw new Error(`filesystem skill mob.toml missing ${required}\n${mobToml}`);
    }
  }
  if (mobToml.includes(skillPath) || mobToml.includes(dir)) {
    throw new Error(`filesystem skill export leaked authoring path into mob.toml\n${mobToml}`);
  }

  const packPath = path.join(dir, exported.filename || "filesystem-skill-definition.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const archiveList = run("tar", ["-tzf", packPath]);
  if (!archiveList.split(/\r?\n/).includes("skills/mob-platform.md")) {
    throw new Error(`filesystem skill archive missing packed skill file:\n${archiveList}`);
  }
  const archivedSkill = run("tar", ["-xOf", packPath, "skills/mob-platform.md"]);
  if (archivedSkill !== skillContent) {
    throw new Error(`filesystem skill archive content changed: ${JSON.stringify(archivedSkill)}`);
  }

  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  fs.unlinkSync(skillPath);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const importedValidation = await rpc("meta/mobpacks/validate", { document: imported.document });
  if (!importedValidation.ok) {
    throw new Error(`imported packed filesystem skill failed validation after original file removal: ${JSON.stringify(importedValidation.diagnostics)}`);
  }
  const importedSkill = (imported.document?.skill_realms || [])
    .flatMap((realm) => realm.skills || [])
    .find((skill) => skill.id === "mob.platform");
  if (!importedSkill || importedSkill.source !== "path" || importedSkill.content !== skillContent) {
    throw new Error(`imported filesystem skill dropped packed content: ${JSON.stringify(importedSkill)}`);
  }
  const importedMember = imported.document?.members?.find((member) => member.id === "m_platform_agent");
  if (!importedMember?.skills?.includes("mob.platform")) {
    throw new Error(`imported filesystem skill dropped member skill ref: ${JSON.stringify(importedMember)}`);
  }
  return {
    validate,
    archivePath: "skills/mob-platform.md",
    source: importedSkill.source,
    hasPackedContent: importedSkill.content === skillContent,
    memberSkills: importedMember.skills,
  };
}

async function buildUnifiedProjectionDocument(catalogs) {
  const definitions = controller.agentDefinitionsFromCatalogs(catalogs);
  const coderDefinition = definitions.find((definition) => definition.role === "coder") || definitions[0];
  const reviewerDefinition = definitions.find((definition) => definition.role === "reviewer" && definition.sourceOrigin === "meta/sample-mobpack")
    || definitions.find((definition) => definition.role === "reviewer")
    || definitions[1]
    || definitions[0];
  if (!coderDefinition || !reviewerDefinition) {
    throw new Error("unified projection proof needs Meta agent definitions from meta/mobpacks/catalogs");
  }
  const modelCatalog = controller.modelCatalogFromCatalogs(catalogs);
  const model = modelCatalog[0]?.id || coderDefinition.model || reviewerDefinition.model || "gpt-5.5";
  const toolCatalog = controller.toolCatalogFromCatalogs(catalogs);
  if (!toolCatalog.length) {
    throw new Error("unified projection proof needs real Meta tools from meta/mobpacks/catalogs");
  }
  const toolIds = toolCatalog.map((tool) => tool.id).filter(Boolean);
  for (const required of ["builtins", "shell", "comms", "mob"]) {
    if (!toolIds.includes(required)) throw new Error(`unified projection proof missing real tool ${required}`);
  }
  const catalogSkillRealms = controller.skillRealmsFromCatalogs(catalogs);
  const leakedSampleRealm = catalogSkillRealms
    .find((realm) => realm.id === "meta/sample-mobpacks" || realm.source === "meta/sample-mobpack");
  if (leakedSampleRealm) {
    throw new Error(`global skill catalog leaked sample mobpack skills: ${JSON.stringify(leakedSampleRealm)}`);
  }
  const sampleSkills = (catalogs.sample_mobpacks || [])
    .flatMap((sample) => sample.document?.skill_realms || [])
    .flatMap((realm) => realm.skills || [])
    .filter((skill) => ["mob.workpad", "mob.review"].includes(skill.id));
  if (sampleSkills.length < 2) {
    throw new Error("unified projection proof needs real Meta sample mobpack skills from sample_mobpacks");
  }
  if (!sampleSkills.every((skill) => skill.source)) {
    throw new Error(`unified projection proof needs Meta sample skill source metadata: ${JSON.stringify(sampleSkills)}`);
  }
  const sampleSkillRealm = {
    id: "meta/sample-mobpacks",
    label: "Meta sample skills",
    source: "meta/sample-mobpack",
    skills: sampleSkills,
  };
  const inlineSkillRealm = {
    id: "meta/editor-inline",
    label: "This mobpack",
    source: "editor",
    skills: [{
      id: "mob.editor.unified",
      label: "mob.editor.unified",
      source: "inline",
      content: "Keep Basic, Graph, and Agent editor projections synchronized against the same deployable mobpack.",
    }],
  };
  const skillRealms = [...catalogSkillRealms, sampleSkillRealm, inlineSkillRealm];

  const baseDocument = catalogs.blank_mobpack?.document;
  if (!baseDocument || typeof baseDocument !== "object") {
    throw new Error("unified projection proof needs Meta blank mobpack document");
  }
  const coderAdd = await rpc("meta/mobpacks/apply_operation", {
    document: baseDocument,
    operation: { type: "add_agent_definition", definition_id: coderDefinition.id },
  });
  if (coderAdd.ok === false || !coderAdd.document || !coderAdd.selection?.id) {
    throw new Error(`unified projection proof could not add coder definition through Meta apply_operation: ${JSON.stringify(coderAdd)}`);
  }
  const reviewerAdd = await rpc("meta/mobpacks/apply_operation", {
    document: coderAdd.document,
    operation: { type: "add_agent_definition", definition_id: reviewerDefinition.id },
  });
  if (reviewerAdd.ok === false || !reviewerAdd.document || !reviewerAdd.selection?.id) {
    throw new Error(`unified projection proof could not add reviewer definition through Meta apply_operation: ${JSON.stringify(reviewerAdd)}`);
  }
  const addedMembers = array(reviewerAdd.document.members, "apply_operation document.members");
  const coderSource = addedMembers.find((member) => member.sourceDefinition?.definitionId === coderDefinition.id);
  const reviewerSource = addedMembers.find((member) => member.sourceDefinition?.definitionId === reviewerDefinition.id);
  if (!coderSource?.sourceDefinition || !reviewerSource?.sourceDefinition) {
    throw new Error(`Meta apply_operation did not preserve sourceDefinition provenance: ${JSON.stringify(addedMembers)}`);
  }

  const coder = {
    ...coderSource,
    id: "m_unified_coder",
    name: "Unified Coder",
    role: "unified_coder",
    model,
    systemPrompt: "Implement the graph-selected path using only the agent-edited Meta profile definition.",
    tools: ["builtins", "shell", "mob"],
    skills: ["mob.workpad", "mob.editor.unified"],
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    backend: "session",
    maxInlinePeerNotifications: 2,
    providerParams: { thinking_budget_tokens: 2048 },
  };
  const reviewer = {
    ...reviewerSource,
    id: "m_unified_reviewer",
    name: "Unified Reviewer",
    role: "unified_reviewer",
    model,
    systemPrompt: "Review the fallback path and emit the unified verdict schema.",
    tools: ["builtins", "comms", "mob"],
    skills: ["mob.review", "mob.editor.unified"],
    schema: "UnifiedVerdict",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    backend: "session",
    maxInlinePeerNotifications: 1,
  };
  const schemas = [{
    id: "UnifiedVerdict",
    description: "Agent-editor schema used by the synchronized projection proof.",
    fields: [
      {
        id: "f_verdict",
        name: "verdict",
        type: "enum",
        required: true,
        description: "Whether the branch output is accepted.",
        enumValues: ["green", "red"],
      },
      {
        id: "f_notes",
        name: "notes",
        type: "string",
        required: false,
        description: "Reviewer notes.",
      },
    ],
  }];
  const mergedDefinitionSchemas = Array.isArray(reviewerAdd.document.schemas) ? reviewerAdd.document.schemas : [];
  const schemasById = new Map([...mergedDefinitionSchemas, ...schemas].map((schema) => [schema.id, schema]));
  const unifiedSchemas = Array.from(schemasById.values());

  const previousFlow = {
    name: "unified-projection-proof",
    steps: [{
      id: "input_1",
      type: "input",
      task: "Route the synchronized editor projection.",
      fields: "",
      inputParams: [{
        id: "p_route",
        name: "route",
        type: "enum",
        required: true,
        description: "Which graph branch should run.",
        enumValues: ["code", "review"],
      }],
    }, {
      id: "unified_code_turn",
      type: "member",
      role: coder.id,
      instruction: "Run the unified coder path and emit text output.",
    }, {
      id: "unified_review_turn",
      type: "member",
      role: reviewer.id,
      instruction: "Review the fallback path and emit the unified verdict schema.",
    }],
  };
  const instances = [
    { id: "g_branch_unified", isGate: true, gateKind: "branch", label: "branch", col: 0, row: 0 },
    {
      id: "unified_code_turn",
      memberId: coder.id,
      col: 1,
      row: 0,
      lane: "route = code",
      launchMode: { kind: "Fresh" },
      allowedTools: ["builtins", "shell"],
      blockedTools: ["mob"],
      outputFormat: "text",
    },
    {
      id: "unified_review_turn",
      memberId: reviewer.id,
      col: 1,
      row: 1,
      lane: "fallback",
      launchMode: { kind: "Fresh" },
      allowedTools: ["builtins", "comms"],
      outputFormat: "json",
    },
    { id: "j_branch_unified", isGate: true, gateKind: "join", label: "join · branch paths", collection: "any", controllerRole: reviewer.id, col: 2, row: 0 },
  ];
  const edges = [
    { id: "e_unified_code", from: "g_branch_unified", to: "unified_code_turn", kind: "cond", label: "route == \"code\"", cond: { var: "params.route", op: "==", val: "code" } },
    { id: "e_unified_review", from: "g_branch_unified", to: "unified_review_turn", kind: "next", label: "fallback" },
    { id: "e_unified_code_join", from: "unified_code_turn", to: "j_branch_unified", kind: "next", label: "" },
    { id: "e_unified_review_join", from: "unified_review_turn", to: "j_branch_unified", kind: "next", label: "" },
  ];
  const flow = controller.graphToFlow({
    previousFlow,
    members: [coder, reviewer],
    instances,
    edges,
    contract: contractSchema,
  });
  const branch = flow.steps.find((step) => step.type === "branch");
  if (!branch || branch.branches?.length !== 1 || branch.fallback?.length !== 1) {
    throw new Error(`unified projection graph did not compile to a Basic branch: ${JSON.stringify(flow.steps)}`);
  }
  if (branch.controllerRole !== reviewer.id) {
    throw new Error(`unified projection graph dropped branch join member: ${JSON.stringify(branch)}`);
  }

  return controller.buildDocument({
    flow,
    studio: {
      members: [coder, reviewer],
      schemas: unifiedSchemas,
      instances,
      edges,
      frames: [],
      skillRealms,
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "unified-projection-proof" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function validateUnifiedEditorProjection(dir, catalogs) {
  const document = await buildUnifiedProjectionDocument(catalogs);
  const validation = await rpc("meta/mobpacks/validate", { document });
  if (!validation.ok) {
    throw new Error(`unified editor projection failed Meta validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document,
    filename: "unified-editor-projection.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`unified editor projection export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const mobToml = exported.mob_toml || "";
  for (const required of [
    "[profiles.unified_coder]",
    "[profiles.unified_reviewer]",
    "skills = [\"mob.workpad\", \"mob.editor.unified\"]",
    "skills = [\"mob.review\", \"mob.editor.unified\"]",
    "[profiles.unified_reviewer.output_schema]",
    "[skills.\"mob.editor.unified\"]",
    "[flows.main.root.nodes.node_02_unified_coder]",
    "[flows.main.root.nodes.node_05_unified_reviewer]",
    "Join branch paths.",
    "branch = \"branch_unified\"",
  ]) {
    if (!mobToml.includes(required)) {
      throw new Error(`unified editor mob.toml missing ${required}\n${mobToml}`);
    }
  }
  if (mobToml.includes("sourceDefinition") || mobToml.includes("sourceMobpack")) {
    throw new Error(`unified editor mob.toml leaked editor-only source-definition provenance:\n${mobToml}`);
  }
  const packPath = path.join(dir, exported.filename || "unified-editor-projection.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const importedValidation = await rpc("meta/mobpacks/validate", { document: imported.document });
  if (!importedValidation.ok) {
    throw new Error(`imported unified editor projection failed validation: ${JSON.stringify(importedValidation.diagnostics)}`);
  }
  const branch = imported.document?.flow?.steps?.find((step) => step.type === "branch");
  if (!branch || branch.branches?.[0]?.steps?.[0]?.role !== "m_unified_coder" || branch.fallback?.[0]?.role !== "m_unified_reviewer") {
    throw new Error(`imported unified editor projection lost graph/basic branch sync: ${JSON.stringify(branch)}`);
  }
  const coder = imported.document?.members?.find((member) => member.id === "m_unified_coder");
  const reviewer = imported.document?.members?.find((member) => member.id === "m_unified_reviewer");
  if (!coder || !reviewer) throw new Error("imported unified editor projection lost edited members");
  if (!coder.tools?.includes("shell") || !coder.skills?.includes("mob.editor.unified") || coder.providerParams?.thinking_budget_tokens !== 2048) {
    throw new Error(`imported unified coder lost agent-editor fields: ${JSON.stringify(coder)}`);
  }
  if (reviewer.schema !== "UnifiedVerdict" || !reviewer.skills?.includes("mob.review")) {
    throw new Error(`imported unified reviewer lost schema/skills: ${JSON.stringify(reviewer)}`);
  }
  for (const [label, member] of [["coder", coder], ["reviewer", reviewer]]) {
    const source = member.sourceDefinition || {};
    const sourceKind = String(source.sourceKind || source.definitionKind || "").trim();
    const sourceOrigin = String(source.sourceOrigin || "").trim();
    const realCatalogSource = sourceKind === "authoring"
      || sourceKind === "sample"
      || sourceOrigin === "meta/authoring-agent-definitions"
      || sourceOrigin === "meta/sample-mobpack";
    if (source.definitionType !== "meta/profile-member" || !source.definitionId || !source.sourceMobpack || !realCatalogSource) {
      throw new Error(`imported unified ${label} lost source-definition provenance from the real profile-member catalog: ${JSON.stringify(source)}`);
    }
    if (!String(source.sourceDocumentPath || "").startsWith("document.members[")) {
      throw new Error(`imported unified ${label} sourceDefinition must carry indexed member source path: ${JSON.stringify(source)}`);
    }
  }
  const frameKinds = (imported.document.frames || []).map((frame) => frame.kind);
  if (!frameKinds.includes("Branch")) {
    throw new Error(`imported unified editor projection lost Branch frame: ${JSON.stringify(imported.document.frames)}`);
  }
  return {
    validate,
    members: [coder.id, reviewer.id],
    branch: {
      branches: branch.branches.length,
      fallback: branch.fallback.length,
      cond: branch.branches[0].cond,
    },
    frameKinds,
    schemaIds: (imported.document.schemas || []).map((candidate) => candidate.id),
    skillIds: (imported.document.skill_realms || []).flatMap((realm) => (realm.skills || []).map((skill) => skill.id)),
    sourceDefinitions: [coder.sourceDefinition.sourceMobpack, reviewer.sourceDefinition.sourceMobpack],
  };
}

function buildRealmProfileDefinitionDocument() {
  const members = [{
    id: "m_realm_quality",
    name: "realm_quality",
    role: "realm_quality",
    profileBinding: "realm_profile",
    realmProfile: "quality-reviewer-v2",
    model: "",
    systemPrompt: "Realm profile reference: quality-reviewer-v2",
    tools: [],
    skills: [],
    runtimeMode: "turn_driven",
  }];
  const flow = {
    name: "realm-profile-definition",
    steps: [
      {
        id: "input_1",
        type: "input",
        task: "Run the realm profile definition.",
        fields: "",
        inputParams: [],
      },
      {
        id: "realm_turn",
        type: "member",
        role: "m_realm_quality",
        instruction: "Run the realm-backed quality reviewer.",
        launchMode: { kind: "Fresh" },
      },
    ],
  };
  return controller.buildDocument({
    flow,
    studio: {
      members,
      schemas: [],
      instances: [],
      edges: [],
      frames: [],
      skillRealms: [],
      mobSettings: testMobSettings(),
    },
    currentFlow: { name: "realm-profile-definition" },
    deploySettings: testDeploySettings(),
    contract: contractSchema,
  });
}

async function rejectRealmProfileDefinition() {
  const document = buildRealmProfileDefinitionDocument();
  const validation = await rpc("meta/mobpacks/validate", { document });
  const diagnostic = validation.diagnostics?.find((candidate) => candidate.code === "unsupported_realm_profile_pack_binding");
  if (validation.ok || !diagnostic) {
    throw new Error(`realm profile definition should fail before export: ${JSON.stringify(validation)}`);
  }
  return {
    ok: validation.ok,
    code: diagnostic.code,
    path: diagnostic.path,
  };
}

async function validateBlankMobpackTemplate(dir, catalogs) {
  const blankTemplate = controller.blankMobpackFromCatalogs(catalogs);
  if (!blankTemplate?.document) {
    throw new Error(`meta/mobpacks/catalogs did not provide a blank mobpack template: ${JSON.stringify(catalogs.blank_mobpack)}`);
  }
  if (blankTemplate.validation?.ok !== true) {
    throw new Error(`blank mobpack template is not API-valid: ${JSON.stringify(blankTemplate.validation)}`);
  }
  const created = await rpc("meta/mobpacks/create", {
    id: "f_blank_live",
    name: "Blank Live Proof",
    trigger: "label · blank-live-proof",
    template: "blank",
  });
  const draft = {
    document: created?.row?.document,
    row: created?.row,
  };
  if (!draft?.document || draft.row?.source !== "meta/blank-mobpack") {
    throw new Error(`meta/mobpacks/create did not return a blank Meta draft: ${JSON.stringify(created)}`);
  }
  const validation = await rpc("meta/mobpacks/validate", { document: draft.document });
  if (!validation.ok) {
    throw new Error(`created blank draft failed validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  const exported = await rpc("meta/mobpacks/export", {
    document: draft.document,
    filename: "blank-live-proof.mobpack",
  });
  if (!exported.validation?.ok) {
    throw new Error(`created blank draft export failed validation: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const packPath = path.join(dir, exported.filename || "blank-live-proof.mobpack");
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  const rkatValidate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const member = imported.document?.members?.find((candidate) => candidate.id === "m_worker");
  if (!member || member.profileBinding !== "inline" || member.runtimeMode !== "turn_driven") {
    throw new Error(`imported blank draft lost real worker profile definition: ${JSON.stringify(imported.document?.members)}`);
  }
  if (!imported.document?.flow?.steps?.some((step) => step.type === "member")) {
    throw new Error(`imported blank draft lost member turn: ${JSON.stringify(imported.document?.flow)}`);
  }
  return {
    validate: rkatValidate,
    source: draft.row.source,
    member: member.id,
    flowSteps: imported.document.flow.steps.length,
  };
}

async function validateCustomDeploySettings(dir) {
  const document = buildEditedAgentDefinitionDocument();
  document.name = "custom-deploy-settings";
  document.mob_id = "custom_deploy_settings";
  document.deploy = controller.normalizeDeploySettings({
    ...testDeploySettings(),
    surface: "cli",
    trustPolicy: "strict",
    model: "gpt-5.5",
    maxDuration: "45s",
    maxToolCalls: 3,
    maxTotalTokens: 128,
    isolated: false,
    realm: "editor-proof-realm",
    instance: "editor-proof-instance",
    realmBackend: "sqlite",
    contextRoot: path.join(dir, "context root"),
    stateRoot: path.join(dir, "state-root"),
    userConfigRoot: path.join(dir, "config-root"),
    prompt: "Custom deploy proof prompt.",
  });
  const packPath = path.join(dir, "custom-deploy-settings.mobpack");
  const preview = await rpc("meta/mobpacks/deploy_command", {
    document,
    pack_path: packPath,
    prompt: "Custom deploy proof prompt.",
  });
  const result = await rpc("meta/mobpacks/deploy", {
    document,
    execute: false,
    pack_path: packPath,
  });
  if (!result.validation?.ok) {
    throw new Error(`custom deploy settings failed validation: ${JSON.stringify(result.validation?.diagnostics)}`);
  }
  if (result.executed) throw new Error("custom deploy settings proof unexpectedly executed deploy");
  if (result.success) throw new Error("custom deploy settings proof reported success without executing deploy");
  if (fs.existsSync(packPath)) {
    throw new Error(`deploy planning wrote host mobpack without execute=true: ${packPath}`);
  }
  const argv = result.argv || [];
  if (preview.command !== result.command || JSON.stringify(preview.argv || []) !== JSON.stringify(argv)) {
    throw new Error(`deploy command preview drifted from deploy plan\npreview=${JSON.stringify(preview)}\nresult=${JSON.stringify({ command: result.command, argv })}`);
  }
  if (preview.source !== "meerkat_meta::mobpack::deploy_argv") {
    throw new Error(`deploy command preview did not report Meta deploy_argv source: ${JSON.stringify(preview)}`);
  }
  if (!preview.validation?.ok || preview.filename !== "custom-deploy-settings.mobpack") {
    throw new Error(`deploy command preview was not document-backed: ${JSON.stringify(preview)}`);
  }
  // `rkat mob run` executes pack policy directly: model/token/duration/tool
  // caps and surface are not CLI overrides; the prompt and realm scope are.
  const expectedPairs = [

    ["--trust-policy", "strict"],
    ["--realm", "editor-proof-realm"],
    ["--instance", "editor-proof-instance"],
    ["--realm-backend", "sqlite"],
    ["--context-root", path.join(dir, "context root")],
    ["--state-root", path.join(dir, "state-root")],
    ["--user-config-root", path.join(dir, "config-root")],
  ];
  // The controller emits the single-token hyphen-safe form (a prompt
  // beginning with `-` must not parse as a flag).
  if (!argv.includes("--prompt=Custom deploy proof prompt.")) {
    throw new Error(`custom deploy argv missing --prompt=<text> single token: ${JSON.stringify(argv)}`);
  }
  for (const [flag, value] of expectedPairs) {
    const index = argv.indexOf(flag);
    if (index < 0 || argv[index + 1] !== value) {
      throw new Error(`custom deploy argv missing ${flag} ${value}: ${JSON.stringify(argv)}`);
    }
  }
  if (argv.includes("--isolated")) {
    throw new Error(`custom deploy argv should not include --isolated when realm is set: ${JSON.stringify(argv)}`);
  }
  if (argv[1] !== "mob" || argv[2] !== "run" || argv[3] !== packPath) {
    throw new Error(`custom deploy argv must invoke rkat mob run on the planned pack: ${JSON.stringify(argv)}`);
  }
  for (const legacyFlag of ["--model", "--max-total-tokens", "--max-duration", "--max-tool-calls", "--surface"]) {
    if (argv.includes(legacyFlag)) {
      throw new Error(`custom deploy argv leaked legacy rkat mob deploy override ${legacyFlag}: ${JSON.stringify(argv)}`);
    }
  }
  if (!array(result.display_rows, "deploy.display_rows").some((row) => row.kind === "warn" && row.head === "Deploy plan ready" && row.sub.includes("rkat mob run"))) {
    throw new Error(`Meta deploy response did not provide API-backed display rows: ${JSON.stringify(result.display_rows)}`);
  }
  const planTrace = assertDeployPlanTrace(result, "customDeploySettings");
  const exported = await rpc("meta/mobpacks/export", { document });
  const packBytes = Buffer.from(exported.content_base64, "base64");
  if (result.pack_sha256 !== sha256(packBytes)) {
    throw new Error(`Meta deploy response did not report the planned pack sha256: ${JSON.stringify({ pack_sha256: result.pack_sha256, pack_path: result.pack_path })}`);
  }
  fs.writeFileSync(result.pack_path, packBytes);
  // editor-authored packs are unsigned; rkat 0.7 `mob validate` defaults to
  // strict trust, so validate the integrity check under permissive policy.
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", result.pack_path]);
  return {
    validate,
    command: result.command,
    previewCommand: preview.command,
    argv,
    executed: result.executed,
    success: result.success,
    packPath: result.pack_path,
    packSha256: result.pack_sha256,
    planTrace,
  };
}

async function validateDocumentBackedDeployPreview(document) {
  const preview = await rpc("meta/mobpacks/deploy_command", { document });
  const argv = array(preview.argv, "documentBackedDeployPreview.argv");
  const expectedPackName = `${String(document?.name || document?.mob_id || "mobpack")
    .trim()
    .replace(/\.mobpack$/i, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "") || "mobpack"}.mobpack`;
  // rkat mob run argv shape: [rkat, mob, run, <pack>, --prompt, <prompt>, ...]
  if (argv[3] !== expectedPackName) {
    throw new Error(`document-backed deploy preview did not derive pack filename ${expectedPackName}: ${JSON.stringify(preview)}`);
  }
  if (preview.command.includes("<pack.mobpack>") || preview.command.includes("<prompt>")) {
    throw new Error(`document-backed deploy preview leaked placeholder copy: ${JSON.stringify(preview)}`);
  }
  if (preview.deploy_command !== "rkat mob run") {
    throw new Error(`document-backed deploy preview used the wrong deploy command: ${JSON.stringify(preview)}`);
  }
  if (!preview.validation?.ok || preview.filename !== expectedPackName) {
    throw new Error(`document-backed deploy preview did not validate/render source metadata: ${JSON.stringify(preview)}`);
  }
  return {
    command: preview.command,
    argvTail: argv.slice(3, 6),
    source: preview.source,
  };
}

async function validateHostDeployRpc(document, dir) {
  const deployDocument = JSON.parse(JSON.stringify(document));
  deployDocument.deploy = controller.normalizeDeploySettings({
    ...testDeploySettings(),
    surface: "cli",
    trustPolicy: "permissive",
    maxDuration: "30s",
    maxToolCalls: 0,
    maxTotalTokens: 64,
    isolated: true,
    realmBackend: "jsonl",
    contextRoot: path.join(dir, "host-context"),
    stateRoot: path.join(dir, "host-state"),
    userConfigRoot: path.join(dir, "host-config"),
    prompt: "Reply with exactly OK.",
  });
  const result = await rpc("meta/mobpacks/deploy", {
    document: deployDocument,
    execute: true,
    pack_path: path.join(dir, "host-deploy-rpc.mobpack"),
  });
  if (!result.executed || !result.success || result.status_code !== 0) {
    throw new Error(`host deploy RPC did not execute successfully: ${JSON.stringify(result)}`);
  }
  const output = [result.stdout || "", result.stderr || ""].join("\n");
  if (!/^run\tmob=.*\tstatus=completed/m.test(output)) {
    throw new Error(`host deploy RPC output did not include the rkat mob run completion envelope: ${output}`);
  }
  if (!array(result.display_rows, "hostDeployRpc.display_rows").some((row) => row.kind === "ok" && row.head === "rkat mob run executed")) {
    throw new Error(`host deploy RPC did not report executed display row: ${JSON.stringify(result.display_rows)}`);
  }
  return {
    executed: result.executed,
    success: result.success,
    statusCode: result.status_code,
    stdout: result.stdout,
    command: result.command,
    packPath: result.pack_path,
  };
}

async function validateNamedTypedOperations(catalogs) {
  let document = catalogs.blank_mobpack.document;
  const catalogSnapshotId = catalogs.catalog_snapshot?.id;
  if (!catalogSnapshotId) {
    throw new Error(`named operation proof needs a Meta catalog snapshot id: ${JSON.stringify(catalogs.catalog_snapshot)}`);
  }
  const implementerDefinition = array(catalogs.agent_definitions, "catalogs.agent_definitions")
    .find((definition) => definition.role === "implementer" && definition.sourceOrigin === "meta/authoring-agent-definitions");
  if (!implementerDefinition?.id) {
    throw new Error(`Meta catalogs did not expose an implementer authoring profile definition: ${JSON.stringify(catalogs.agent_definitions)}`);
  }
  try {
    await rpc("meta/mobpacks/apply_operation", {
      document,
      expected_catalog_snapshot_id: "stale-catalog-snapshot",
      operation: {
        type: "add_agent_definition",
        definition_id: implementerDefinition.id,
      },
    });
    throw new Error("stale catalog snapshot unexpectedly accepted add_agent_definition");
  } catch (error) {
    if (!String(error?.message || error).includes("catalog snapshot conflict")) {
      throw error;
    }
  }
  const added = await rpc("meta/mobpacks/apply_operation", {
    document,
    expected_catalog_snapshot_id: catalogSnapshotId,
    operation: {
      type: "add_agent_definition",
      definition_id: implementerDefinition.id,
    },
  });
  if (!added.ok) {
    throw new Error(`named operation proof could not add agent definition: ${JSON.stringify(added.validation)}`);
  }
  document = added.document;
  const memberId = added.selection.id;
  const updated = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "update_member",
      member_id: memberId,
      patch: { name: "Named typed operation agent" },
      selection: { kind: "agent", id: memberId },
    },
  });
  const updatedMember = updated.document.members.find((member) => member.id === memberId);
  if (!updated.ok || updatedMember?.name !== "Named typed operation agent") {
    throw new Error(`named typed operation did not apply member patch: ${JSON.stringify(updated)}`);
  }
  const blankDefinition = array(catalogs.agent_definitions, "catalogs.agent_definitions")
    .find((definition) => definition.role === "agent" && definition.sourceOrigin === "meta/authoring-agent-definitions");
  if (!blankDefinition?.id) {
    throw new Error(`Meta catalogs did not expose a blank authoring profile definition: ${JSON.stringify(catalogs.agent_definitions)}`);
  }
  const blankAdded = await rpc("meta/mobpacks/apply_operation", {
    document: updated.document,
    expected_catalog_snapshot_id: catalogSnapshotId,
    operation: {
      type: "add_agent_definition",
      definition_id: blankDefinition.id,
    },
  });
  const blankMemberId = blankAdded.selection?.id;
  const blankMember = blankAdded.document.members.find((member) => member.id === blankMemberId);
  if (!blankAdded.ok || blankMember?.role !== "agent" || blankMember?.sourceDefinition?.definitionId !== blankDefinition.id) {
    throw new Error(`blank authoring profile definition did not materialize as an editable member: ${JSON.stringify(blankAdded)}`);
  }
  const settings = await rpc("meta/mobpacks/apply_operation", {
    document: blankAdded.document,
    operation: {
      type: "update_deploy_settings",
      field: "prompt",
      value: "Named section payload prompt.",
    },
  });
  if (!settings.ok || settings.document.deploy.prompt !== "Named section payload prompt.") {
    throw new Error(`named deploy settings operation did not apply field payload: ${JSON.stringify(settings)}`);
  }
  const invalidSettings = await rpcExpectError("meta/mobpacks/apply_operation", {
    document: settings.document,
    operation: {
      type: "update_deploy_settings",
      field: "surface",
      value: "desktop",
    },
  });
  if (!String(invalidSettings.message || "").includes("update_deploy_settings surface must be one of cli, rpc")) {
    throw new Error(`invalid deploy settings were not rejected at the mutation boundary: ${JSON.stringify(invalidSettings)}`);
  }
  const mobSettings = await rpc("meta/mobpacks/apply_operation", {
    document: settings.document,
    operation: {
      type: "update_mob_settings",
      field: "backendDefault",
      value: "session",
    },
  });
  if (!mobSettings.ok || mobSettings.document.mob_settings?.backendDefault !== "session") {
    throw new Error(`named mob settings operation did not apply field payload: ${JSON.stringify(mobSettings)}`);
  }
  const roleWiring = await rpc("meta/mobpacks/apply_operation", {
    document: mobSettings.document,
    operation: {
      type: "update_role_wiring",
      action: "add",
    },
    rkat_validate: true,
  });
  if (!roleWiring.ok || !Array.isArray(roleWiring.document.mob_settings?.roleWiring) || roleWiring.document.mob_settings.roleWiring.length < 1) {
    throw new Error(`named role wiring operation did not apply action payload: ${JSON.stringify(roleWiring)}`);
  }
  if (roleWiring.validation?.validation_source !== "rkat mob validate") {
    throw new Error(`named role wiring operation did not run rkat validation: ${JSON.stringify(roleWiring.validation)}`);
  }
  const validation = await rpc("meta/mobpacks/validate", { document: roleWiring.document });
  if (!validation.ok) {
    throw new Error(`named projected operation document failed validation: ${JSON.stringify(validation.diagnostics)}`);
  }
  return {
    memberId,
    blankMemberId,
    name: roleWiring.document.name,
    prompt: roleWiring.document.deploy.prompt,
    operationDocumentApplied: updated.operation === "update_flow_step",
    sectionPayloadApplied: settings.operation === "update_deploy_settings",
    mobFieldApplied: mobSettings.operation === "update_mob_settings",
    roleActionApplied: roleWiring.operation === "update_role_wiring",
    roleActionValidationSource: roleWiring.validation?.validation_source,
    catalogSnapshotGuarded: true,
  };
}

async function validateInputParamOperations(catalogs) {
  const document = JSON.parse(JSON.stringify(catalogs.blank_mobpack.document));
  document.members = [{
    id: "m_worker",
    name: "worker",
    role: "worker",
    profileBinding: "inline",
    runtimeMode: "turn_driven",
    model: "gpt-5.5",
    tools: [],
    skills: [],
  }];
  document.flow = {
    id: "main",
    steps: [
      {
        type: "input",
        id: "input",
        task: "Route the work.",
        inputParams: [{ id: "p1", name: "route", type: "enum", required: true, enumValues: ["code", "docs"] }],
        fields: "route: enum",
      },
      {
        type: "branch",
        id: "branch_route",
        branches: [{
          cond: { namespace: "params", stepId: "params", field: "route", op: "==", val: "code" },
          condition: "params.route == code",
          steps: [{ type: "member", id: "worker_step", role: "m_worker" }],
        }],
      },
    ],
  };
  document.instances = [
    { id: "input", kind: "source" },
    { id: "worker", kind: "member", memberId: "m_worker" },
  ];
  document.edges = [{
    id: "edge_route",
    from: "input",
    to: "worker",
    kind: "cond",
    cond: { var: "params.route", op: "==", val: "code" },
    label: "params.route == code",
  }];

  const added = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "add_input_param",
      step_id: "input",
    },
  });
  if (added.selection?.param_id !== "p2" || added.document.flow.steps[0].inputParams[1]?.name !== "param") {
    throw new Error(`input param add operation did not use Meta draft defaults: ${JSON.stringify(added.document.flow.steps[0])}`);
  }
  const renamed = await rpc("meta/mobpacks/apply_operation", {
    document: added.document,
    operation: {
      type: "rename_input_param",
      step_id: "input",
      param_id: "p1",
      new_name: "kind",
    },
  });
  if (renamed.document.flow.steps[1].branches[0].cond.field !== "kind" || renamed.document.edges[0].cond.var !== "params.kind") {
    throw new Error(`input param rename operation did not rewrite references: ${JSON.stringify(renamed.document)}`);
  }
  const deleted = await rpc("meta/mobpacks/apply_operation", {
    document: renamed.document,
    operation: {
      type: "delete_input_param",
      step_id: "input",
      param_id: "p1",
    },
  });
  if (deleted.document.edges[0].cond !== null || Object.keys(deleted.document.flow.steps[1].branches[0].cond || {}).length !== 0) {
    throw new Error(`input param delete operation did not clear references: ${JSON.stringify(deleted.document)}`);
  }
  return {
    addedFields: added.document.flow.steps[0].fields,
    renamedField: renamed.document.flow.steps[1].branches[0].cond.field,
    deletedEdgeCond: deleted.document.edges[0].cond,
    operations: [added.operation, renamed.operation, deleted.operation],
  };
}

async function validateSchemaOperations(catalogs) {
  const document = JSON.parse(JSON.stringify(catalogs.blank_mobpack.document));
  document.schemas = [{
    id: "Review",
    description: "Review result",
    fields: [{ id: "f1", name: "verdict", type: "string", required: true, description: "", enumValues: [] }],
  }];

  const addedSchema = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "add_schema",
    },
  });
  if (addedSchema.selection?.id !== "Artifact1" || addedSchema.document.schemas[1]?.fields?.[0]?.name !== "field_one") {
    throw new Error(`schema add operation did not use Meta draft defaults: ${JSON.stringify(addedSchema.document.schemas)}`);
  }

  const addedField = await rpc("meta/mobpacks/apply_operation", {
    document: addedSchema.document,
    operation: {
      type: "add_schema_field",
      schema_id: "Review",
    },
  });
  const field = addedField.document.schemas[0]?.fields?.[1];
  if (addedField.selection?.field_id !== "f2" || field?.name !== "new_field") {
    throw new Error(`schema field add operation did not use Meta draft defaults: ${JSON.stringify(addedField.document.schemas[0])}`);
  }

  return {
    schemaId: addedSchema.selection.id,
    initialField: addedSchema.document.schemas[1].fields[0].name,
    addedField: field.name,
    operations: [addedSchema.operation, addedField.operation],
  };
}

async function validateGraphOperations(catalogs) {
  const document = JSON.parse(JSON.stringify(catalogs.blank_mobpack.document));
  document.members = [
    {
      id: "planner",
      name: "planner",
      role: "planner",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      model: "gpt-5.5",
      tools: [],
      skills: [],
    },
    {
      id: "reviewer",
      name: "reviewer",
      role: "reviewer",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      model: "gpt-5.5",
      tools: [],
      skills: [],
    },
  ];
  document.instances = [
    { id: "n_plan", kind: "member", memberId: "planner", col: 0, row: 0 },
    { id: "n_review", kind: "member", memberId: "reviewer", col: 1, row: 0 },
  ];
  document.edges = [];
  const semanticMember = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "insert_graph_node",
      pick: { kind: "memberInstance", memberId: "planner" },
      cell: { col: 3, row: 4 },
    },
  });
  if (semanticMember.selection?.id !== "i_planner" || semanticMember.document.instances[2]?.launchMode?.kind !== "Fresh") {
    throw new Error(`semantic graph member insert did not use Meta defaults: ${JSON.stringify(semanticMember.document.instances)}`);
  }
  const semanticBranch = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "insert_graph_node",
      pick: { kind: "gate", gateKind: "branch" },
      cell: { col: 0, row: 0 },
    },
  });
  if (semanticBranch.selection?.id !== "g_branch_1" || semanticBranch.document.edges[1]?.label !== "fallback") {
    throw new Error(`semantic graph branch insert did not use Meta graph draft: ${JSON.stringify(semanticBranch.document)}`);
  }
  try {
    await rpc("meta/mobpacks/apply_operation", {
      document,
      operation: {
        type: "insert_graph_node",
        instance: { id: "n_terminal", kind: "terminal", isTerminal: true, col: 2, row: 0 },
      },
    });
    throw new Error("raw graph node insert was accepted");
  } catch (error) {
    if (!String(error?.message || "").includes("insert_graph_node does not accept raw instance")) {
      throw error;
    }
  }
  const inserted = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "insert_graph_node",
      pick: { kind: "memberInstance", memberId: "reviewer" },
      cell: { col: 2, row: 0 },
    },
  });
  const insertedId = inserted.selection?.id;
  if (!insertedId) {
    throw new Error(`semantic graph insert did not return a selected instance: ${JSON.stringify(inserted)}`);
  }
  const moved = await rpc("meta/mobpacks/apply_operation", {
    document: inserted.document,
    operation: {
      type: "move_graph_node",
      instance_id: insertedId,
      cell: { col: 1, row: 0 },
      original_cell: { col: 2, row: 0 },
    },
  });
  const doneAfterMove = moved.document.instances.find((instance) => instance.id === insertedId);
  const reviewerAfterMove = moved.document.instances.find((instance) => instance.id === "n_review");
  if (doneAfterMove?.col !== 1 || reviewerAfterMove?.col !== 2) {
    throw new Error(`graph move operation did not swap cells: ${JSON.stringify(moved.document.instances)}`);
  }
  const updated = await rpc("meta/mobpacks/apply_operation", {
    document: moved.document,
    operation: {
      type: "apply_graph_node_edit",
      instance_id: insertedId,
      action: "set_label",
      label: "Review lane",
    },
  });
  try {
    await rpc("meta/mobpacks/apply_operation", {
      document: updated.document,
      operation: {
        type: "insert_graph_node",
        pick: { kind: "terminal", terminalKind: "success" },
        cell: { col: 3, row: 0 },
      },
    });
    throw new Error("terminal graph node insert was accepted");
  } catch (error) {
    if (!String(error?.message || "").includes("unsupported insert_graph_node pick.kind: terminal")) {
      throw error;
    }
  }
  const legacyTerminalDocument = {
    ...updated.document,
    instances: [
      { id: "n_plan", kind: "member", memberId: "planner", col: 0, row: 0 },
      { id: "n_done", kind: "terminal", isTerminal: true, col: 1, row: 0 },
    ],
    edges: [],
  };
  try {
    await rpc("meta/mobpacks/apply_operation", {
      document: legacyTerminalDocument,
      operation: {
        type: "connect_graph_nodes",
        from_id: "n_plan",
        to_id: "n_done",
      },
    });
    throw new Error("terminal graph endpoint connect was accepted");
  } catch (error) {
    if (!String(error?.message || "").includes("edge endpoints cannot reference uncompiled graph terminal nodes")) {
      throw error;
    }
  }
  const connected = await rpc("meta/mobpacks/apply_operation", {
    document: updated.document,
    operation: {
      type: "connect_graph_nodes",
      from_id: "n_plan",
      to_id: insertedId,
    },
  });
  const expectedEdgeId = `e_n_plan_${insertedId}`;
  if (connected.selection?.id !== expectedEdgeId || connected.document.edges[0]?.kind !== "next") {
    throw new Error(`semantic graph connect operation did not draft expected edge: ${JSON.stringify(connected.document.edges)}`);
  }
  const edgeUpdated = await rpc("meta/mobpacks/apply_operation", {
    document: connected.document,
    operation: {
      type: "apply_graph_edge_edit",
      edge_id: expectedEdgeId,
      action: "set_label",
      label: "done",
    },
  });
  if (edgeUpdated.document.edges[0]?.label !== "done") {
    throw new Error(`semantic graph edge edit did not apply label: ${JSON.stringify(edgeUpdated.document.edges)}`);
  }
  const edgeDeleted = await rpc("meta/mobpacks/apply_operation", {
    document: edgeUpdated.document,
    operation: {
      type: "delete_graph_edge",
      edge_id: expectedEdgeId,
    },
  });
  if (edgeDeleted.document.edges.length !== 0) {
    throw new Error(`graph edge delete operation did not remove edge: ${JSON.stringify(edgeDeleted.document.edges)}`);
  }
  const reconnected = await rpc("meta/mobpacks/apply_operation", {
    document: edgeDeleted.document,
    operation: {
      type: "connect_graph_nodes",
      from_id: insertedId,
      to_id: "n_plan",
    },
  });
  const deleted = await rpc("meta/mobpacks/apply_operation", {
    document: reconnected.document,
    operation: {
      type: "delete_graph_node",
      instance_id: insertedId,
    },
  });
  if (deleted.document.instances.some((instance) => instance.id === insertedId) || deleted.document.edges.length !== 0) {
    throw new Error(`graph node delete operation did not prune node/edges: ${JSON.stringify(deleted.document)}`);
  }
  return {
    inserted: insertedId,
    semanticMember: semanticMember.selection.id,
    semanticBranch: semanticBranch.selection.id,
    moved: { done: doneAfterMove.col, reviewer: reviewerAfterMove.col },
    nodeLabel: updated.document.instances.find((instance) => instance.id === insertedId)?.label,
    edgeLabel: edgeUpdated.document.edges[0]?.label,
    remainingInstances: deleted.document.instances.map((instance) => instance.id),
    operations: [
      semanticMember.operation,
      semanticBranch.operation,
      inserted.operation,
      moved.operation,
      updated.operation,
      connected.operation,
      edgeUpdated.operation,
      edgeDeleted.operation,
      deleted.operation,
    ],
  };
}

async function validateFlowStepOperations(catalogs) {
  const document = JSON.parse(JSON.stringify(catalogs.blank_mobpack.document));
  document.members = [
    {
      id: "planner",
      name: "planner",
      role: "planner",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      model: "gpt-5.5",
      tools: [],
      skills: [],
    },
    {
      id: "reviewer",
      name: "reviewer",
      role: "reviewer",
      profileBinding: "inline",
      runtimeMode: "turn_driven",
      model: "gpt-5.5",
      tools: [],
      skills: [],
    },
  ];
  document.flow = {
    id: "main",
    steps: [
      { type: "member", id: "plan", role: "planner", instruction: "Plan." },
      {
        type: "branch",
        id: "route",
        branches: [{
          id: "approved",
          cond: { stepId: "s_1", field: "verdict", op: "==", val: "green" },
          condition: "steps.s_1.verdict == green",
          steps: [],
        }],
        fallback: [],
      },
    ],
  };
  const inserted = await rpc("meta/mobpacks/apply_operation", {
    document,
    operation: {
      type: "insert_flow_step",
      lane_ref: { lane: "main", index: 1 },
      pick: { kind: "member", id: "reviewer" },
    },
  });
  const semanticStepId = inserted.selection?.id;
  if (!semanticStepId || inserted.document.flow.steps[1]?.role !== "reviewer") {
    throw new Error(`semantic flow step insert did not return a real reviewer step: ${JSON.stringify(inserted.document.flow.steps)}`);
  }
  const authoredReview = await rpc("meta/mobpacks/apply_operation", {
    document: inserted.document,
    operation: {
      type: "apply_flow_step_edit",
      step_id: semanticStepId,
      action: "set_instruction",
      value: "Review.",
    },
  });
  const nested = await rpc("meta/mobpacks/apply_operation", {
    document: authoredReview.document,
    operation: {
      type: "insert_flow_step",
      lane_ref: { parentId: "route", branchId: "approved", index: 0 },
      step: { type: "member", id: "approve", role: "planner", instruction: "Approve." },
    },
  });
  if (nested.document.flow.steps[2]?.branches?.[0]?.steps?.[0]?.id !== "approve") {
    throw new Error(`nested flow step insert did not mutate branch lane: ${JSON.stringify(nested.document.flow)}`);
  }
  const updated = await rpc("meta/mobpacks/apply_operation", {
    document: nested.document,
    operation: {
      type: "apply_flow_step_edit",
      step_id: semanticStepId,
      action: "set_instruction",
      value: "Review carefully.",
    },
  });
  if (updated.document.flow.steps[1]?.instruction !== "Review carefully.") {
    throw new Error(`semantic flow step edit did not apply instruction: ${JSON.stringify(updated.document.flow.steps)}`);
  }
  const branchCondition = await rpc("meta/mobpacks/apply_operation", {
    document: updated.document,
    operation: {
      type: "apply_flow_step_edit",
      step_id: "route",
      action: "set_branch_condition",
      branch_id: "approved",
      patch: { namespace: "steps", stepId: semanticStepId, field: "verdict", op: "==", val: "blue" },
    },
  });
  const routeStep = branchCondition.document.flow.steps.find((step) => step.id === "route");
  if (routeStep?.branches?.[0]?.cond?.stepId !== semanticStepId || routeStep.branches[0].condition !== `steps.${semanticStepId}.verdict == "blue"`) {
    throw new Error(`semantic branch condition edit did not mutate branch condition: ${JSON.stringify(routeStep)}`);
  }
  const branchAdded = await rpc("meta/mobpacks/apply_operation", {
    document: branchCondition.document,
    operation: {
      type: "apply_flow_step_edit",
      step_id: "route",
      action: "add_branch",
    },
  });
  const branchAddedRoute = branchAdded.document.flow.steps.find((step) => step.id === "route");
  if (branchAddedRoute?.branches?.length !== 2 || branchAddedRoute.branches[1]?.id !== "br_1" || branchAddedRoute.branches[1]?.label !== "Branch 2") {
    throw new Error(`semantic branch add did not append a real Meta lane: ${JSON.stringify(branchAddedRoute)}`);
  }
  const deleted = await rpc("meta/mobpacks/apply_operation", {
    document: branchAdded.document,
    operation: {
      type: "delete_flow_step",
      step_id: semanticStepId,
    },
  });
  if (deleted.document.flow.steps.some((step) => step.id === semanticStepId)) {
    throw new Error(`flow step delete did not remove step: ${JSON.stringify(deleted.document.flow.steps)}`);
  }
  if (Object.keys(deleted.document.flow.steps[1]?.branches?.[0]?.cond || {}).length !== 0 || deleted.document.flow.steps[1]?.branches?.[0]?.condition !== "") {
    throw new Error(`flow step delete did not clear deleted-step conditions: ${JSON.stringify(deleted.document.flow.steps[1])}`);
  }
  return {
    inserted: inserted.selection.id,
    nested: nested.document.flow.steps[2].branches[0].steps[0].id,
    updatedInstruction: updated.document.flow.steps[1].instruction,
    branchCondition: branchCondition.document.flow.steps.find((step) => step.id === "route").branches[0].condition,
    branchCount: branchAddedRoute.branches.length,
    remainingSteps: deleted.document.flow.steps.map((step) => step.id),
    operations: [inserted.operation, authoredReview.operation, nested.operation, updated.operation, branchCondition.operation, branchAdded.operation, deleted.operation],
  };
}

(async () => {
  run("rkat", ["mob", "--help"]);

  const authoringCapabilities = await assertAuthoringCapabilities();
  const schema = await rpc("meta/mobpacks/schema", {});
  const catalogs = await rpc("meta/mobpacks/catalogs", {});
  const toolsCatalog = await rpc("meta/tools/catalog", {});
  const skillsCatalog = await rpc("meta/skills/catalog", {});
  const agentDefinitions = await rpc("meta/agent_definitions/list", {});
  const templatesCatalog = await rpc("meta/mobpacks/templates", {});
  if (!catalogs.sources || catalogs.sources.tools !== "meta/tools/catalog" || catalogs.sources.templates !== "meta/mobpacks/templates") {
    throw new Error(`meta/mobpacks/catalogs did not expose split catalog source metadata: ${JSON.stringify(catalogs.sources)}`);
  }
  if (!Array.isArray(toolsCatalog.tool_catalog) || toolsCatalog.tool_catalog.length !== catalogs.tool_catalog.length) {
    throw new Error(`meta/tools/catalog did not expose the tool catalog slice: ${JSON.stringify(toolsCatalog)}`);
  }
  if (!Array.isArray(skillsCatalog.skill_realms) || skillsCatalog.skill_realms.length !== catalogs.skill_realms.length) {
    throw new Error(`meta/skills/catalog did not expose the skill realm slice: ${JSON.stringify(skillsCatalog)}`);
  }
  if (!Array.isArray(agentDefinitions.agent_definitions) || agentDefinitions.agent_definitions.length !== catalogs.agent_definitions.length) {
    throw new Error(`meta/agent_definitions/list did not expose the agent definition slice: ${JSON.stringify(agentDefinitions)}`);
  }
  if (!templatesCatalog.blank_mobpack || !Array.isArray(templatesCatalog.sample_mobpacks) || !Array.isArray(templatesCatalog.sample_agent_definitions)) {
    throw new Error(`meta/mobpacks/templates did not expose separated templates and samples: ${JSON.stringify(templatesCatalog)}`);
  }
  if (!Array.isArray(catalogs.tool_catalog) || catalogs.tool_catalog.length === 0) {
    throw new Error("meta/mobpacks/catalogs did not expose a real tool_catalog");
  }
  const shellTool = catalogs.tool_catalog.find((tool) => tool.id === "shell");
  if (!shellTool || shellTool.tag_class !== "is-shell") {
    throw new Error(`meta/mobpacks/catalogs did not expose graph tag metadata for shell: ${JSON.stringify(shellTool)}`);
  }
  if (!catalogs.tool_catalog.every((tool) => Object.prototype.hasOwnProperty.call(tool, "tag_class"))) {
    throw new Error(`meta/mobpacks/catalogs omitted tool tag_class metadata: ${JSON.stringify(catalogs.tool_catalog)}`);
  }
  if (!Array.isArray(catalogs.skill_realms) || catalogs.skill_realms.length === 0) {
    throw new Error("meta/mobpacks/catalogs did not expose real skill_realms");
  }
  if (!Array.isArray(catalogs.agent_definitions) || catalogs.agent_definitions.length === 0) {
    throw new Error("meta/mobpacks/catalogs did not expose real agent_definitions");
  }
  if (!Array.isArray(catalogs.sample_agent_definitions) || catalogs.sample_agent_definitions.length === 0) {
    throw new Error("meta/mobpacks/catalogs did not expose sample_agent_definitions separately from authoring agent_definitions");
  }
  const authoringDefinition = catalogs.agent_definitions.find((definition) => (
    definition.role === "reviewer"
    && definition.sourceMobpack === "meta_authoring_profiles"
    && definition.sourceOrigin === "meta/authoring-agent-definitions"
  ));
  if (!authoringDefinition) {
    throw new Error(`meta/mobpacks/catalogs did not expose Meta-owned authoring agent definitions: ${JSON.stringify(catalogs.agent_definitions)}`);
  }
  if (!Array.isArray(authoringDefinition.tools) || !authoringDefinition.tools.includes("shell")) {
    throw new Error(`Meta authoring reviewer definition did not carry real tool refs: ${JSON.stringify(authoringDefinition)}`);
  }
  if (!Array.isArray(authoringDefinition.skills) || !authoringDefinition.skills.includes("mob.authoring.review")) {
    throw new Error(`Meta authoring reviewer definition did not carry real skill refs: ${JSON.stringify(authoringDefinition)}`);
  }
  if (authoringDefinition.schemaDefinition?.id !== "ReviewerOutput") {
    throw new Error(`Meta authoring reviewer definition did not carry its real schema definition: ${JSON.stringify(authoringDefinition)}`);
  }
  if (catalogs.agent_definitions.some((definition) => definition.sourceOrigin === "meta/sample-mobpack" || definition.sourceKind === "sample")) {
    throw new Error(`Meta Agent Editor definitions must not include sample-derived profiles: ${JSON.stringify(catalogs.agent_definitions)}`);
  }
  const authoringRealm = catalogs.skill_realms.find((realm) => realm.id === "meta/authoring-agent-definitions");
  if (!authoringRealm || !String(authoringRealm.source || "").includes("authoring-agent-definition")) {
    throw new Error(`meta/mobpacks/catalogs did not expose authoring skill realm metadata: ${JSON.stringify(catalogs.skill_realms)}`);
  }
  const reviewerDefinitions = catalogs.sample_agent_definitions.filter((definition) => definition.role === "reviewer");
  const reviewerSources = new Set(reviewerDefinitions.map((definition) => definition.sourceMobpack).filter(Boolean));
  const reviewerIds = new Set(reviewerDefinitions.map((definition) => definition.id).filter(Boolean));
  if (reviewerDefinitions.length < 2 || reviewerSources.size < 2 || reviewerIds.size !== reviewerDefinitions.length) {
    throw new Error(`meta/mobpacks/catalogs collapsed sample reviewer agent definitions: ${JSON.stringify(reviewerDefinitions)}`);
  }
  for (const dynamicKey of ["tool_catalog", "skill_realms", "agent_definitions", "sample_agent_definitions", "sample_mobpacks", "blank_mobpack", "models", "provider_defaults"]) {
    if (Object.prototype.hasOwnProperty.call(schema, dynamicKey)) {
      throw new Error(`meta/mobpacks/schema leaked dynamic catalog key ${dynamicKey}`);
    }
  }
  contractSchema = schema;
  const mobDefaults = schema.mob_definition?.mob_settings?.defaults;
  if (mobDefaults?.backendDefault !== "session" || mobDefaults?.advanced?.topology !== null) {
    throw new Error(`flow editor schema did not expose Meta mob setting defaults: ${JSON.stringify(mobDefaults)}`);
  }
  const realmProfileRestriction = schema.mob_definition?.profile_binding_restrictions?.realm_profile;
  if (realmProfileRestriction?.deployable !== false || !String(realmProfileRestriction?.reason || "").includes("rkat mob validate")) {
    throw new Error(`flow editor schema did not expose rkat-backed realm_profile restriction: ${JSON.stringify(realmProfileRestriction)}`);
  }
  const agentView = schema.mob_definition?.editor_agent_view || {};
  for (const [key, expected] of Object.entries({
    member_sub_label_template: "{role} · {model}",
    member_placed_count_template: "×{count}",
    schema_field_singular_template: "{count} field",
    schema_field_plural_template: "{count} fields",
    schema_usage_label_template: "used by {count}",
    sidebar_sub_label_separator: " · ",
  })) {
    if (agentView[key] !== expected) {
      throw new Error(`flow editor schema did not expose Agent sidebar template ${key}: ${JSON.stringify(agentView)}`);
    }
  }
  const agentAccessView = schema.mob_definition?.editor_agent_access_view || {};
  for (const [key, expected] of Object.entries({
    inline_skill_realm_source: "meta/editor",
    inline_skill_source: "inline",
  })) {
    if (agentAccessView[key] !== expected) {
      throw new Error(`flow editor schema did not expose Agent access provenance ${key}: ${JSON.stringify(agentAccessView)}`);
    }
  }
  const schemaView = schema.mob_definition?.editor_schema_view || {};
  for (const [key, expected] of Object.entries({
    fields_title_template: "{prefix} · {count}",
    used_by_title_template: "{prefix} · {count}",
    usage_singular_template: "used by {count} agent",
    usage_plural_template: "used by {count} agents",
  })) {
    if (schemaView[key] !== expected) {
      throw new Error(`flow editor schema did not expose Schema Editor template ${key}: ${JSON.stringify(schemaView)}`);
    }
  }
  const sourceView = schema.mob_definition?.editor_source_view || {};
  if (sourceView.primary_source_path !== "meta/mob.toml") {
    throw new Error(`flow editor schema did not expose primary source path: ${JSON.stringify(sourceView)}`);
  }
  const graphView = schema.mob_definition?.editor_graph_view || {};
  for (const [key, expected] of Object.entries({
    source_file_node_id: "source_mob_toml",
    source_file_node_kind: "source",
    source_file_node_col_offset: 0,
    source_file_node_row_offset: -1,
    source_file_activation_hash: "#meta-graph-source",
    source_file_activation_selector: ".node--source-file",
  })) {
    if (graphView[key] !== expected) {
      throw new Error(`flow editor schema did not expose Graph source-file node contract ${key}: ${JSON.stringify(graphView)}`);
    }
  }
  const samples = catalogs.sample_mobpacks || [];
  const sample = samples.find((candidate) => candidate.id === sampleId) || samples[0];
  if (!sample?.document) throw new Error("flow editor schema did not return any sample mobpack documents");

  const validation = await rpc("meta/mobpacks/validate", { document: sample.document });
  if (!validation.ok) {
    throw new Error(`Meta validation rejected ${sample.id}: ${JSON.stringify(validation.diagnostics)}`);
  }
  if (validation.validation_source !== "rkat mob validate") {
    throw new Error(`Meta validation response did not run rkat mob validate: ${JSON.stringify(validation)}`);
  }
  if (!array(validation.display_rows, "validation.display_rows").some((row) => row.kind === "ok" && row.head === "rkat mob validate executed")) {
    throw new Error(`Meta validation response did not provide executed rkat display rows: ${JSON.stringify(validation.display_rows)}`);
  }

  const sourcePreview = await rpc("meta/mobpacks/source", { document: sample.document });
  if (!sourcePreview.validation?.ok || sourcePreview.source !== "meta/mobpacks/source") {
    throw new Error(`source preview did not return API-backed validated source metadata: ${JSON.stringify(sourcePreview)}`);
  }
  if (Object.prototype.hasOwnProperty.call(sourcePreview, "content_base64")) {
    throw new Error("meta/mobpacks/source must not return the full archive content_base64 payload");
  }
  const previewSourceFiles = array(sourcePreview.source_files, "sourcePreview.source_files");
  const previewMobToml = previewSourceFiles.find((file) => file.path === "meta/mob.toml");
  if (previewMobToml?.text !== sourcePreview.mob_toml || previewMobToml?.media_type !== "text/toml") {
    throw new Error(`source preview mob.toml file does not match Meta-rendered TOML: ${JSON.stringify(previewMobToml)}`);
  }

  const exported = await rpc("meta/mobpacks/export", {
    document: sample.document,
    filename: `${sample.id || "flow-editor-e2e"}.mobpack`,
  });
  if (!exported.validation?.ok) {
    throw new Error(`export validation rejected ${sample.id}: ${JSON.stringify(exported.validation?.diagnostics)}`);
  }
  const sourceFiles = array(exported.source_files, "exported.source_files");
  for (const requiredPath of ["manifest.toml", "definition.json", "meta/editor.json", "meta/mob.toml"]) {
    if (!sourceFiles.some((file) => file.path === requiredPath)) {
      throw new Error(`export did not expose archive source file ${requiredPath}: ${JSON.stringify(sourceFiles)}`);
    }
  }
  const sourceMobToml = sourceFiles.find((file) => file.path === "meta/mob.toml");
  if (sourceMobToml?.text !== exported.mob_toml || sourceMobToml?.media_type !== "text/toml") {
    throw new Error(`exported mob.toml source file does not match Meta-rendered TOML: ${JSON.stringify(sourceMobToml)}`);
  }
  if (sourceMobToml.sha256 !== sha256(Buffer.from(exported.mob_toml || "", "utf8"))) {
    throw new Error(`exported mob.toml source file did not report a matching sha256: ${JSON.stringify(sourceMobToml)}`);
  }

  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "meta-flow-editor-e2e."));
  const packPath = path.join(dir, exported.filename || `${sample.id || "flow-editor-e2e"}.mobpack`);
  fs.writeFileSync(packPath, Buffer.from(exported.content_base64, "base64"));
  fs.writeFileSync(path.join(dir, "mob.toml"), exported.mob_toml || "");

  const inspect = run("rkat", ["mob", "inspect", packPath]);
  const validate = run("rkat", ["mob", "validate", "--trust-policy", "permissive", packPath]);
  const imported = await rpc("meta/mobpacks/import", { content_base64: exported.content_base64 });
  const roundTrip = assertRoundTrip(imported, sample.document);
  const importedValidation = await rpc("meta/mobpacks/validate", { document: imported.document });
  if (!importedValidation.ok) {
    throw new Error(`round-tripped document failed validation: ${JSON.stringify(importedValidation.diagnostics)}`);
  }

  const result = {
    rpcUrl,
    authoringCapabilities,
    sample: sample.id,
    packPath,
    inspect,
    validate,
    sourcePreview: {
      source: sourcePreview.source,
      files: previewSourceFiles.map((file) => ({ path: file.path, media_type: file.media_type, size_bytes: file.size_bytes })),
    },
    sourceFiles: sourceFiles.map((file) => ({ path: file.path, media_type: file.media_type, size_bytes: file.size_bytes })),
    roundTrip,
    graphBranchShape: await validateGraphBranchShape(dir),
    graphParallelShape: await validateGraphParallelShape(dir),
    graphLoopShape: await validateGraphLoopShape(dir),
    editedAgentDefinition: await validateEditedAgentDefinition(dir),
    filesystemSkillPacking: await validateFilesystemSkillPacking(dir),
    unifiedEditorProjection: await validateUnifiedEditorProjection(dir, catalogs),
    realmProfileDefinition: await rejectRealmProfileDefinition(),
    blankMobpackTemplate: await validateBlankMobpackTemplate(dir, catalogs),
    customDeploySettings: await validateCustomDeploySettings(dir),
    documentBackedDeployPreview: await validateDocumentBackedDeployPreview(sample.document),
    hostDeployRpc: null,
    namedTypedOperations: await validateNamedTypedOperations(catalogs),
    inputParamOperations: await validateInputParamOperations(catalogs),
    schemaOperations: await validateSchemaOperations(catalogs),
    flowStepOperations: await validateFlowStepOperations(catalogs),
    graphOperations: await validateGraphOperations(catalogs),
    deploy: null,
  };

  if (runDeploy) {
    if (authoringCapabilities.hostDeployAvailable) {
      result.hostDeployRpc = await validateHostDeployRpc(sample.document, dir);
    }
    result.deploy = run("rkat", [
      "mob",
      "run",
      "--isolated",
      "--realm-backend",
      "jsonl",
      "--trust-policy",
      "permissive",
      packPath,
      "--prompt",
      "Reply with exactly OK.",
    ]);
    if (!/^run\tmob=.*\tstatus=completed$/m.test(result.deploy) || !/^result\t/m.test(result.deploy) || !result.deploy.includes("warning\tunsigned pack accepted in permissive mode")) {
      throw new Error(`rkat mob run output did not match run envelope contract:\n${result.deploy}`);
    }
  }

  console.log(JSON.stringify(result, null, 2));
})().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
