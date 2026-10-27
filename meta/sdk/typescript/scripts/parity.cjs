#!/usr/bin/env node

const assert = require("node:assert/strict");
const {
  MetaTypedClient,
  buildConsoleModulesRoute,
  defineModuleSpec,
  parseDurableAgentSpec,
  parsePersistedEvent,
} = require("../dist/index.cjs");

const gatewayBin = process.env.META_RPC_GATEWAY_BIN;
if (!gatewayBin) {
  console.error("META_RPC_GATEWAY_BIN must be set");
  process.exit(2);
}

const results = [];

function check(name, fn) {
  try {
    fn();
    results.push({ name, ok: true });
  } catch (error) {
    results.push({ name, ok: false, error: String(error && error.message ? error.message : error) });
  }
}

function assertDeepEqual(actual, expected, label) {
  try {
    assert.deepStrictEqual(actual, expected);
  } catch (_error) {
    throw new Error(`${label}: observed=${JSON.stringify(actual)} expected=${JSON.stringify(expected)}`);
  }
}

const client = new MetaTypedClient(gatewayBin);

check("typed client status success", () => {
  const response = client.rpc("ts-status", "meta/status", {});
  assertDeepEqual(response, {
    jsonrpc: "2.0",
    id: "ts-status",
    result: {
      contract_version: "0.5.0",
      running: true,
      loaded_modules: ["routing"],
    },
  }, "unexpected status");
});

check("typed client capabilities success", () => {
  const response = client.rpc("ts-caps", "meta/capabilities", {});
  const methods = response.result && response.result.methods;
  if (!Array.isArray(methods) || !methods.includes("meta/events/subscribe")) {
    throw new Error(`unexpected capabilities methods: ${JSON.stringify(response)}`);
  }
});

check("typed client invalid params exact json-rpc error", () => {
  const response = client.rpc("ts-invalid", "meta/spawn_member", {});
  assertDeepEqual(response, {
    jsonrpc: "2.0",
    id: "ts-invalid",
    error: {
      code: -32602,
      message: "Invalid params: module_id required",
    },
  }, "unexpected invalid params error shape");
});

check("typed client unloaded module exact json-rpc error", () => {
  const response = client.rpc("ts-unloaded", "delivery/tools.list", { probe: "parity" });
  assertDeepEqual(response, {
    jsonrpc: "2.0",
    id: "ts-unloaded",
    error: {
      code: -32601,
      message: "Module 'delivery' not loaded",
    },
  }, "unexpected unloaded error shape");
});

check("console route helper encodes auth token", () => {
  const route = buildConsoleModulesRoute("token+/=?");
  if (route !== "/console/modules?auth_token=token%2B%2F%3D%3F") {
    throw new Error(`unexpected console route: ${route}`);
  }
});

check("module-authoring helper normalizes schema", () => {
  const moduleSpec = defineModuleSpec({
    id: "router",
    command: "node",
    args: ["router.js"],
    restartPolicy: "on_failure",
    boundary: "mcp",
  });

  assertDeepEqual(moduleSpec, {
    id: "router",
    command: "node",
    args: ["router.js"],
    restart_policy: "on_failure",
    boundary: "mcp",
  }, "unexpected module spec");
});

check("persisted events parse rust internal tags", () => {
  const event = parsePersistedEvent({
    id: "e-1",
    seq: 1,
    timestamp_ms: 1000,
    member_id: "lead",
    event: {
      kind: "agent",
      agent_id: "lead",
      event_type: "run_completed",
      payload: { ok: true },
    },
  });
  assertDeepEqual(event.event, {
    kind: "agent",
    agentId: "lead",
    eventType: "run_completed",
    payload: { ok: true },
  }, "unexpected persisted event parse");
});

check("identity-first agent identity rejects invalid shape", () => {
  assert.throws(
    () => parseDurableAgentSpec({ identity: "bad identity", profile: "worker" }),
    /invalid agent identity/,
  );
});

const failed = results.filter((item) => !item.ok).length;
const passed = results.length - failed;
const summary = {
  sdk: "typescript",
  passed,
  failed,
  checks: results,
};

process.stdout.write(JSON.stringify(summary));
process.exit(failed === 0 ? 0 : 1);
