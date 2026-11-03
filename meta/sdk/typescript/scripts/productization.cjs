#!/usr/bin/env node

const assert = require("node:assert/strict");
const {
  MetaAsyncClient,
  MetaRpcError,
  buildConsoleRoute,
  buildConsoleModulesRoute,
  buildConsoleExperienceRoute,
  buildConsoleRoutes,
  defineModuleSpec,
  decorateModuleSpec,
  defineModuleTool,
  defineModule,
} = require("../dist/index.cjs");

const results = [];
const EXPECTED_CONTRACT_VERSION = "0.5.0";

async function check(name, fn) {
  try {
    await fn();
    results.push({ name, ok: true });
  } catch (error) {
    results.push({
      name,
      ok: false,
      error: String(error && error.message ? error.message : error),
    });
  }
}

function assertDeepEqual(actual, expected, label) {
  try {
    assert.deepStrictEqual(actual, expected);
  } catch (_error) {
    throw new Error(`${label}: observed=${JSON.stringify(actual)} expected=${JSON.stringify(expected)}`);
  }
}

async function run() {
  const gatewayBin = process.env.META_RPC_GATEWAY_BIN;

  const asyncTransport = async (request) => {
    if (!request || request.jsonrpc !== "2.0" || typeof request.id !== "string") {
      throw new Error("invalid JSON-RPC request");
    }

    const params = request.params || {};

    if (request.method === "meta/status") {
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          contract_version: EXPECTED_CONTRACT_VERSION,
          running: true,
          loaded_modules: ["routing", "delivery"],
        },
      };
    }

    if (request.method === "meta/capabilities") {
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          contract_version: EXPECTED_CONTRACT_VERSION,
          methods: [
            "meta/status",
            "meta/capabilities",
            "meta/reconcile",
            "meta/spawn_member",
            "meta/events/subscribe",
          ],
          loaded_modules: ["routing", "delivery"],
        },
      };
    }

    if (request.method === "meta/reconcile") {
      const modules = Array.isArray(params.modules) ? params.modules.filter((item) => typeof item === "string") : [];
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          accepted: true,
          reconciled_modules: modules,
          added: modules.includes("delivery") ? 1 : 0,
        },
      };
    }

    if (request.method === "meta/spawn_member") {
      const moduleId = typeof params.module_id === "string" ? params.module_id.trim() : "";
      if (!moduleId) {
        return {
          jsonrpc: "2.0",
          id: request.id,
          error: {
            code: -32602,
            message: "Invalid params: module_id required",
          },
        };
      }

      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          accepted: true,
          module_id: moduleId,
        },
      };
    }

    if (request.method === "meta/events/subscribe") {
      const scope = params.scope === "agent" || params.scope === "interaction" ? params.scope : "mob";
      if (scope === "agent" && typeof params.agent_id !== "string") {
        return {
          jsonrpc: "2.0",
          id: request.id,
          error: {
            code: -32602,
            message: "Invalid params: scope=agent requires non-empty agent_id",
          },
        };
      }
      return {
        jsonrpc: "2.0",
        id: request.id,
        result: {
          scope,
          replay_from_event_id: params.last_event_id || null,
          keep_alive: {
            interval_ms: 15000,
            event: "keep-alive",
          },
          keep_alive_comment: ": keep-alive\n\n",
          event_frames: [
            "id: evt-routing\nevent: ready\ndata: {\"kind\":\"module\"}\n\n",
          ],
          events: [
            {
              event_id: "evt-routing",
              source: "module",
              timestamp_ms: 101,
              event: {
                kind: "module",
                module: "routing",
                event_type: "ready",
                payload: { ok: true },
              },
            },
          ],
        },
      };
    }

    return {
      jsonrpc: "2.0",
      id: request.id,
      error: {
        code: -32601,
        message: `Method not found: ${request.method}`,
      },
    };
  };

  const client = new MetaAsyncClient(asyncTransport);

  await check("async client status typed result", async () => {
    const status = await client.status("ts-prod-status");
    assertDeepEqual(status, {
      contract_version: EXPECTED_CONTRACT_VERSION,
      running: true,
      loaded_modules: ["routing", "delivery"],
    }, "unexpected status");
  });

  await check("async client capabilities typed result", async () => {
    const caps = await client.capabilities("ts-prod-caps");
    if (!caps.methods.includes("meta/events/subscribe")) {
      throw new Error(`missing events subscribe capability: ${JSON.stringify(caps.methods)}`);
    }
    if (!caps.methods.includes("meta/reconcile") || !caps.methods.includes("meta/spawn_member")) {
      throw new Error(`missing key methods: ${JSON.stringify(caps.methods)}`);
    }
  });

  await check("async client reconcile typed result", async () => {
    const reconcile = await client.reconcile(["routing", "delivery"], "ts-prod-reconcile");
    assertDeepEqual(reconcile, {
      accepted: true,
      reconciled_modules: ["routing", "delivery"],
      added: 1,
    }, "unexpected reconcile");
  });

  await check("async client spawn_member typed result", async () => {
    const spawned = await client.spawnMember("delivery", "ts-prod-spawn");
    assertDeepEqual(spawned, {
      accepted: true,
      module_id: "delivery",
    }, "unexpected spawn_member");
  });

  await check("async client events subscribe typed shape", async () => {
    const subscribed = await client.subscribeEvents({ scope: "mob" }, "ts-prod-events");
    if (subscribed.scope !== "mob") {
      throw new Error(`unexpected scope: ${subscribed.scope}`);
    }
    if (!Array.isArray(subscribed.event_frames) || subscribed.event_frames.length !== 1) {
      throw new Error(`unexpected event frames: ${JSON.stringify(subscribed.event_frames)}`);
    }
    if (!Array.isArray(subscribed.events) || subscribed.events.length !== 1) {
      throw new Error(`unexpected events: ${JSON.stringify(subscribed.events)}`);
    }
  });

  await check("async client rpc errors surface typed metadata", async () => {
    let observed = null;
    try {
      await client.spawnMember("", "ts-prod-invalid-spawn");
    } catch (error) {
      observed = error;
    }
    if (!(observed instanceof MetaRpcError)) {
      throw new Error(`expected MetaRpcError, got ${String(observed)}`);
    }
    if (observed.code !== -32602 || observed.method !== "meta/spawn_member") {
      throw new Error(`unexpected rpc error metadata: ${JSON.stringify({
        code: observed.code,
        method: observed.method,
        requestId: observed.requestId,
      })}`);
    }
  });

  await check("async factory fromGatewayBin status success", async () => {
    if (!gatewayBin) {
      throw new Error("META_RPC_GATEWAY_BIN must be set for fromGatewayBin checks");
    }

    const factoryClient = MetaAsyncClient.fromGatewayBin(gatewayBin);
    const status = await factoryClient.status("ts-prod-factory-gateway-status");
    assertDeepEqual(status, {
      contract_version: EXPECTED_CONTRACT_VERSION,
      running: true,
      loaded_modules: ["routing"],
    }, "unexpected fromGatewayBin status");
  });

  await check("async factory fromGatewayBin transport errors surface", async () => {
    const factoryClient = MetaAsyncClient.fromGatewayBin("/__meta__/missing/sdk-test-gateway");
    let observed = null;
    try {
      await factoryClient.status("ts-prod-factory-gateway-missing");
    } catch (error) {
      observed = error;
    }
    if (!(observed instanceof Error)) {
      throw new Error(`expected Error, got ${String(observed)}`);
    }
    const message = String(observed.message || observed);
    if (!message.includes("ENOENT") && !message.toLowerCase().includes("spawn")) {
      throw new Error(`unexpected transport error message: ${message}`);
    }
  });

  await check("async factory fromHttp status success", async () => {
    let observedCall = null;
    const factoryClient = MetaAsyncClient.fromHttp("https://meta.local/rpc", {
      headers: { "x-sdk-productization": "true" },
      fetchImpl: async (url, init = {}) => {
        observedCall = { url, init };
        const payload = JSON.parse(String(init.body || "{}"));
        return {
          ok: true,
          status: 200,
          text: async () => JSON.stringify({
            jsonrpc: "2.0",
            id: payload.id,
            result: {
              contract_version: EXPECTED_CONTRACT_VERSION,
              running: true,
              loaded_modules: ["routing", "delivery"],
            },
          }),
        };
      },
    });

    const status = await factoryClient.status("ts-prod-factory-http-status");
    assertDeepEqual(status, {
      contract_version: EXPECTED_CONTRACT_VERSION,
      running: true,
      loaded_modules: ["routing", "delivery"],
    }, "unexpected fromHttp status");

    if (!observedCall || observedCall.url !== "https://meta.local/rpc") {
      throw new Error(`unexpected fromHttp endpoint: ${JSON.stringify(observedCall)}`);
    }
    if (!observedCall.init || observedCall.init.method !== "POST") {
      throw new Error(`unexpected fromHttp request method: ${JSON.stringify(observedCall)}`);
    }
    const headers = observedCall.init.headers || {};
    if ((headers["x-sdk-productization"] || headers["X-Sdk-Productization"]) !== "true") {
      throw new Error(`missing custom header in fromHttp request: ${JSON.stringify(headers)}`);
    }
    const payload = JSON.parse(String(observedCall.init.body || "{}"));
    if (payload.method !== "meta/status") {
      throw new Error(`unexpected fromHttp rpc payload: ${JSON.stringify(payload)}`);
    }
  });

  await check("async factory fromHttp transport errors surface", async () => {
    const factoryClient = MetaAsyncClient.fromHttp("https://meta.local/rpc", {
      fetchImpl: async () => ({
        ok: false,
        status: 503,
        text: async () => "service unavailable",
      }),
    });

    let observed = null;
    try {
      await factoryClient.status("ts-prod-factory-http-error");
    } catch (error) {
      observed = error;
    }
    if (!(observed instanceof Error)) {
      throw new Error(`expected Error, got ${String(observed)}`);
    }
    const message = String(observed.message || observed);
    if (!message.includes("status=503") || !message.includes("service unavailable")) {
      throw new Error(`unexpected transport error message: ${message}`);
    }
  });

  await check("console route helpers expose modules and experience routes", async () => {
    const modules = buildConsoleModulesRoute("token+/=?");
    const experience = buildConsoleExperienceRoute("token+/=?");
    const routes = buildConsoleRoutes("token+/=?");
    const explicit = buildConsoleRoute("/console/modules", "token+/=?");

    if (modules !== "/console/modules?auth_token=token%2B%2F%3D%3F") {
      throw new Error(`unexpected modules route: ${modules}`);
    }
    if (experience !== "/console/experience?auth_token=token%2B%2F%3D%3F") {
      throw new Error(`unexpected experience route: ${experience}`);
    }
    if (explicit !== modules) {
      throw new Error(`explicit route helper mismatch: ${explicit} vs ${modules}`);
    }
    assertDeepEqual(routes, { modules, experience }, "unexpected route map");
  });

  await check("module authoring helpers support base structures and decorators", async () => {
    const baseSpec = defineModuleSpec({
      id: "routing",
      command: "node",
      args: ["routing.js"],
      restartPolicy: "never",
      boundary: "mcp",
    });

    const decoratedSpec = decorateModuleSpec(
      baseSpec,
      (spec) => ({ ...spec, restart_policy: "on_failure" }),
      (spec) => ({ ...spec, args: [...spec.args, "--prod"] }),
    );

    const tool = defineModuleTool({
      name: "health",
      description: "returns module health",
      decorators: [
        (next) => async (input, context) => {
          const result = await next(input, context);
          return { ...result, decorated: true };
        },
      ],
      handler: async (input, context) => ({
        moduleId: context.moduleId,
        requestId: context.requestId,
        probe: input.probe,
      }),
    });

    const moduleDefinition = defineModule({
      spec: decoratedSpec,
      description: "routing module",
      tools: [tool],
    });

    const toolResult = await moduleDefinition.tools[0].handler(
      { probe: "ready" },
      { moduleId: "routing", requestId: "tool-1" },
    );

    assertDeepEqual(moduleDefinition.spec, {
      id: "routing",
      command: "node",
      args: ["routing.js", "--prod"],
      restart_policy: "on_failure",
      boundary: "mcp",
    }, "unexpected decorated spec");

    assertDeepEqual(toolResult, {
      moduleId: "routing",
      requestId: "tool-1",
      probe: "ready",
      decorated: true,
    }, "unexpected decorated tool result");
  });

  const failed = results.filter((item) => !item.ok).length;
  const passed = results.length - failed;
  const summary = {
    sdk: "typescript",
    suite: "productization",
    passed,
    failed,
    checks: results,
  };

  process.stdout.write(JSON.stringify(summary));
  process.exit(failed === 0 ? 0 : 1);
}

run().catch((error) => {
  const message = String(error && error.stack ? error.stack : error);
  process.stderr.write(`${message}\n`);
  process.exit(1);
});
