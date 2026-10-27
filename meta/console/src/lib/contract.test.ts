import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";

import {
  CONSOLE_CONTRACT_VERSION,
  CONSOLE_BLOB_PATH_PREFIX,
  CONSOLE_REST_PATHS,
  CONSOLE_RPC_METHODS,
  CONSOLE_RPC_PATHS,
  CONSOLE_TIMELINE_QUERY_MODES,
  CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE,
} from "./contract";

type ContractError = {
  status?: number;
  shape?: {
    error?: string;
    message?: string;
    reason?: string;
    requested_cursor?: string;
    latest_cursor?: string;
  };
  codes?: number[];
};

type ContractSchema = {
  contract_version: string;
  spec_source?: {
    checked_contract?: string;
    previous_contract?: string;
    rct_spec_markdown?: string;
    rct_spec_yaml?: string;
  };
  surfaces: {
    rest: Record<string, {
      method: string;
      path?: string;
      path_template?: string;
      path_prefix?: string;
      response?: { required_top_level_fields?: string[] };
      query_optional_fields?: string[];
      errors?: ContractError[];
    }>;
    rpc: {
      console_rpc_endpoint: { method: string; path: string; errors?: ContractError[] };
      console_multipart_rpc_endpoint: { method: string; path: string; errors?: ContractError[] };
      methods: Record<string, {
        success?: { mode_values?: string[] };
        errors?: ContractError[];
      }>;
    };
    sse: Record<string, {
      method?: string;
      path?: string;
      path_template?: string;
      errors?: ContractError[];
    } | string>;
  };
};

test("console contract constants stay synchronized with docs/rct contract v0.5.0", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.5.0.json"), "utf8"),
  ) as ContractSchema;

  assert.equal(CONSOLE_CONTRACT_VERSION, schema.contract_version);
  assert.equal(CONSOLE_REST_PATHS.experience, schema.surfaces.rest.experience.path);
  assert.equal(CONSOLE_REST_PATHS.modules, schema.surfaces.rest.modules.path);
  assert.equal(CONSOLE_REST_PATHS.identities, schema.surfaces.rest.identities.path);
  assert.equal(CONSOLE_REST_PATHS.timeline, schema.surfaces.rest.timeline.path);
  assert.equal(CONSOLE_REST_PATHS.legacySend, schema.surfaces.rest.legacy_send.path);
  assert.equal(CONSOLE_REST_PATHS.sendRecover, schema.surfaces.rest.send_recover.path);
  assert.equal(CONSOLE_REST_PATHS.mcpAppsResolve, schema.surfaces.rest.mcp_apps_resolve.path);
  assert.equal(CONSOLE_REST_PATHS.mcpAppsReadResource, schema.surfaces.rest.mcp_apps_read_resource.path);
  assert.equal(CONSOLE_REST_PATHS.mcpAppsCallTool, schema.surfaces.rest.mcp_apps_call_tool.path);
  assert.equal(CONSOLE_BLOB_PATH_PREFIX, schema.surfaces.rest.blob.path_prefix);
  assert.equal(CONSOLE_REST_PATHS.timelineStream, (schema.surfaces.sse.timeline as { path: string }).path);
  assert.equal(
    CONSOLE_REST_PATHS.identityTimelineStreamTemplate,
    (schema.surfaces.sse.identity_timeline as { path_template: string }).path_template,
  );
  assert.equal(CONSOLE_RPC_PATHS.jsonRpc, schema.surfaces.rpc.console_rpc_endpoint.path);
  assert.equal(CONSOLE_RPC_PATHS.multipartJsonRpc, schema.surfaces.rpc.console_multipart_rpc_endpoint.path);
  for (const method of Object.values(CONSOLE_RPC_METHODS)) {
    assert.equal(
      method in schema.surfaces.rpc.methods,
      true,
      `expected docs/rct contract to document ${method}`,
    );
  }
  assert.deepEqual(
    Object.keys(schema.surfaces.rpc.methods).filter((method) => !Object.values(CONSOLE_RPC_METHODS).includes(method as typeof CONSOLE_RPC_METHODS[keyof typeof CONSOLE_RPC_METHODS])),
    [],
    "contract JSON must not grow RPC methods without adding typed frontend constants",
  );
  assert.deepEqual(
    [...CONSOLE_TIMELINE_QUERY_MODES],
    schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.queryTimeline]?.success?.mode_values,
  );
  assert.equal(
    schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.queryTimeline]?.errors
      ?.some((entry) => entry.codes?.includes(CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE)),
    true,
  );
});

test("console contract names a checked-in canonical source", () => {
  const contractPath = "../docs/rct/console-rest-sse-contract-v0.5.0.json";
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), contractPath), "utf8"),
  ) as ContractSchema;
  const source = schema.spec_source || {};

  assert.equal(source.checked_contract, "docs/rct/console-rest-sse-contract-v0.5.0.json");
  assert.equal(source.rct_spec_markdown, undefined);
  assert.equal(source.rct_spec_yaml, undefined);
  assert.equal(existsSync(resolve(process.cwd(), contractPath)), true);
  assert.equal(
    source.previous_contract
      ? existsSync(resolve(process.cwd(), "..", source.previous_contract))
      : false,
    true,
  );
});

test("console contract documents live REST, SSE, and send RPC error shapes", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.5.0.json"), "utf8"),
  ) as ContractSchema;
  const sendCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.send]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const listIdentityCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.listIdentities]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const inspectIdentityCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.inspectIdentity]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const queryTimelineCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.queryTimeline]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const routingRouteCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.routingRoutesList]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const deliveryHistoryCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.deliveryHistory]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const gatingPendingCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.gatingPending]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const gatingAuditCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.gatingAudit]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const gatingDecideCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.gatingDecide]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const topologyAuditCodes = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.topologyAuditQuery]?.errors?.flatMap((entry) => entry.codes || []) || [];
  const blobUploadErrors = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.blobUpload]?.errors || [];
  const blobUploadCodes = blobUploadErrors.flatMap((entry) => entry.codes || []);
  const rpcEndpointErrors = schema.surfaces.rpc.console_rpc_endpoint.errors || [];
  const multipartEndpointErrors = schema.surfaces.rpc.console_multipart_rpc_endpoint.errors || [];

  assertRestReason(schema.surfaces.rest.experience.errors, 401, "unauthorized", "string");
  assertRestReason(schema.surfaces.rest.modules.errors, 401, "unauthorized", "string");
  assertRestError(schema.surfaces.rest.identities.errors, 500, "internal_error", "string");
  assertRestError(schema.surfaces.rest.timeline.errors, 401, "unauthorized", "console timeline requires a valid auth token");
  assertRestError(schema.surfaces.rest.timeline.errors, 404, "unavailable", "console aggregator unavailable");
  assertRestError(schema.surfaces.rest.timeline.errors, 409, "replay_unavailable");
  assertRestError(schema.surfaces.rest.blob.errors, 401, "unauthorized");
  assertRestError(schema.surfaces.rest.blob.errors, 400, "invalid_blob_id");
  assertRestError(schema.surfaces.rest.blob.errors, 404, "blob_store_unavailable");
  assertRestError(schema.surfaces.rest.blob.errors, 404, "blob_not_found");
  assertRestError(schema.surfaces.rest.blob.errors, 500, "string");
  assertRestError((schema.surfaces.sse.timeline as { errors?: ContractError[] }).errors, 404, "unavailable", "console aggregator unavailable");
  assert.deepEqual(
    [-32001, -32002, -32004, -32009, -32000, -32602].filter((code) => !sendCodes.includes(code)),
    [],
  );
  assert.equal(sendCodes.includes(-32003), false);
  assert.equal(sendCodes.includes(-32603), false);
  assert.deepEqual([-32004, -32000].filter((code) => !listIdentityCodes.includes(code)), []);
  assert.equal(listIdentityCodes.includes(-32603), false);
  assert.deepEqual([-32001, -32004, -32602, -32000].filter((code) => !inspectIdentityCodes.includes(code)), []);
  assert.equal(inspectIdentityCodes.includes(-32603), false);
  assert.deepEqual([-32004, -32013, -32602].filter((code) => !queryTimelineCodes.includes(code)), []);
  assert.deepEqual([-32004, -32000, -32601].filter((code) => !routingRouteCodes.includes(code)), []);
  assert.deepEqual([-32004, -32000, -32601].filter((code) => !deliveryHistoryCodes.includes(code)), []);
  assert.deepEqual([-32004, -32000, -32601].filter((code) => !gatingPendingCodes.includes(code)), []);
  assert.deepEqual([-32004, -32000, -32601].filter((code) => !gatingAuditCodes.includes(code)), []);
  assert.deepEqual([-32004, -32602, -32000, -32601].filter((code) => !gatingDecideCodes.includes(code)), []);
  assert.deepEqual([-32030, -32009, -32001, -32602, -32000].filter((code) => !topologyAuditCodes.includes(code)), []);
  assert.equal(queryTimelineCodes.includes(-32003), false);
  assert.equal(queryTimelineCodes.includes(-32603), false);
  assert.deepEqual([-32600, -32602, -32000].filter((code) => !blobUploadCodes.includes(code)), []);
  for (const status of [401, 400, 200, 404, 500]) {
    assert.equal(blobUploadErrors.some((entry) => entry.status === status), true);
  }
  assertRpcEndpointError(rpcEndpointErrors, 200, -32600);
  assertRpcEndpointError(rpcEndpointErrors, 401, -32600);
  assertRpcEndpointError(multipartEndpointErrors, 401, -32600);
  assertRpcEndpointError(multipartEndpointErrors, 400, -32602);
  assertRpcEndpointError(multipartEndpointErrors, 200, -32600);
  assertRpcEndpointError(multipartEndpointErrors, 200, -32602);
});

test("console contract route and method names stay synchronized with Rust http console source", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.5.0.json"), "utf8"),
  ) as ContractSchema;
  const rustSource = readFileSync(resolve(process.cwd(), "../crates/meerkat-meta/src/http_console.rs"), "utf8");
  const registeredRoutes = parseAxumRoutes(rustSource);
  const dispatchedRpcMethods = parseJsonRpcDispatchMethods(rustSource);
  // The meta/workgraph/* group dispatches through one guard arm
  // (`method if workgraph_methods::is_workgraph_method(method)`) whose
  // authoritative method list lives in rpc/workgraph_methods.rs — union it in
  // only while http_console.rs actually routes through that guard.
  if (/workgraph_methods::is_workgraph_method\(method\)/.test(rustSource)) {
    const workGraphSource = readFileSync(
      resolve(process.cwd(), "../crates/meerkat-meta/src/rpc/workgraph_methods.rs"),
      "utf8",
    );
    for (const method of parseWorkGraphMethodLists(workGraphSource)) {
      dispatchedRpcMethods.add(method);
    }
  }
  // Topology dispatch uses named constants so the same method identities are
  // shared by stdio and HTTP. Resolve those constants when the HTTP dispatcher
  // references the topology module instead of weakening the source contract
  // with duplicated string literals.
  if (/topology_methods::TOPOLOGY_[A-Z_]+_METHOD/.test(rustSource)) {
    const topologySource = readFileSync(
      resolve(process.cwd(), "../crates/meerkat-meta/src/rpc/topology_methods.rs"),
      "utf8",
    );
    for (const method of parseTopologyMethods(topologySource)) {
      dispatchedRpcMethods.add(method);
    }
  }
  const contractedRoutes = contractRoutes(schema);
  const frontendRoutes = new Set([
    "GET /",
    "GET /favicon.ico",
    "GET /console",
    "GET /console/",
    "GET /console/assets/console-app.js",
    "GET /console/assets/console-app.css",
  ]);

  for (const route of contractedRoutes) {
    assert.equal(
      registeredRoutes.has(route),
      true,
      `expected http_console.rs to register ${route}`,
    );
  }

  for (const route of registeredRoutes) {
    const path = route.split(" ")[1] || "";
    if (!path.startsWith("/console") && !path.startsWith("/blobs")) {
      continue;
    }
    assert.equal(
      contractedRoutes.has(route) || frontendRoutes.has(route),
      true,
      `registered route ${route} must be contracted or explicitly frontend-only`,
    );
  }

  for (const method of Object.keys(schema.surfaces.rpc.methods)) {
    assert.equal(
      dispatchedRpcMethods.has(method),
      true,
      `expected http_console.rs to dispatch RPC method ${method}`,
    );
  }
});

test("console experience contract names required top-level projection fields", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.5.0.json"), "utf8"),
  ) as ContractSchema;
  const required = schema.surfaces.rest.experience.response?.required_top_level_fields || [];

  assert.deepEqual(required, [
    "contract_version",
    "runtime_capabilities",
    "base_panel",
    "agent_sidebar",
    "activity_feed",
    "chat_inspector",
    "topology",
    "health_overview",
    "flows",
    "session_history",
  ]);
});

function parseAxumRoutes(source: string): Set<string> {
  const routes = new Set<string>();
  const routePattern = /\.route\(\s*"([^"]+)"\s*,\s*(get|post)\s*\(/gs;
  for (const match of source.matchAll(routePattern)) {
    routes.add(`${match[2]!.toUpperCase()} ${match[1]}`);
  }
  return routes;
}

function parseJsonRpcDispatchMethods(source: string): Set<string> {
  const methods = new Set<string>();
  const armPattern = /^\s*"([^"]+)"\s*=>/gm;
  for (const match of source.matchAll(armPattern)) {
    methods.add(match[1]!);
  }
  return methods;
}

function parseWorkGraphMethodLists(source: string): Set<string> {
  const methods = new Set<string>();
  const listPattern = /const WORKGRAPH_(?:READ|MUTATE)_METHODS: &\[&str\] = &\[([^\]]+)\]/g;
  for (const list of source.matchAll(listPattern)) {
    for (const entry of list[1]!.matchAll(/"([^"]+)"/g)) {
      methods.add(entry[1]!);
    }
  }
  return methods;
}

function parseTopologyMethods(source: string): Set<string> {
  const methods = new Set<string>();
  const constantPattern = /const TOPOLOGY_[A-Z_]+_METHOD: &str = "([^"]+)"/g;
  for (const match of source.matchAll(constantPattern)) {
    methods.add(match[1]!);
  }
  return methods;
}

function contractRoutes(schema: ContractSchema): Set<string> {
  const routes = new Set<string>();
  for (const surface of Object.values(schema.surfaces.rest)) {
    const path = surface.path || surface.path_template;
    if (path && surface.method) {
      routes.add(`${surface.method.toUpperCase()} ${path}`);
    }
  }
  routes.add(`${schema.surfaces.rpc.console_rpc_endpoint.method.toUpperCase()} ${schema.surfaces.rpc.console_rpc_endpoint.path}`);
  routes.add(`${schema.surfaces.rpc.console_multipart_rpc_endpoint.method.toUpperCase()} ${schema.surfaces.rpc.console_multipart_rpc_endpoint.path}`);
  for (const surface of Object.values(schema.surfaces.sse)) {
    if (typeof surface === "string") {
      continue;
    }
    const path = surface.path || surface.path_template;
    if (path && surface.method) {
      routes.add(`${surface.method.toUpperCase()} ${path}`);
    }
  }
  return routes;
}

function assertRestError(
  errors: ContractError[] | undefined,
  status: number,
  error: string,
  message?: string,
) {
  assert.equal(
    (errors || []).some((entry) => (
      entry.status === status
        && errorMatches(entry.shape?.error, error)
        && (message === undefined || entry.shape?.message === message)
    )),
    true,
    `expected ${status} ${error}${message ? ` ${message}` : ""}`,
  );
}

function assertRestReason(
  errors: ContractError[] | undefined,
  status: number,
  error: string,
  reason: string,
) {
  assert.equal(
    (errors || []).some((entry) => (
      entry.status === status
        && errorMatches(entry.shape?.error, error)
        && entry.shape?.reason === reason
    )),
    true,
    `expected ${status} ${error} reason ${reason}`,
  );
}

function errorMatches(actual: string | undefined, expected: string): boolean {
  if (expected === "string") {
    return actual === "string";
  }
  return (actual || "").split("|").includes(expected);
}

function assertRpcEndpointError(errors: ContractError[], status: number, code: number) {
  assert.equal(
    errors.some((entry) => entry.status === status && (entry.codes || []).includes(code)),
    true,
    `expected endpoint error ${status} ${code}`,
  );
}
