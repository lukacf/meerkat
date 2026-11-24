import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import test from "node:test";

import {
  CONSOLE_CONTRACT_VERSION,
  CONSOLE_BLOB_PATH_PREFIX,
  CONSOLE_HOST_SCOPED_RPC_METHODS,
  CONSOLE_PER_MOB_RPC_METHODS,
  CONSOLE_LISTED_MOB_FIELDS,
  CONSOLE_HOST_MEMBER_PARAM_BY_METHOD,
  CONSOLE_MEMBER_PARAMS_BY_METHOD,
  CONSOLE_MEMBER_MOB_ID_METHODS,
  CONSOLE_MEMBER_MOB_ID_PARAM,
  CONSOLE_MEMBER_NAMING_REFUSAL_KINDS,
  CONSOLE_MEMBER_ROW_BARE_NAME_FIELD,
  CONSOLE_MEMBER_ROW_DOOR_FIELD,
  CONSOLE_MEMBER_ROW_MOB_FIELD,
  CONSOLE_MEMBER_ROW_NAME_FIELDS,
  CONSOLE_MOB_ADDRESSING_CAPABILITY,
  CONSOLE_MOB_OPEN_METHOD,
  CONSOLE_MOB_ID_BODY_ROUTES,
  CONSOLE_MOB_ID_PARAM,
  CONSOLE_MOB_ID_QUERY_ROUTES,
  CONSOLE_MOB_PROVENANCES,
  CONSOLE_MOB_REFUSAL_KIND,
  CONSOLE_MOB_STATES,
  CONSOLE_MOBS_SELECTOR_EVERY,
  CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS,
  CONSOLE_MOBS_SELECTOR_METHODS,
  CONSOLE_MOBS_SELECTOR_PARAM,
  CONSOLE_MOBS_SELECTOR_ROUTES,
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
  note?: string;
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
        request?: { note?: string };
        success?: { mode_values?: string[]; note?: string };
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

test("console contract constants stay synchronized with docs/rct contract v0.6.0", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
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
  const contractPath = "../docs/rct/console-rest-sse-contract-v0.6.0.json";
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), contractPath), "utf8"),
  ) as ContractSchema;
  const source = schema.spec_source || {};

  assert.equal(source.checked_contract, "docs/rct/console-rest-sse-contract-v0.6.0.json");
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
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
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

test("console contract documents the side chat refusals", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
  ) as ContractSchema;
  const forkMember = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.forkMember];
  const forkCodes = forkMember?.errors?.flatMap((entry) => entry.codes || []) || [];
  assert.deepEqual(
    [-32055, -32037, -32001, -32004, -32602, -32603].filter((code) => !forkCodes.includes(code)),
    [],
  );
  const forkNotes = [
    forkMember?.request?.note,
    forkMember?.success?.note,
    ...(forkMember?.errors || []).map((entry) => entry.note),
  ].join(" ");
  for (const kind of [
    "fork_source_unavailable",
    "fork_source_changed",
    "unwired_seat_unavailable",
    "lineage_too_deep",
    "identity_hidden_by_policy",
    "side_chat_restart_refused",
    "side_chat_adoption_refused",
  ]) {
    assert.equal(forkNotes.includes(kind), true, `expected meta/fork_member to document ${kind}`);
  }
  for (const method of [CONSOLE_RPC_METHODS.respawnIdentity, CONSOLE_RPC_METHODS.resetIdentity]) {
    const errors = schema.surfaces.rpc.methods[method]?.errors || [];
    assert.equal(
      errors.some((entry) => entry.codes?.includes(-32602) && entry.note?.includes("side_chat_restart_refused")),
      true,
      `expected ${method} to document side_chat_restart_refused under -32602`,
    );
  }
});

test("console contract decides approvals as the signed-in person only", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
  ) as {
    surfaces: {
      rpc: {
        methods: Record<string, {
          policy_error_refs?: string[];
          request?: { optional_fields?: string[]; refused_fields?: string[] };
          success?: { required_fields?: string[]; actor_kind_values?: string[] };
          errors?: ContractError[];
        }>;
        policy_errors: Record<string, { code?: number; data?: { kind?: string } }>;
      };
    };
  };
  const decide = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.gatingDecide];
  assert.deepEqual(decide?.request?.refused_fields, ["approver_id"]);
  assert.equal(decide?.request?.optional_fields?.includes("approver_id"), false);
  assert.equal(
    decide?.policy_error_refs?.includes("#/surfaces/rpc/policy_errors/approver_unauthenticated"),
    true,
  );
  assert.equal(
    (decide?.errors || []).some((entry) => entry.codes?.includes(-32057)),
    true,
    "expected meta/gating/decide to document -32057",
  );
  const refusal = schema.surfaces.rpc.policy_errors.approver_unauthenticated;
  assert.equal(refusal?.code, -32057);
  assert.equal(refusal?.data?.kind, "approver_unauthenticated");
  const whoami = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.whoami];
  assert.equal(whoami?.success?.required_fields?.includes("actor"), true);
  assert.equal(whoami?.success?.actor_kind_values?.includes("person"), true);
});

test("console contract documents the person-only quarantine review, goal confirmation and claim refusals", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
  ) as {
    surfaces: {
      rpc: {
        methods: Record<string, { policy_error_refs?: string[]; errors?: Array<{ codes?: number[] }> }>;
        policy_errors: Record<string, { code?: number; data?: { kind?: string } }>;
      };
    };
  };
  const method = (name: string) => schema.surfaces.rpc.methods[name];
  for (const [name, refusal] of [
    ["meta/memory/quarantine/decide", "approver_unauthenticated"],
    ["meta/workgraph/goal/confirm", "approver_unauthenticated"],
    ["meta/workgraph/claim", "person_authentication_required"],
    ["meta/workgraph/release", "person_authentication_required"],
  ]) {
    assert.equal(method(name)?.policy_error_refs?.includes(`#/surfaces/rpc/policy_errors/${refusal}`), true, name);
    assert.equal((method(name)?.errors || []).some((entry) => entry.codes?.includes(-32057)), true, name);
  }
  const refusal = schema.surfaces.rpc.policy_errors.person_authentication_required;
  assert.equal(refusal?.code, -32057);
  assert.equal(refusal?.data?.kind, "person_authentication_required");
});

test("console contract documents a login that ended before a decision was taken up as -32056", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
  ) as { surfaces: { rpc: { methods: Record<string, { errors?: Array<{ codes?: number[] }> }> } } };
  for (const name of ["meta/gating/decide", "meta/memory/quarantine/decide", "meta/workgraph/goal/confirm"]) {
    const codes = (schema.surfaces.rpc.methods[name]?.errors || []).flatMap((entry) => entry.codes || []);
    assert.equal(codes.includes(-32057), true, name);
    assert.equal(codes.includes(-32056), true, name);
  }
});

test("console contract documents every method of the long-term agent family, and what a launch without an identity policy answers", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
  ) as ContractSchema;
  const family = parseIdentityFamilyMethods(
    readFileSync(resolve(process.cwd(), "../crates/meerkat-meta/src/rpc/identity_methods.rs"), "utf8"),
  );
  assert.equal(family.size > 0, true, "the family's registry lists its methods");
  const methods = schema.surfaces.rpc.methods as Record<string, { errors?: Array<{ codes?: number[]; note?: string }> }>;
  for (const method of family) {
    const entry = methods[method];
    assert.notEqual(entry, undefined, `the contract documents ${method}`);
    const notes = (entry?.errors ?? []).map((error) => error.note ?? "").join(" ");
    // The reads are served with or without a policy, and a change without
    // an enabled one is refused: no launch with agents answers "no such
    // method" for the family.
    assert.equal(/does not serve the family/.test(notes), false, `${method}: a launch without a policy serves the reads`);
    assert.equal(/change_denied/.test(notes), true, `${method}: a change without an enabled policy is refused change_denied`);
    assert.equal(/-32601 only on a launch without agents of its own/.test(notes), true, method);
  }
});

test("console contract route and method names stay synchronized with Rust http console source", () => {
  const schema = JSON.parse(
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
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
  // The durable decision methods dispatch by their prefix to the Console's
  // decision module, which serves the method constants it names. Resolve
  // those constants while the HTTP dispatcher routes through that module.
  if (/durable_decisions::DECISIONS_METHOD_PREFIX/.test(rustSource)) {
    const decisionWireSource = readFileSync(
      resolve(process.cwd(), "../crates/meerkat-meta/src/durable_decisions/wire.rs"),
      "utf8",
    );
    const decisionConsoleSource = readFileSync(
      resolve(process.cwd(), "../crates/meerkat-meta/src/durable_decisions/console.rs"),
      "utf8",
    );
    for (const method of parseConsoleDecisionMethods(decisionWireSource, decisionConsoleSource)) {
      dispatchedRpcMethods.add(method);
    }
  }
  // The changes and reads of long-term agents dispatch through one guard
  // (`identity_methods::is_identity_family_method(method)`) to the family's
  // own handler, which serves the methods of its registry. Resolve the
  // registry's constants while the HTTP dispatcher routes through the guard.
  if (/identity_methods::is_identity_family_method\(/.test(rustSource)) {
    const identitySource = readFileSync(
      resolve(process.cwd(), "../crates/meerkat-meta/src/rpc/identity_methods.rs"),
      "utf8",
    );
    for (const method of parseIdentityFamilyMethods(identitySource)) {
      dispatchedRpcMethods.add(method);
    }
  }
  // Console-only methods dispatch through named constants of rpc.rs
  // (`request_method == crate::rpc::CONSOLE_WHOAMI_METHOD`); resolve the ones
  // the HTTP dispatcher compares the request method with.
  const rpcSource = readFileSync(
    resolve(process.cwd(), "../crates/meerkat-meta/src/rpc.rs"),
    "utf8",
  );
  for (const method of parseDispatchedConsoleConstants(rustSource, rpcSource)) {
    dispatchedRpcMethods.add(method);
  }
  // The console's own methods dispatch by rpc.rs's named constants
  // (`request_method == crate::rpc::CONSOLE_..._METHOD`, or a `matches!` of
  // them): resolve the constants the dispatcher compares the method with.
  const consoleConstants = parseConsoleMethodDispatchConstants(rustSource);
  for (const [name, method] of parseMethodConstants(rpcSource)) {
    if (consoleConstants.has(name)) dispatchedRpcMethods.add(method);
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
    readFileSync(resolve(process.cwd(), "../docs/rct/console-rest-sse-contract-v0.6.0.json"), "utf8"),
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

test("the topology's team field is the one the Rust experience builder writes", () => {
  // The gateway names the mob its topology shows at topology.live_snapshot.mob_id.
  const builder = readFileSync(
    resolve(process.cwd(), "../crates/meerkat-meta/src/runtime/console_ingress.rs"),
    "utf8",
  );
  assert.match(builder, /body\["topology"\]\["live_snapshot"\]\["mob_id"\]/);
  // Both experience types the console reads declare it there.
  for (const path of ["src/types.ts", "../packages/console-core/src/runtime-types.ts"]) {
    const types = readFileSync(resolve(process.cwd(), path), "utf8");
    const topology = /topology\?: ConsoleExperienceSection<\{[\s\S]*?\n {2}\}>;/.exec(types)?.[0] || "";
    assert.match(topology, /live_snapshot\?: \{[\s\S]*\bmob_id\?: string;/, path);
  }
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
  // One literal per arm, or an alternation of literals
  // (`"meta/mob_events/query" | "meta/mob_events/subscribe" =>`).
  const armPattern = /^\s*("[^"]+"(?:\s*\|\s*"[^"]+")*)\s*=>/gm;
  for (const match of source.matchAll(armPattern)) {
    for (const literal of match[1]!.matchAll(/"([^"]+)"/g)) {
      methods.add(literal[1]!);
    }
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

/** The methods of the long-term agent family: the constants its registry
 *  (`IDENTITY_FAMILY_METHODS`) lists, resolved in the same module. */
function parseIdentityFamilyMethods(source: string): Set<string> {
  const values = new Map<string, string>();
  for (const match of source.matchAll(/const (IDENTITY_[A-Z_]+): &str = "([^"]+)"/g)) {
    values.set(match[1]!, match[2]!);
  }
  const methods = new Set<string>();
  const registry = /const IDENTITY_FAMILY_METHODS: &\[&str\] = &\[([^\]]*)\]/.exec(source);
  for (const name of (registry?.[1] ?? "").matchAll(/IDENTITY_[A-Z_]+/g)) {
    const value = values.get(name[0]);
    if (value) methods.add(value);
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

/** The decision methods the Console's decision module dispatches: the
 *  `*_METHOD` constants of its match arms, resolved in the wire module. */
function parseConsoleDecisionMethods(wireSource: string, consoleSource: string): Set<string> {
  const values = new Map<string, string>();
  for (const match of wireSource.matchAll(/const ([A-Z_]+_METHOD): &str = "([^"]+)"/g)) {
    values.set(match[1]!, match[2]!);
  }
  const methods = new Set<string>();
  const armPattern = /^\s*([A-Z_]+_METHOD(?:\s*\|\s*[A-Z_]+_METHOD)*)(?:\s+if\b[^=]*)?\s*=>/gm;
  for (const arm of consoleSource.matchAll(armPattern)) {
    for (const name of arm[1]!.matchAll(/[A-Z_]+_METHOD/g)) {
      const value = values.get(name[0]);
      if (value) methods.add(value);
    }
  }
  return methods;
}

function parseDispatchedConsoleConstants(dispatcher: string, rpcSource: string): Set<string> {
  const methods = new Set<string>();
  const constantPattern = /pub const (CONSOLE_[A-Z_]+_METHOD): &str = "([^"]+)"/g;
  for (const match of rpcSource.matchAll(constantPattern)) {
    if (dispatcher.includes(`request_method == crate::rpc::${match[1]!}`)) {
      methods.add(match[2]!);
    }
  }
  return methods;
}

function parseConsoleMethodDispatchConstants(source: string): Set<string> {
  const names = new Set<string>();
  const constant = /crate::rpc::(CONSOLE_[A-Z_]+_METHOD)\b/g;
  for (const match of source.matchAll(/request_method == crate::rpc::(CONSOLE_[A-Z_]+_METHOD)\b/g)) {
    names.add(match[1]!);
  }
  for (const arms of source.matchAll(/matches!\(\s*request_method\.as_str\(\),\s*([^)]*)\)/g)) {
    for (const match of arms[1]!.matchAll(constant)) names.add(match[1]!);
  }
  return names;
}

function parseMethodConstants(source: string): Map<string, string> {
  const constants = new Map<string, string>();
  for (const match of source.matchAll(/pub const ([A-Z_]+_METHOD): &str = "([^"]+)"/g)) {
    constants.set(match[1]!, match[2]!);
  }
  return constants;
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

// ---------------------------------------------------------------------------
// Contract v0.6.0: the mob of a request (mob_addressing).
// ---------------------------------------------------------------------------

type MobAddressing = {
  capability: { name: string };
  mob_id: {
    name: string;
    placement: {
      params_member: { doors: string[] };
      query_parameter: string[];
      body_member: string[];
    };
    refusal: {
      kind: string;
      route: { status: number; shape: { error: string; requested_mob_id: string } };
      rpc: { status: number; code: number; data: { kind: string; requested_mob_id: string } };
    };
    not_a_string: { route: { status: number; shape: { error: string; message: string } } };
  };
  mobs_selector: {
    name: string;
    values: string[];
    valid_on: { routes: string[]; rpc_methods: string[] };
    refused: { route: { status: number; shape: { error: string } }; rpc: { code: number } };
  };
  listed_mob: {
    required_fields: string[];
    optional_fields: string[];
    state_values: { values: string[]; added_in_this_version: string[] };
    provenance_values: { values: string[] };
  };
  frame: { field: string };
  limits: { send_retry: string };
  compatibility: { contract_version: string; older_client_newer_gateway: string };
  row_key: { row_members: { door: string; member_mob: string; member_name: string }; shared_spelling: string };
  member_mob_id: {
    name: string;
    methods: {
      of_this_contract: string[];
      outside_this_contract: { methods: string[] };
      member_word: Record<string, string>;
      roster_reads: { methods: string[] };
    };
    refusals: {
      code: number;
      earlier_kind: string;
      kinds: Record<string, { data: { kind: string } }>;
      routes: { status: number; shape: { error: string }; routes: string[] };
    };
  };
  scopes: {
    one_mob?: string;
    one_mob_not_yet_served?: { methods?: string[] };
    methods_outside_this_contract?: string;
  };
};

type ContractRoute = {
  method?: string;
  path?: string;
  path_template?: string;
  mob_scope?: string;
  query_optional_fields?: string[];
  request?: { optional_fields?: string[] };
  errors?: Array<{ status?: number; shape?: { error?: string; requested_mob_id?: string } }>;
};

type ContractV6 = {
  contract_version: string;
  mob_addressing: MobAddressing;
  surfaces: {
    rest: Record<string, ContractRoute>;
    sse: Record<string, ContractRoute | string>;
    rpc: {
      methods: Record<string, {
        mob_scope?: string;
        member_mob_id?: string;
        success?: { optional_fields?: string[]; request_optional_fields?: string[] };
      }>;
    };
  };
};

const CONTRACT_V6_PATH = "../docs/rct/console-rest-sse-contract-v0.6.0.json";

function readContractV6(): ContractV6 {
  return JSON.parse(readFileSync(resolve(process.cwd(), CONTRACT_V6_PATH), "utf8")) as ContractV6;
}

/** `GET /path` for each of the paths. */
function doorsOf(method: "GET" | "POST", paths: readonly string[]): string[] {
  return paths.map((path) => `${method} ${path}`);
}

/** A source file of the gateway, by its path from the console package, or
 *  null where this tree has none. */
function gatewaySource(fromConsole: string): string | null {
  const path = resolve(process.cwd(), fromConsole);
  return existsSync(path) ? readFileSync(path, "utf8") : null;
}

/** The body of `pub struct NAME {...}` or `pub enum NAME {...}` in a Rust source. */
function rustItemBody(source: string, kind: "struct" | "enum", name: string): string {
  const start = new RegExp(`pub ${kind} ${name}\\s*\\{`).exec(source);
  if (!start) return "";
  let depth = 0;
  for (let at = start.index + start[0].length - 1; at < source.length; at += 1) {
    const char = source[at];
    if (char === "{") depth += 1;
    if (char === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(start.index, at + 1);
    }
  }
  return "";
}

/** Whether a Rust struct body writes the JSON member `name`: a field of that
 *  name (raw or not), or a field renamed to it. */
function rustStructWrites(body: string, name: string): boolean {
  return new RegExp(`pub (?:r#)?${name}\\s*:`).test(body)
    || new RegExp(`rename\\s*=\\s*"${name}"`).test(body);
}

/** Whether a `rename_all = "snake_case"` Rust enum body has the wire word. */
function rustEnumHasWord(body: string, word: string): boolean {
  const variant = word.split("_").map((part) => part.charAt(0).toUpperCase() + part.slice(1)).join("");
  return new RegExp(`^\\s*${variant}\\b`, "m").test(body)
    || new RegExp(`rename\\s*=\\s*"${word}"`).test(body);
}

test("console contract constants for the mob of a request equal contract v0.6.0", () => {
  const schema = readContractV6();
  const addressing = schema.mob_addressing;

  assert.equal(CONSOLE_CONTRACT_VERSION, "0.6.0");
  assert.equal(CONSOLE_MOB_ADDRESSING_CAPABILITY, addressing.capability.name);
  assert.equal(CONSOLE_MOB_ID_PARAM, addressing.mob_id.name);
  assert.equal(CONSOLE_MOB_ID_PARAM, addressing.frame.field);
  assert.equal(CONSOLE_MOBS_SELECTOR_PARAM, addressing.mobs_selector.name);
  assert.deepEqual([CONSOLE_MOBS_SELECTOR_EVERY], addressing.mobs_selector.values);
  assert.equal(CONSOLE_MOB_REFUSAL_KIND, addressing.mob_id.refusal.kind);
  assert.equal(CONSOLE_MOB_REFUSAL_KIND, addressing.mob_id.refusal.route.shape.error);
  assert.equal(CONSOLE_MOB_REFUSAL_KIND, addressing.mob_id.refusal.rpc.data.kind);
  assert.equal(addressing.mob_id.refusal.route.status, 404);
  assert.equal(addressing.mob_id.refusal.rpc.code, -32602);
  assert.equal(addressing.mobs_selector.refused.route.status, 400);
  assert.equal(addressing.mobs_selector.refused.rpc.code, -32602);

  assert.deepEqual(
    doorsOf("POST", [CONSOLE_RPC_PATHS.jsonRpc, CONSOLE_RPC_PATHS.multipartJsonRpc]),
    addressing.mob_id.placement.params_member.doors,
  );
  assert.deepEqual(doorsOf("GET", CONSOLE_MOB_ID_QUERY_ROUTES), addressing.mob_id.placement.query_parameter);
  assert.deepEqual(doorsOf("POST", CONSOLE_MOB_ID_BODY_ROUTES), addressing.mob_id.placement.body_member);
  assert.deepEqual(doorsOf("GET", CONSOLE_MOBS_SELECTOR_ROUTES), addressing.mobs_selector.valid_on.routes);
  assert.deepEqual([...CONSOLE_MOBS_SELECTOR_METHODS], addressing.mobs_selector.valid_on.rpc_methods);

  assert.deepEqual([...CONSOLE_MOB_STATES], addressing.listed_mob.state_values.values);
  assert.deepEqual(["dormant", "unavailable"], addressing.listed_mob.state_values.added_in_this_version);
  assert.deepEqual([...CONSOLE_MOB_PROVENANCES], addressing.listed_mob.provenance_values.values);
  assert.deepEqual(
    [...CONSOLE_LISTED_MOB_FIELDS].sort(),
    [...addressing.listed_mob.required_fields, ...addressing.listed_mob.optional_fields].sort(),
  );
  // A mob that is not loaded has no count: the member is optional.
  assert.equal(addressing.listed_mob.optional_fields.includes("member_count"), true);
  assert.deepEqual(addressing.listed_mob.required_fields, ["mob_id", "state"]);
  // No client compares the version: the document says so.
  assert.match(addressing.compatibility.contract_version, /^tolerant:/);
});

test("contract v0.6.0 gives every method one scope, and the methods of the host are the constants'", () => {
  const schema = readContractV6();
  const methods = schema.surfaces.rpc.methods;
  const wrong = Object.entries(methods)
    .filter(([, entry]) => !["host", "one_mob", "launch_mob_only"].includes(entry.mob_scope ?? ""))
    .map(([name]) => name);
  assert.deepEqual(wrong, [], "every method is the host's, serves one mob, or is the launch mob's alone");
  assert.deepEqual(
    Object.entries(methods).filter(([, entry]) => entry.mob_scope === "host").map(([name]) => name).sort(),
    [...CONSOLE_HOST_SCOPED_RPC_METHODS].sort(),
  );
  // The methods of scope one_mob are the constant's: the methods of this
  // contract the request doors serve for a mob other than the launch mob.
  // A method outside it that is not the host's is refused for another mob,
  // also where the transport names a request's mob on it. A method
  // of that scope the document lists as not served yet is not in the
  // constant: it enters it with the gateway change that serves it.
  const notYetServed = schema.mob_addressing.scopes.one_mob_not_yet_served?.methods ?? [];
  assert.deepEqual(
    notYetServed.filter((name) => methods[name]?.mob_scope !== "one_mob"),
    [],
    "a method listed as not served yet is a method of scope one_mob",
  );
  // The scope's sentence points at that list exactly while it holds a
  // method.
  assert.equal(
    (schema.mob_addressing.scopes.one_mob ?? "").includes("one_mob_not_yet_served"),
    notYetServed.length > 0,
    "the scope's sentence names the not-yet-served list exactly while the list holds a method",
  );
  assert.deepEqual(
    Object.entries(methods)
      .filter(([name, entry]) => entry.mob_scope === "one_mob" && !notYetServed.includes(name))
      .map(([name]) => name)
      .sort(),
    [...CONSOLE_PER_MOB_RPC_METHODS].sort(),
  );
  // The gating lists and their decision are the host's: one list for a person.
  for (const method of [
    CONSOLE_RPC_METHODS.gatingPending,
    CONSOLE_RPC_METHODS.gatingAudit,
    CONSOLE_RPC_METHODS.gatingDecide,
    CONSOLE_RPC_METHODS.gatingQuestions,
  ]) {
    assert.equal(methods[method]?.mob_scope, "host", method);
  }
  assert.equal(
    methods[CONSOLE_RPC_METHODS.queryTimeline]?.success?.request_optional_fields?.includes(CONSOLE_MOBS_SELECTOR_PARAM),
    true,
  );
  assert.equal(
    methods[CONSOLE_RPC_METHODS.capabilities]?.success?.optional_fields?.includes("feature_capabilities"),
    true,
  );
});

test("contract v0.6.0 places the mob of a request on each of the twelve console routes, with its one refusal", () => {
  const schema = readContractV6();
  const addressing = schema.mob_addressing;
  const routes: Array<[string, ContractRoute]> = [];
  for (const surface of [schema.surfaces.rest, schema.surfaces.sse]) {
    for (const route of Object.values(surface)) {
      if (typeof route === "string" || !route.method) continue;
      routes.push([`${route.method} ${route.path || route.path_template}`, route]);
    }
  }
  const taking = routes.filter(([, route]) => route.mob_scope !== undefined);
  assert.deepEqual(
    taking.map(([door]) => door).sort(),
    [...addressing.mob_id.placement.query_parameter, ...addressing.mob_id.placement.body_member].sort(),
    "the twelve routes, each once",
  );
  assert.equal(taking.length, 12);
  // A blob is the host's and is addressed by its id alone.
  assert.deepEqual(
    routes.filter(([, route]) => route.mob_scope === undefined).map(([door]) => door),
    [`GET ${CONSOLE_BLOB_PATH_PREFIX}{blob_id}`],
  );
  for (const [door, route] of taking) {
    const inQuery = route.query_optional_fields?.includes(CONSOLE_MOB_ID_PARAM) === true;
    const inBody = route.request?.optional_fields?.includes(CONSOLE_MOB_ID_PARAM) === true;
    assert.equal(inQuery, addressing.mob_id.placement.query_parameter.includes(door), `${door}: query parameter`);
    assert.equal(inBody, addressing.mob_id.placement.body_member.includes(door), `${door}: body member`);
    assert.equal(
      (route.errors || []).some((entry) => (
        entry.status === 404
          && entry.shape?.error === CONSOLE_MOB_REFUSAL_KIND
          && entry.shape?.requested_mob_id === "string"
      )),
      true,
      `${door}: the refusal of a mob it does not serve`,
    );
    const takesSelector = route.query_optional_fields?.includes(CONSOLE_MOBS_SELECTOR_PARAM) === true;
    assert.equal(takesSelector, addressing.mobs_selector.valid_on.routes.includes(door), `${door}: the selector`);
  }
});

test("the gateway names the mob of a request as contract v0.6.0 does", () => {
  const schema = readContractV6();
  const addressing = gatewaySource("../crates/meerkat-meta/src/rpc/mob_addressing.rs");
  assert.notEqual(addressing, null, "the gateway reads a request's mob (rpc/mob_addressing.rs)");
  const constant = (name: string) => new RegExp(`const ${name}: &str = "([^"]+)"`).exec(addressing!)?.[1];
  assert.equal(constant("MOB_ADDRESSING_FEATURE"), schema.mob_addressing.capability.name);
  assert.equal(constant("FOREIGN_MOB_TARGET_KIND"), schema.mob_addressing.mob_id.refusal.kind);
  assert.equal(constant("MOB_ID_NOT_A_STRING"), schema.mob_addressing.mob_id.not_a_string.route.shape.message);
  assert.match(addressing!, /"requested_mob_id"/);
});

test("the gateway's methods of the host are contract v0.6.0's", () => {
  const schema = readContractV6();
  // The console's door reads its own list of the methods of the host: the
  // list the door that takes a request's mob decides by.
  const door = gatewaySource("../crates/meerkat-meta/src/http_console/mobs_selector.rs");
  assert.notEqual(door, null, "the console's door lists its methods of the host (http_console/mobs_selector.rs)");
  const listOf = (name: string): Set<string> => {
    const list = new RegExp(`const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`).exec(door!)?.[1] ?? "";
    return new Set([...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]!));
  };
  const hostMethods = listOf("CONSOLE_METHODS_OF_THE_HOST");
  assert.equal(hostMethods.size > 0, true, "the door lists its methods of the host");
  // The lists of the host that take the selector are the document's.
  const selectorLists = (schema.mob_addressing.mobs_selector.valid_on as { host_lists?: string[] }).host_lists ?? [];
  assert.deepEqual(
    [...listOf("HOST_LIST_METHODS")].sort(),
    [...selectorLists].sort(),
    "the lists that take the selector are the document's",
  );
  // Every method the door serves as a method of the host is a method of
  // the document, and of the host there: the door's list is walked too, so
  // a method the document does not have is seen.
  assert.deepEqual(
    [...hostMethods].filter((name) => schema.surfaces.rpc.methods[name]?.mob_scope !== "host").sort(),
    [],
    "methods of the host for the door that are not methods of the host in the document",
  );
  const methods = schema.surfaces.rpc.methods;
  const differ = Object.keys(methods)
    .filter((name) => (methods[name]?.mob_scope === "host") !== hostMethods.has(name));
  assert.deepEqual(differ, [], "a method is the host's in the document exactly when it is in the gateway");
});

test("the gateway writes each member and state of a listed mob that contract v0.6.0 names", () => {
  const schema = readContractV6();
  const listed = schema.mob_addressing.listed_mob;
  // A listed mob is written by two reads. The plain read lists the launch
  // mob and the mobs nested under it, through its own type. The read over
  // every mob lists the mobs the gateway hosts, through its own function.
  // Each must write its own members and state words.
  const hostedAdds = ["provenance", "default", "incarnation"];
  const nestedOnly = ["parent_mob_id", "host_identity"];
  const fields = [...listed.required_fields, ...listed.optional_fields];
  assert.deepEqual(
    [...hostedAdds, ...nestedOnly].filter((field) => !listed.optional_fields.includes(field)),
    [],
    "the members one read alone writes are optional members of a listed mob",
  );

  // The plain read: every member but the ones a hosted mob adds, and every
  // state word of contract v0.5.0.
  const builder = gatewaySource("../crates/meerkat-meta/src/runtime/console_ingress.rs") ?? "";
  const mob = rustItemBody(builder, "struct", "ConsoleMob");
  const state = rustItemBody(builder, "enum", "ConsoleMobState");
  assert.notEqual(mob, "", "the gateway lists mobs (ConsoleMob)");
  assert.notEqual(state, "", "the gateway names a listed mob's state (ConsoleMobState)");
  assert.deepEqual(
    fields.filter((field) => !hostedAdds.includes(field) && !rustStructWrites(mob, field)),
    [],
    "members the plain read does not write",
  );
  // The plain read's type writes the count as a number that is always
  // there: an entry of that list is never without one.
  assert.match(mob, /pub member_count\s*:\s*usize\b/, "the plain read's type always writes a count");
  const added = listed.state_values.added_in_this_version;
  assert.deepEqual(
    listed.state_values.values.filter((word) => !added.includes(word) && !rustEnumHasWord(state, word)),
    [],
    "state words the plain read does not write",
  );

  // The read over every mob: every member but the ones only a nested mob
  // has, the count only for a loaded mob, and the state words this version
  // adds.
  const everyMob = gatewaySource("../crates/meerkat-meta/src/http_console/every_mob.rs") ?? "";
  const listedMob = /fn listed_mob\([\s\S]*?\n\}/.exec(everyMob)?.[0] ?? "";
  assert.notEqual(listedMob, "", "the gateway's read over every mob writes a listed mob (http_console/every_mob.rs)");
  const writtenByEveryMob = (field: string) =>
    listedMob.includes(`"${field}":`) || listedMob.includes(`entry["${field}"]`);
  assert.deepEqual(
    fields.filter((field) => !nestedOnly.includes(field) && !writtenByEveryMob(field)),
    [],
    "members the read over every mob does not write",
  );
  assert.equal(
    listedMob.includes('entry["member_count"]'),
    true,
    "a mob that is not loaded has no count: the read over every mob writes it onto an entry opened without one",
  );
  const everyMobCode = everyMob.split("#[cfg(test)]")[0] ?? "";
  assert.deepEqual(
    added.filter((word) => !everyMobCode.includes(`"${word}"`)),
    [],
    "state words of this version the read over every mob does not write",
  );
});

test("a served frame names its mob where contract v0.6.0 says", () => {
  const schema = readContractV6();
  const types = gatewaySource("../crates/meerkat-meta/src/console_aggregator/types.rs") ?? "";
  const frame = rustItemBody(types, "struct", "ConsoleFrame");
  assert.notEqual(frame, "", "the gateway's frame type");
  assert.equal(rustStructWrites(frame, schema.mob_addressing.frame.field), true, "a frame carries mob_id");
  // Absent for the launch mob: an optional member that is left out when empty.
  assert.match(frame, /skip_serializing_if = "Option::is_none"\)\]\s*pub mob_id\s*:\s*Option</);
});

test("the console's listed mob type declares the members and states contract v0.6.0 adds", () => {
  const schema = readContractV6();
  const listed = schema.mob_addressing.listed_mob;
  const mobs = readFileSync(resolve(process.cwd(), "../packages/console-core/src/mobs.ts"), "utf8");
  const entry = /export interface ConsoleMobEntry \{[\s\S]*?\n\}/.exec(mobs)?.[0] ?? "";
  assert.notEqual(entry, "", "the console's listed mob type (ConsoleMobEntry)");
  const fields = [...listed.required_fields, ...listed.optional_fields];
  assert.deepEqual(
    fields.filter((field) => !new RegExp(`^\\s*${field}\\??:`, "m").test(entry)),
    [],
    "members the console's type does not declare",
  );
  const state = /export type ConsoleMobState = ([^;]+);/.exec(mobs)?.[1] ?? "";
  assert.deepEqual(
    listed.state_values.values.filter((word) => !state.includes(`"${word}"`)),
    [],
    "state words the console does not read",
  );
});

// The shared cases of the member row key and of the door of a member. The
// two functions below are the contract's own reading of
// mob_addressing.row_key: they keep the case file consistent with the
// document; the console's and the gateway's functions are pinned by the
// same file in their own tests.

type RowKeyMob = { mob_id: string; parent_mob_id?: string; provenance?: string; default?: boolean };
type RowKeyRequest = { mob_id: string | null; identity: string };
type RowKeyRequestByMob = { mob_id: string | null; member_mob_id: string; identity: string };
type RowKeyMembers = Record<string, string>;
type RowKeyCases = {
  mobs: RowKeyMob[];
  door: Array<{
    case: string;
    mob_id: string;
    name: string;
    row_key: string | null;
    request: RowKeyRequest | null;
    request_by_mob?: RowKeyRequestByMob;
  }>;
  frame: Array<{ case: string; mob_id: string | null; identity: string; row_key: string | null }>;
  row: Array<{ case: string; mob_id: string; name: string; row: RowKeyMembers }>;
  plain_row: Array<{ case: string; mob_id: string; name: string; row: RowKeyMembers }>;
  whole: Array<{
    case: string;
    mob_id: string;
    name: string;
    row: RowKeyMembers;
    frame: RowKeyRequest;
    request: RowKeyRequest;
    request_by_mob: RowKeyRequestByMob;
  }>;
  node: Array<{ case: string; mob_id: string; name: string; row_key: string; node: RowKeyMembers }>;
  shared_spelling: Array<{
    case: string;
    identity: string;
    request: null;
    members: Array<{ mob_id: string; name: string; row: RowKeyMembers; request_by_mob: RowKeyRequestByMob }>;
  }>;
};

/** The mob at the top of `mobId`'s tree, or null for a mob the list does not hold. */
function rowKeyRoot(mobs: RowKeyMob[], mobId: string): RowKeyMob | null {
  let at = mobs.find((mob) => mob.mob_id === mobId) ?? null;
  for (let hops = 0; at && at.parent_mob_id && hops <= mobs.length; hops += 1) {
    const parent = mobs.find((mob) => mob.mob_id === at!.parent_mob_id);
    if (!parent) break;
    at = parent;
  }
  return at;
}

/** Whether a mob at the top of a tree has a door of its own. */
function rowKeyOwnDoor(root: RowKeyMob): boolean {
  return root.provenance === "host" && root.default !== true;
}

/**
 * The member `name` of the mob `mobId` as its door names it: the row key
 * (the door's mob id before the name as the door knows it, on a door other
 * than the launch mob's), the one-string request and the two-value request.
 */
function rowKeyDoor(mobs: RowKeyMob[], mobId: string, name: string) {
  const root = rowKeyRoot(mobs, mobId);
  const own = mobs.find((mob) => mob.mob_id === mobId);
  if (!root || !own || !name) return { row_key: null, request: null, request_by_mob: null, door: null };
  const launch = mobs.find((mob) => mob.default === true)!.mob_id;
  const door = rowKeyOwnDoor(root) ? root.mob_id : null;
  const known = mobId === (door ?? launch) ? name : `${mobId}/${name}`;
  return {
    row_key: door === null ? known : `${door}/${known}`,
    request: { mob_id: door, identity: known },
    request_by_mob: { mob_id: door, member_mob_id: mobId, identity: name },
    door: door ?? launch,
  };
}

/** The row key a frame's pair is: with no look at what is nested where. */
function rowKeyOfFrame(mobs: RowKeyMob[], mobId: string | null, identity: string): string | null {
  if (!identity) return null;
  if (mobId === null) return identity;
  const door = mobs.find((mob) => mob.mob_id === mobId);
  if (!door) return null;
  return door.default === true ? identity : `${mobId}/${identity}`;
}

/** The members a door writes on the row of the member `name` of `mobId`. */
function rowKeyRowOf(mobs: RowKeyMob[], mobId: string, name: string): RowKeyMembers {
  const keyed = rowKeyDoor(mobs, mobId, name);
  const row: RowKeyMembers = {};
  for (const field of CONSOLE_MEMBER_ROW_NAME_FIELDS) row[field] = keyed.row_key!;
  row[CONSOLE_MEMBER_ROW_DOOR_FIELD] = keyed.door!;
  row[CONSOLE_MEMBER_ROW_MOB_FIELD] = mobId;
  row[CONSOLE_MEMBER_ROW_BARE_NAME_FIELD] = name;
  return row;
}

test("the shared row key cases agree with contract v0.6.0: a row is keyed by its door, and a frame's pair is the key", () => {
  const cases = JSON.parse(
    readFileSync(resolve(process.cwd(), "../crates/meerkat-meta/src/console_row_key_cases.json"), "utf8"),
  ) as RowKeyCases;
  const schema = readContractV6();
  // The names the cases use are the document's and the constants'.
  assert.deepEqual(schema.mob_addressing.row_key.row_members.door, CONSOLE_MEMBER_ROW_DOOR_FIELD);
  assert.deepEqual(schema.mob_addressing.row_key.row_members.member_mob, CONSOLE_MEMBER_ROW_MOB_FIELD);
  assert.deepEqual(schema.mob_addressing.row_key.row_members.member_name, CONSOLE_MEMBER_ROW_BARE_NAME_FIELD);
  assert.equal(schema.mob_addressing.member_mob_id.name, CONSOLE_MEMBER_MOB_ID_PARAM);
  assert.equal(schema.mob_addressing.member_mob_id.refusals.code, -32602);
  assert.deepEqual(
    Object.keys(schema.mob_addressing.member_mob_id.refusals.kinds).sort(),
    Object.values(CONSOLE_MEMBER_NAMING_REFUSAL_KINDS).sort(),
  );
  for (const [word, kind] of Object.entries(schema.mob_addressing.member_mob_id.refusals.kinds)) {
    assert.equal(kind.data.kind, word, `${word}: the kind the answer carries`);
  }
  assert.equal(cases.door.length >= 8, true);
  assert.equal(cases.frame.length >= 8, true);
  assert.equal(cases.mobs.filter((mob) => mob.default === true).length, 1, "one launch mob");
  for (const mob of cases.mobs) {
    assert.equal(mob.mob_id.includes("/"), false, `${mob.mob_id}: a mob id holds no separator`);
  }
  for (const entry of cases.door) {
    const keyed = rowKeyDoor(cases.mobs, entry.mob_id, entry.name);
    assert.deepEqual(
      { row_key: keyed.row_key, request: keyed.request, request_by_mob: keyed.request_by_mob },
      { row_key: entry.row_key, request: entry.request, request_by_mob: entry.request_by_mob ?? null },
      entry.case,
    );
    // The one-string pair of a request is the pair the member's frames
    // carry, and the pair is the row key.
    if (entry.request) {
      assert.equal(
        rowKeyOfFrame(cases.mobs, entry.request.mob_id, entry.request.identity),
        entry.row_key,
        `${entry.case}: the request pair as a frame pair`,
      );
    }
    // The two values name the member's own mob and its bare name.
    if (entry.request_by_mob) {
      assert.equal(entry.request_by_mob[CONSOLE_MEMBER_MOB_ID_PARAM], entry.mob_id, `${entry.case}: the member's mob`);
      assert.equal(entry.request_by_mob.identity, entry.name, `${entry.case}: the bare name`);
    }
  }
  for (const entry of cases.frame) {
    assert.equal(rowKeyOfFrame(cases.mobs, entry.mob_id, entry.identity), entry.row_key, entry.case);
  }
  // A member row carries the row key in every name of its member, its door,
  // the mob its member sits in and the member's bare name, on either read.
  assert.equal(cases.row.length >= 4, true);
  assert.equal(cases.plain_row.length >= 2, true);
  for (const entry of [...cases.row, ...cases.plain_row]) {
    assert.deepEqual(entry.row, rowKeyRowOf(cases.mobs, entry.mob_id, entry.name), entry.case);
  }
  // A member of a mob nested under a hosted mob, whole.
  assert.equal(cases.whole.length >= 1, true);
  for (const entry of cases.whole) {
    const keyed = rowKeyDoor(cases.mobs, entry.mob_id, entry.name);
    assert.deepEqual(entry.row, rowKeyRowOf(cases.mobs, entry.mob_id, entry.name), entry.case);
    assert.deepEqual(entry.request, keyed.request, `${entry.case}: the one-string request`);
    assert.deepEqual(entry.request_by_mob, keyed.request_by_mob, `${entry.case}: the two-value request`);
    assert.deepEqual(entry.frame, entry.request, `${entry.case}: the frame's pair is the request's`);
    assert.equal(
      rowKeyOfFrame(cases.mobs, entry.frame.mob_id, entry.frame.identity),
      entry.row.identity,
      `${entry.case}: the frame's pair is the row's key`,
    );
    assert.notEqual(entry.row[CONSOLE_MEMBER_ROW_DOOR_FIELD], entry.row[CONSOLE_MEMBER_ROW_MOB_FIELD], entry.case);
  }
  // A topology node names its member as a frame does, and carries the mob
  // the member sits in and its bare name as a row does.
  assert.equal(cases.node.length >= 4, true);
  for (const entry of cases.node) {
    const keyed = rowKeyDoor(cases.mobs, entry.mob_id, entry.name);
    assert.deepEqual(
      entry.node,
      {
        identity: keyed.request!.identity,
        [CONSOLE_MEMBER_ROW_DOOR_FIELD]: keyed.door!,
        [CONSOLE_MEMBER_ROW_MOB_FIELD]: entry.mob_id,
        [CONSOLE_MEMBER_ROW_BARE_NAME_FIELD]: entry.name,
      },
      entry.case,
    );
    assert.equal(entry.row_key, keyed.row_key, `${entry.case}: the key the node is filed under`);
    assert.equal(rowKeyOfFrame(cases.mobs, entry.node.mob_id!, entry.node.identity!), entry.row_key, entry.case);
  }
  // Two members of one door can spell one key: their rows carry one
  // identity and differ in the two members; a one-string request names
  // neither, and each is named by the two values.
  assert.equal(cases.shared_spelling.length >= 1, true);
  for (const entry of cases.shared_spelling) {
    assert.equal(entry.request, null, `${entry.case}: no one-string request`);
    assert.equal(entry.members.length, 2, entry.case);
    for (const member of entry.members) {
      const keyed = rowKeyDoor(cases.mobs, member.mob_id, member.name);
      assert.equal(keyed.row_key, entry.identity, `${entry.case}: ${member.mob_id} spells the identity`);
      assert.deepEqual(member.row, rowKeyRowOf(cases.mobs, member.mob_id, member.name), entry.case);
      assert.deepEqual(member.request_by_mob, keyed.request_by_mob, entry.case);
    }
    const pairs = entry.members.map(
      (member) => `${member.row[CONSOLE_MEMBER_ROW_MOB_FIELD]}\u0000${member.row[CONSOLE_MEMBER_ROW_BARE_NAME_FIELD]}`,
    );
    assert.equal(new Set(pairs).size, 2, `${entry.case}: the two members differ in their mob and name`);
  }
  assert.match(schema.mob_addressing.row_key.shared_spelling, /member_mob_id/);
  // The judge of a shared spelling is the method the request is of, and
  // the changed form of the answer is told to a client of contract v0.5.0.
  assert.match(schema.mob_addressing.row_key.shared_spelling, /its method would answer the caller for/);
  assert.match(
    schema.mob_addressing.compatibility.older_client_newer_gateway,
    new RegExp(CONSOLE_MEMBER_NAMING_REFUSAL_KINDS.required),
  );
  // The contract lists no breaking change: none is verified as an
  // incompatible change of an existing request, and the earlier answer
  // stands beside the two kinds of a member's naming.
  const changes = (schema as unknown as { breaking_changes: unknown[] }).breaking_changes;
  assert.deepEqual(changes, [], "the contract lists no breaking change");
  assert.match(
    schema.mob_addressing.member_mob_id.refusals.earlier_kind,
    /is answered as a gateway of contract v0\.5\.0 answers it/,
  );
  assert.doesNotMatch(schema.mob_addressing.member_mob_id.refusals.earlier_kind, /data\.kind/);
  assert.match(schema.mob_addressing.limits.send_retry, /is answered the first send's acceptance/);
  // What is unique is the mob a member sits in and its name there.
  const members = cases.door
    .filter((entry) => entry.row_key !== null)
    .map((entry) => `${entry.mob_id}\u0000${entry.name}`);
  assert.equal(new Set(members).size, members.length, "a member's mob and name are unique");
});

test("contract v0.6.0 names the opening request and the member of params by which each method names a member", () => {
  const schema = JSON.parse(readFileSync(resolve(process.cwd(), CONTRACT_V6_PATH), "utf8")) as {
    mob_addressing: {
      opening: { method: string; automatic: string };
      members: Record<string, string>;
      row_key: { row_names: string; host_list_entries: string; launch_mob_by_id: string };
    };
    surfaces: {
      rpc: {
        methods: Record<string, { mob_scope?: string; member_params?: string[]; host_member_param?: string }>;
      };
    };
  };
  const methods = schema.surfaces.rpc.methods;
  assert.equal(CONSOLE_MOB_OPEN_METHOD, schema.mob_addressing.opening.method);
  assert.equal(CONSOLE_MOB_OPEN_METHOD, CONSOLE_RPC_METHODS.status);
  assert.equal(methods[CONSOLE_MOB_OPEN_METHOD]?.mob_scope, "one_mob");

  const documented: Record<string, string[]> = {};
  const hostDocumented: Record<string, string> = {};
  for (const [name, entry] of Object.entries(methods)) {
    if (entry.member_params) {
      assert.notEqual(entry.mob_scope, "host", `${name}: a method that names a member of a mob is no method of the host`);
      documented[name] = entry.member_params;
    }
    if (entry.host_member_param) {
      assert.equal(entry.mob_scope, "host", `${name}: a row key names a member for a method of the host`);
      hostDocumented[name] = entry.host_member_param;
    }
    assert.equal(
      entry.member_params !== undefined && entry.host_member_param !== undefined,
      false,
      `${name}: one form of naming a member`,
    );
  }
  assert.deepEqual(documented, { ...CONSOLE_MEMBER_PARAMS_BY_METHOD });
  assert.deepEqual(hostDocumented, { ...CONSOLE_HOST_MEMBER_PARAM_BY_METHOD });
  // A method of the host never names a member by the name a door knows.
  for (const method of CONSOLE_HOST_SCOPED_RPC_METHODS) {
    assert.equal(method in CONSOLE_MEMBER_PARAMS_BY_METHOD, false, method);
  }
  for (const field of CONSOLE_MEMBER_ROW_NAME_FIELDS) {
    assert.equal(schema.mob_addressing.row_key.row_names.includes(field), true, field);
  }
  assert.match(schema.mob_addressing.members.not_served_for_another_mob, /mcp-apps/);
  // A pair that names the launch mob by its own id is the launch mob's.
  assert.match(schema.mob_addressing.row_key.launch_mob_by_id, /read as one without mob_id/);
  // An answer names a member as the request's door knows it.
  assert.match(schema.mob_addressing.members.answers ?? "", /named as that mob's door knows it/);
});

test("contract v0.6.0 serves the host's lists per mob only to a request that asks for every mob, and states its limits", () => {
  const schema = JSON.parse(readFileSync(resolve(process.cwd(), CONTRACT_V6_PATH), "utf8")) as {
    mob_addressing: {
      mobs_selector: { host_lists: string; valid_on: { host_lists: string[] } };
      row_key: { host_list_entries: string };
      compatibility: { older_client_newer_gateway: string };
      limits: Record<string, string>;
    };
    surfaces: {
      rpc: { methods: Record<string, { mob_scope?: string; mobs_selector?: string; host_list_entry?: string }> };
    };
  };
  const addressing = schema.mob_addressing;
  const methods = schema.surfaces.rpc.methods;
  assert.deepEqual([...CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS], addressing.mobs_selector.valid_on.host_lists);
  for (const method of CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS) {
    assert.equal(methods[method]?.mob_scope, "host", method);
    assert.equal(typeof methods[method]?.host_list_entry, "string", method);
    assert.equal(CONSOLE_HOST_SCOPED_RPC_METHODS.includes(method), true, method);
  }
  // No other method of the host takes the selector.
  assert.deepEqual(
    Object.entries(methods)
      .filter(([, entry]) => entry.mob_scope === "host" && entry.mobs_selector !== undefined)
      .map(([name]) => name)
      .sort(),
    [...CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS].sort(),
  );
  assert.match(addressing.mobs_selector.host_lists, /Without it/);
  // Five lists take the selector, the console audit read among them; a
  // decision row names no member and no mob, so its list does not.
  assert.equal(CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS.length, 5);
  assert.equal(
    (CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS as readonly string[]).includes(CONSOLE_RPC_METHODS.decisionsList),
    false,
  );
  assert.equal(methods[CONSOLE_RPC_METHODS.decisionsList]?.mobs_selector, undefined);
  assert.match(addressing.mobs_selector.host_lists, /meta\/console\/audit/);
  assert.match(addressing.mobs_selector.host_lists, /a decision row names no member and no mob/);
  // The gating stream stays a limit: no selector, nothing for another mob.
  assert.match(addressing.limits.gating_stream ?? "", /takes no selector/);
  assert.match(addressing.limits.gating_stream ?? "", /nothing for an entry of another mob/);
  // A row of another mob offers a lifecycle action only where the door
  // serves its method per mob: the stop, and no retirement.
  assert.match(addressing.limits.member_actions ?? "", /can_retire \(meta\/retire\)/);
  assert.match(addressing.limits.member_actions ?? "", /offers the stop and none of the four/);
  assert.match(addressing.row_key.host_list_entries, /mob_id/);
  assert.match(addressing.compatibility.older_client_newer_gateway, /lists of the host/);
  assert.deepEqual(
    Object.keys(addressing.limits).sort(),
    [
      "frames_shared_spelling",
      "gating_stream",
      "mcp_apps",
      "member_actions",
      "member_stream",
      "methods",
      "note",
      "routes",
      "send",
      "send_retry",
      "shared_spelling_rows",
      "voice",
    ],
  );
  // The methods the request doors do not serve for another mob are a
  // stated limit, and the method that opens a mob is not among them.
  assert.match(addressing.limits.methods ?? "", /launch_mob_only in this version/);
});

test("contract v0.6.0 documents the console audit read: its request, its answer, who may call it and what a mob named beside it gets", () => {
  const schema = JSON.parse(readFileSync(resolve(process.cwd(), CONTRACT_V6_PATH), "utf8")) as {
    mob_addressing: { mobs_selector: { host_lists: string; valid_on: { host_lists: string[] } } };
    surfaces: {
      rpc: {
        methods: Record<string, {
          mob_scope?: string;
          policy_error_refs?: string[];
          request?: { optional_fields?: string[]; log_values?: string[]; validation?: string };
          success?: { required_fields?: string[]; retention_values?: string[]; note?: string };
          errors?: Array<{ codes?: number[] }>;
          who?: string;
          mobs_selector?: string;
          host_list_entry?: string;
          mob_note?: string;
        }>;
      };
    };
  };
  const audit = schema.surfaces.rpc.methods[CONSOLE_RPC_METHODS.consoleAudit];
  assert.notEqual(audit, undefined, "the document has the console audit read");
  assert.equal(audit?.mob_scope, "host");
  assert.equal(CONSOLE_HOST_SCOPED_RPC_METHODS.includes(CONSOLE_RPC_METHODS.consoleAudit), true);
  assert.deepEqual(audit?.request?.optional_fields, ["log", "after_seq", "limit", CONSOLE_MOBS_SELECTOR_PARAM]);
  assert.deepEqual(audit?.request?.log_values, ["changes", "tool_calls"]);
  assert.deepEqual(audit?.success?.required_fields, [
    "log",
    "records",
    "next_after_seq",
    "oldest_available_seq",
    "latest_seq",
    "has_more",
    "gap",
    "retention",
  ]);
  assert.deepEqual(audit?.success?.retention_values, ["process", "durable"]);
  assert.equal(audit?.policy_error_refs?.includes("#/surfaces/rpc/policy_errors/access_denied"), true);
  assert.match(audit?.who ?? "", /audit\.view/);
  const codes = (audit?.errors || []).flatMap((entry) => entry.codes || []);
  assert.deepEqual([-32601, -32602, -32603].filter((code) => !codes.includes(code)), []);
  // A mob named beside it selects nothing; the selector decides whose rows
  // are answered, and a row names its mob.
  assert.equal(audit?.mobs_selector, "#/mob_addressing/mobs_selector/host_lists");
  assert.match(audit?.mob_note ?? "", /mob_id selects nothing/);
  assert.match(audit?.host_list_entry ?? "", /target\.mob_id/);
  assert.equal(schema.mob_addressing.mobs_selector.valid_on.host_lists.includes(CONSOLE_RPC_METHODS.consoleAudit), true);
  assert.equal(
    (CONSOLE_MOBS_SELECTOR_HOST_LIST_METHODS as readonly string[]).includes(CONSOLE_RPC_METHODS.consoleAudit),
    true,
  );

  // The gateway's read takes the request the document names and answers
  // the page the document names.
  const handler = gatewaySource("../crates/meerkat-meta/src/http_console/audit.rs") ?? "";
  for (const field of ["log", "after_seq", "limit"]) {
    assert.equal(handler.includes(`params.get("${field}")`), true, `the audit read takes ${field}`);
  }
  const page = /struct ConsoleAuditPage \{[\s\S]*?\n\}/.exec(
    gatewaySource("../crates/meerkat-meta/src/console_audit.rs") ?? "",
  )?.[0] ?? "";
  assert.notEqual(page, "", "the gateway's audit page type");
  assert.deepEqual(
    (audit?.success?.required_fields ?? []).filter((field) => !new RegExp(`pub\\(crate\\) ${field}\\s*:`).test(page)),
    [],
    "members of the page the gateway does not write",
  );
  // And the other way: the page type has no member the document leaves out.
  const written = [...page.matchAll(/pub\(crate\) (\w+)\s*:/g)].map((found) => found[1]!);
  assert.equal(written.length > 0, true, "the members of the gateway's audit page type");
  assert.deepEqual(
    written.filter((field) => !(audit?.success?.required_fields ?? []).includes(field)),
    [],
    "members of the gateway's page the document does not name",
  );
  // The record sets a read names and the bounds of a page are the
  // gateway's own.
  const auditSource = gatewaySource("../crates/meerkat-meta/src/console_audit.rs") ?? "";
  const sets = /enum ConsoleAuditSet \{[\s\S]*?\n\}/.exec(auditSource)?.[0] ?? "";
  const setWords = [...sets.matchAll(/^\s+([A-Z]\w*),/gm)].map((found) =>
    found[1]!.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase(),
  );
  assert.deepEqual(setWords, audit?.request?.log_values, "the record sets of the gateway's log");
  const bound = (name: string) => new RegExp(`const ${name}: usize = (\\d+);`).exec(auditSource)?.[1] ?? "";
  assert.notEqual(bound("CONSOLE_AUDIT_MAX_PAGE"), "", "the gateway's largest page");
  assert.notEqual(bound("CONSOLE_AUDIT_DEFAULT_PAGE"), "", "the gateway's default page");
  assert.equal(
    (audit?.request?.validation ?? "").includes(
      `from 1 to ${bound("CONSOLE_AUDIT_MAX_PAGE")} (${bound("CONSOLE_AUDIT_DEFAULT_PAGE")} by default)`,
    ),
    true,
    "the bounds of a page are the gateway's",
  );
  // A record's members that are not always there are named.
  for (const member of ["target", "receipt", "code", "kind", "truncated"]) {
    assert.equal((audit?.success?.note ?? "").includes(member), true, `the record's ${member}`);
  }
});

test("a method of contract v0.6.0 serves the mob a request names exactly when the gateway's door serves it per mob", () => {
  const schema = readContractV6();
  const methods = schema.surfaces.rpc.methods;
  // The console's door serves a request for a mob other than the launch
  // mob on a closed list of methods. A method of the host takes no mob as
  // a selector; every other method is the launch mob's alone.
  const door = gatewaySource("../crates/meerkat-meta/src/http_console.rs");
  assert.notEqual(door, null, "the console's door (http_console.rs)");
  const list = /const CONSOLE_METHODS_SERVED_PER_MOB: &\[&str\] = &\[([^\]]*)\]/.exec(door!)?.[1] ?? "";
  const perMob = new Set([...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]!));
  assert.equal(perMob.size > 0, true, "the door lists the methods it serves per mob");
  const scopeOf = (name: string) => methods[name]?.mob_scope;
  assert.deepEqual(
    Object.keys(methods).filter((name) => !["host", "one_mob", "launch_mob_only"].includes(scopeOf(name) ?? "")),
    [],
    "every method of the document names one of the three scopes",
  );
  // The document says a method serves the mob a request names exactly when
  // the door does, and that it is the launch mob's alone exactly when the
  // door lists it nowhere. A method of that scope the door does not list
  // yet is one the document lists as not served yet, and no other: when the
  // door serves it, the document's list and the console's constant follow
  // in one change, and until they do this row names the method.
  const notYetServed = schema.mob_addressing.scopes.one_mob_not_yet_served?.methods ?? [];
  assert.deepEqual(
    Object.keys(methods).filter((name) => scopeOf(name) === "one_mob" && !perMob.has(name)).sort(),
    [...notYetServed].sort(),
    "the methods of scope one_mob the door does not serve are the ones the document lists as not served yet",
  );
  assert.deepEqual(
    Object.keys(methods).filter((name) => scopeOf(name) !== "one_mob" && perMob.has(name)).sort(),
    [],
    "methods the door serves per mob and the document does not",
  );
});

// The methods the console's door serves per mob that this contract does
// not list: no console of this contract sends them.
const DOOR_PER_MOB_METHODS_OUTSIDE_THE_CONTRACT = [
  "meta/find_members",
  "meta/get_member",
  "meta/list_members",
  "meta/mob_events/subscribe",
  "meta/workgraph/attention/break_glass_reassign",
  "meta/workgraph/attention/list",
  "meta/workgraph/attention/prune",
  "meta/workgraph/block",
  "meta/workgraph/create",
  "meta/workgraph/evidence/add",
  "meta/workgraph/facts",
  "meta/workgraph/goal/create",
  "meta/workgraph/link",
  "meta/workgraph/list",
  "meta/workgraph/policy/escalate",
  "meta/workgraph/ready",
  "meta/workgraph/update",
];

test("a method the gateway's door serves per mob is a method of contract v0.6.0 or one of the named methods the contract does not list", () => {
  const schema = readContractV6();
  const methods = schema.surfaces.rpc.methods;
  const door = gatewaySource("../crates/meerkat-meta/src/http_console.rs");
  assert.notEqual(door, null, "the console's door (http_console.rs)");
  const list = /const CONSOLE_METHODS_SERVED_PER_MOB: &\[&str\] = &\[([^\]]*)\]/.exec(door!)?.[1] ?? "";
  const perMob = [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]!);
  assert.equal(perMob.length > 0, true, "the door lists the methods it serves per mob");
  // A method that joins the door's list is seen here: it is a method of
  // the document, or it is named above as one the contract does not list.
  assert.deepEqual(
    perMob.filter((name) => methods[name] === undefined).sort(),
    [...DOOR_PER_MOB_METHODS_OUTSIDE_THE_CONTRACT].sort(),
    "the door's per-mob methods that are no methods of the document",
  );
  assert.match(schema.mob_addressing.scopes.methods_outside_this_contract ?? "", /does not list/);
});

test("the methods that take member_mob_id are the door's list: the marked methods of contract v0.6.0 and the four it names outside", () => {
  const schema = readContractV6();
  const methods = schema.surfaces.rpc.methods;
  const entry = schema.mob_addressing.member_mob_id.methods;
  const marked = Object.keys(methods)
    .filter((name) => methods[name]!.member_mob_id !== undefined)
    .sort();
  assert.deepEqual(marked, [...CONSOLE_MEMBER_MOB_ID_METHODS].sort(), "the marked methods are the constant's");
  assert.deepEqual([...entry.of_this_contract].sort(), marked, "the entry lists the marked methods");
  for (const name of marked) {
    assert.equal(methods[name]!.member_mob_id, "#/mob_addressing/member_mob_id", name);
    assert.equal(entry.member_word[name], "identity", `${name}: the member word`);
  }
  for (const name of entry.outside_this_contract.methods) {
    assert.equal(methods[name], undefined, `${name} is no method of the document`);
  }
  assert.deepEqual(
    Object.keys(entry.member_word).sort(),
    [...entry.of_this_contract, ...entry.outside_this_contract.methods].sort(),
    "a member word for each method that takes it",
  );
  for (const name of entry.roster_reads.methods) {
    assert.equal(entry.member_word[name], "member_id", `${name}: a roster read`);
  }
  // The door's own lists, read from its source.
  const door = gatewaySource("../crates/meerkat-meta/src/http_console.rs");
  assert.notEqual(door, null, "the console's door (http_console.rs)");
  const names = (list: string) => [...list.matchAll(/"([^"]+)"/g)].map((match) => match[1]!);
  const taking = names(
    /const CONSOLE_METHODS_TAKING_MEMBER_MOB_ID: &\[&str\] = &\[([^\]]*)\]/.exec(door!)?.[1] ?? "",
  );
  assert.equal(taking.length > 0, true, "the door lists the methods that take member_mob_id");
  assert.deepEqual(
    taking.filter((name) => methods[name] !== undefined).sort(),
    marked,
    "the door's methods that are methods of the document",
  );
  assert.deepEqual(
    taking.filter((name) => methods[name] === undefined).sort(),
    [...entry.outside_this_contract.methods].sort(),
    "the door's methods that are no methods of the document",
  );
  assert.deepEqual(
    names(/const CONSOLE_ROSTER_READS: &\[&str\] = &\[([^\]]*)\]/.exec(door!)?.[1] ?? "").sort(),
    [...entry.roster_reads.methods].sort(),
    "the door's roster reads",
  );
});

test("no member route takes member_mob_id: each refuses it as contract v0.6.0 says", () => {
  const schema = readContractV6();
  const refusal = schema.mob_addressing.member_mob_id.refusals.routes;
  assert.equal(refusal.status, 400);
  assert.equal(refusal.shape.error, CONSOLE_MEMBER_NAMING_REFUSAL_KINDS.notAccepted);
  assert.deepEqual([...refusal.routes].sort(), [
    "GET /console/identity/{identity}/stream",
    "GET /console/timeline",
    "GET /console/timeline/stream",
    "POST /console/send",
    "POST /console/send/recover",
  ]);
  const listed = [...Object.values(schema.surfaces.rest), ...Object.values(schema.surfaces.sse)]
    .filter((route): route is ContractRoute => typeof route === "object")
    .filter((route) =>
      (route.errors ?? []).some(
        (entry) => entry.status === 400 && entry.shape?.error === CONSOLE_MEMBER_NAMING_REFUSAL_KINDS.notAccepted,
      ))
    .map((route) => `${route.method} ${route.path ?? route.path_template}`)
    .sort();
  assert.deepEqual(listed, [...refusal.routes].sort(), "the routes whose errors list the refusal");
  // The door's own refusal, read from its source.
  const door = gatewaySource("../crates/meerkat-meta/src/http_console.rs");
  assert.notEqual(door, null, "the console's door (http_console.rs)");
  assert.match(
    door!,
    /fn route_takes_no_member_mob_id\(\)[^}]*StatusCode::BAD_REQUEST,\s*"member_mob_id_not_accepted"/,
    "the door refuses the member on a route with a 400 and the kind word",
  );
});

test("contract v0.6.0 names the member of each entry of the listing of identities by the three values of its row", () => {
  const addressing = readContractV6().mob_addressing as unknown as {
    member_mob_id: { answer?: string };
    limits: { shared_spelling_rows?: string };
  };
  const answer = addressing.member_mob_id.answer ?? "";
  for (const field of [
    CONSOLE_MEMBER_ROW_DOOR_FIELD,
    CONSOLE_MEMBER_ROW_MOB_FIELD,
    CONSOLE_MEMBER_ROW_BARE_NAME_FIELD,
  ]) {
    assert.match(answer, new RegExp(`\\b${field}\\b`), `the answer names ${field}`);
  }
  assert.match(answer, /meta\/console\/list_identities/, "the answer names the RPC listing");
  assert.match(answer, /GET \/console\/identities/, "the answer names the route's listing");
  assert.match(
    addressing.limits.shared_spelling_rows ?? "",
    /the listing is the source of member_mob_id and member_name for a pair/,
    "the limit of a pair names the listing as the source of the two values",
  );
  assert.ok(
    (CONSOLE_PER_MOB_RPC_METHODS as readonly string[]).includes(CONSOLE_RPC_METHODS.inspectIdentity),
    "the inspection of an identity is served for the mob a request names",
  );
});
