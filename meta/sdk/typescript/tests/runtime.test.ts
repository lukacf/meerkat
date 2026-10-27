/**
 * Tests for MetaRuntime and MobHandle with a mock RPC transport.
 */

import { describe, it, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  Meta,
  MetaRuntime,
  MobHandle,
  ToolCaller,
  CapabilityUnavailableError,
  NotConnectedError,
  RpcError,
  TransportError,
} from "../dist/index.js";
import type { AgentEvent } from "../dist/index.js";
import { runtimeConfig } from "./runtime-config.js";

/**
 * A stand-in gateway that answers every JSON-RPC request on stdin with
 * `result` (twin of the Python `_write_echo_gateway`). The transport spawns
 * `gatewayBin` directly, so the stand-in is a `sh` wrapper around a CommonJS
 * script run by the current node binary (a shebang would break on a
 * node path containing spaces).
 */
function writeEchoGateway(dir: string, result: Record<string, unknown>): string {
  const script = join(dir, "echo_gateway.cjs");
  writeFileSync(
    script,
    [
      'const readline = require("node:readline");',
      `const RESULT = ${JSON.stringify(result)};`,
      "const rl = readline.createInterface({ input: process.stdin });",
      'rl.on("line", (line) => {',
      "  let request;",
      "  try { request = JSON.parse(line); } catch { return; }",
      '  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: RESULT }) + "\\n");',
      "});",
      'rl.on("close", () => process.exit(0));',
      "",
    ].join("\n"),
  );
  const gateway = join(dir, "echo_gateway.sh");
  writeFileSync(
    gateway,
    `#!/bin/sh\nexec "${process.execPath}" "${script}" "$@"\n`,
    { mode: 0o755 },
  );
  return gateway;
}

// ---------------------------------------------------------------------------
// Mock RPC helper
// ---------------------------------------------------------------------------

interface RpcCall {
  method: string;
  params: Record<string, unknown> | undefined;
}

function createMockRuntime(): {
  rt: MetaRuntime;
  handle: MobHandle;
  calls: RpcCall[];
  setResponse: (
    fn: (method: string, params?: Record<string, unknown>) => unknown,
  ) => void;
} {
  const config = runtimeConfig();

  const rt = new MetaRuntime(config);
  // Mark as running so MobHandle methods don't reject
  (rt as any)._running = true;

  const calls: RpcCall[] = [];
  let responseFn: (
    method: string,
    params?: Record<string, unknown>,
  ) => unknown = () => ({});

  (rt as any)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    calls.push({ method, params });
    return responseFn(method, params);
  };

  const handle = rt.mobHandle();

  return {
    rt,
    handle,
    calls,
    setResponse: (fn) => {
      responseFn = fn;
    },
  };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("MetaRuntime", () => {
  it("mobHandle() returns a MobHandle", () => {
    const { handle } = createMockRuntime();
    assert.ok(handle instanceof MobHandle);
  });

  it("isRunning reflects _running state", () => {
    const { rt } = createMockRuntime();
    assert.equal(rt.isRunning, true);
  });

  it("shutdown sets isRunning to false", async () => {
    const { rt } = createMockRuntime();
    await rt.shutdown();
    assert.equal(rt.isRunning, false);
  });

  it("shutdown awaits persistent transport cleanup", async () => {
    const { rt } = createMockRuntime();
    let releaseStop: (() => void) | undefined;
    let shutdownSettled = false;
    const transport = {
      stop: () =>
        new Promise<void>((resolve) => {
          releaseStop = resolve;
        }),
    };
    (rt as any)._transport = transport;

    const shutdown = rt.shutdown().then(() => {
      shutdownSettled = true;
    });
    await Promise.resolve();

    assert.equal(shutdownSettled, false);
    assert.equal(rt.isRunning, false, "RPC admission closes before the gateway drain completes");
    await assert.rejects(
      (MetaRuntime.prototype as any)._rpc.call(rt, "meta/status", {}),
      NotConnectedError,
    );
    assert.equal(
      (rt as any)._transport,
      transport,
      "the runtime keeps the transport until its stop fulfils",
    );

    assert.ok(releaseStop);
    releaseStop();
    await shutdown;

    assert.equal(shutdownSettled, true);
    assert.equal((rt as any)._transport, null);
  });

  it("coalesces concurrent shutdown and delays reconnect until drain completes", async () => {
    const { rt } = createMockRuntime();
    let releaseStop: (() => void) | undefined;
    let stopCalls = 0;
    const transport = {
      stop: () => {
        stopCalls += 1;
        return new Promise<void>((resolve) => {
          releaseStop = resolve;
        });
      },
    };
    (rt as any)._transport = transport;

    const firstShutdown = rt.shutdown();
    const secondShutdown = rt.shutdown();
    await Promise.resolve();

    assert.equal(stopCalls, 1);
    assert.equal((rt as any)._transport, transport, "kept until its stop fulfils");
    await assert.rejects(
      (MetaRuntime.prototype as any)._rpc.call(rt, "meta/status", {}),
      NotConnectedError,
    );

    let bootstrapCalls = 0;
    (rt as any)._bootstrap = async () => {
      bootstrapCalls += 1;
      (rt as any)._running = true;
    };
    const reconnect = rt.connect();
    await Promise.resolve();
    assert.equal(bootstrapCalls, 0);

    assert.ok(releaseStop);
    releaseStop();
    await Promise.all([firstShutdown, secondShutdown, reconnect]);
    assert.equal(stopCalls, 1);
    assert.equal(bootstrapCalls, 1);
    assert.equal(rt.isRunning, true);
  });

  it("coalesces concurrent connect attempts into one bootstrap", async () => {
    const { rt } = createMockRuntime();
    (rt as any)._running = false;
    (rt as any)._transport = null;

    let markStarted: (() => void) | undefined;
    const started = new Promise<void>((resolve) => {
      markStarted = resolve;
    });
    let releaseBootstrap: (() => void) | undefined;
    const bootstrapGate = new Promise<void>((resolve) => {
      releaseBootstrap = resolve;
    });
    let bootstrapCalls = 0;
    (rt as any)._bootstrap = async () => {
      bootstrapCalls += 1;
      markStarted?.();
      await bootstrapGate;
    };

    const firstConnect = rt.connect();
    const secondConnect = rt.connect();
    await started;

    assert.equal(bootstrapCalls, 1);
    releaseBootstrap?.();
    await Promise.all([firstConnect, secondConnect]);
    assert.equal(bootstrapCalls, 1);
    assert.equal(rt.isRunning, true);
  });

  it("orders shutdown behind an admitted connect and leaves the runtime stopped", async () => {
    const { rt } = createMockRuntime();
    (rt as any)._running = false;
    (rt as any)._transport = null;

    let markStarted: (() => void) | undefined;
    const started = new Promise<void>((resolve) => {
      markStarted = resolve;
    });
    let releaseBootstrap: (() => void) | undefined;
    const bootstrapGate = new Promise<void>((resolve) => {
      releaseBootstrap = resolve;
    });
    let stopCalls = 0;
    const transport = {
      stop: async () => {
        stopCalls += 1;
      },
    };
    (rt as any)._bootstrap = async () => {
      markStarted?.();
      await bootstrapGate;
      (rt as any)._transport = transport;
    };

    const connect = rt.connect();
    await started;
    let shutdownSettled = false;
    const shutdown = rt.shutdown().then(() => {
      shutdownSettled = true;
    });
    await Promise.resolve();

    assert.equal(rt.isRunning, false);
    assert.equal(shutdownSettled, false);
    releaseBootstrap?.();
    await Promise.all([connect, shutdown]);

    assert.equal(stopCalls, 1);
    assert.equal(rt.isRunning, false);
    assert.equal((rt as any)._transport, null);
  });

  it("preserves connect, shutdown, reconnect invocation order", async () => {
    const { rt } = createMockRuntime();
    (rt as any)._running = false;
    (rt as any)._transport = null;

    let markStarted: (() => void) | undefined;
    const started = new Promise<void>((resolve) => {
      markStarted = resolve;
    });
    let releaseFirstBootstrap: (() => void) | undefined;
    const firstBootstrapGate = new Promise<void>((resolve) => {
      releaseFirstBootstrap = resolve;
    });
    let bootstrapCalls = 0;
    let stopCalls = 0;
    (rt as any)._bootstrap = async () => {
      bootstrapCalls += 1;
      if (bootstrapCalls === 1) {
        markStarted?.();
        await firstBootstrapGate;
      }
      (rt as any)._transport = {
        stop: async () => {
          stopCalls += 1;
        },
      };
    };

    const firstConnect = rt.connect();
    await started;
    const shutdown = rt.shutdown();
    const reconnect = rt.connect();
    releaseFirstBootstrap?.();
    await Promise.all([firstConnect, shutdown, reconnect]);

    assert.equal(bootstrapCalls, 2);
    assert.equal(stopCalls, 1);
    assert.equal(rt.isRunning, true);
    assert.notEqual((rt as any)._transport, null);
  });

  it("preserves shutdown, reconnect, shutdown invocation order", async () => {
    const { rt } = createMockRuntime();
    let releaseFirstStop: (() => void) | undefined;
    const firstStopGate = new Promise<void>((resolve) => {
      releaseFirstStop = resolve;
    });
    let stopCalls = 0;
    (rt as any)._transport = {
      stop: async () => {
        stopCalls += 1;
        await firstStopGate;
      },
    };
    let bootstrapCalls = 0;
    (rt as any)._bootstrap = async () => {
      bootstrapCalls += 1;
      (rt as any)._transport = {
        stop: async () => {
          stopCalls += 1;
        },
      };
    };

    const firstShutdown = rt.shutdown();
    const reconnect = rt.connect();
    const finalShutdown = rt.shutdown();
    releaseFirstStop?.();
    await Promise.all([firstShutdown, reconnect, finalShutdown]);

    assert.equal(bootstrapCalls, 1);
    assert.equal(stopCalls, 2);
    assert.equal(rt.isRunning, false);
    assert.equal((rt as any)._transport, null);
  });

  it("rustHttpBaseUrl getter and setter", () => {
    const { rt } = createMockRuntime();
    assert.equal(rt.rustHttpBaseUrl, null);
    rt.setRustHttpBase("http://127.0.0.1:8081");
    assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:8081");
  });

  it("builds console_read_only runtime option", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.consoleReadOnly = true;

    const params = (rt as any)._buildInitParams();

    assert.equal(params.runtime_options.console_read_only, true);
  });

  it("builds access_config_path runtime option", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.accessConfigPath = "config/access.toml";

    const params = (rt as any)._buildInitParams();

    assert.equal(
      params.runtime_options.access_config_path,
      "config/access.toml",
    );
  });

  it("forwards an explicit access_config_path, never drops it", () => {
    // Even an empty explicit path reaches the gateway, which refuses it;
    // only an absent path is omitted.
    const { rt } = createMockRuntime();
    assert.equal(
      "access_config_path" in (rt as any)._buildInitParams().runtime_options,
      false,
    );
    (rt as any)._config.accessConfigPath = "";
    const params = (rt as any)._buildInitParams();
    assert.equal(params.runtime_options.access_config_path, "");
  });

  it("forwards an explicit gating_config_path, never drops it", () => {
    // Even an empty explicit path reaches the gateway, which refuses it;
    // only an absent path is omitted.
    const { rt } = createMockRuntime();
    assert.equal(
      "gating_config_path" in (rt as any)._buildInitParams().runtime_options,
      false,
    );
    (rt as any)._config.gatingConfigPath = "";
    const params = (rt as any)._buildInitParams();
    assert.equal(params.runtime_options.gating_config_path, "");
  });

  it("builds meerkat_config_path runtime option", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.meerkatConfigPath = "/etc/example/config.toml";

    const params = (rt as any)._buildInitParams();

    assert.equal(
      params.runtime_options.meerkat_config_path,
      "/etc/example/config.toml",
    );
  });

  it("omits meerkat_config_path runtime option when unset", () => {
    const { rt } = createMockRuntime();

    const params = (rt as any)._buildInitParams();

    assert.equal("meerkat_config_path" in params.runtime_options, false);
  });

  it("builds provider_credentials_root runtime option", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.providerCredentialsRoot =
      "/srv/meta/users/a/credentials";

    const params = (rt as any)._buildInitParams();

    assert.equal(
      params.runtime_options.provider_credentials_root,
      "/srv/meta/users/a/credentials",
    );
  });

  it("omits provider_credentials_root runtime option when unset", () => {
    const { rt } = createMockRuntime();

    const params = (rt as any)._buildInitParams();

    assert.equal("provider_credentials_root" in params.runtime_options, false);
  });

  it("builds http_listen, http_public_base_url and allow_remote runtime options", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.httpListen = "0.0.0.0:8080";
    (rt as any)._config.httpPublicBaseUrl = "https://mob.example.com";
    (rt as any)._config.allowRemote = true;

    const params = (rt as any)._buildInitParams();

    assert.equal(params.runtime_options.http_listen, "0.0.0.0:8080");
    assert.equal(
      params.runtime_options.http_public_base_url,
      "https://mob.example.com",
    );
    assert.equal(params.runtime_options.allow_remote, true);
  });

  it("transmits allow_remote=false rather than dropping it", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.allowRemote = false;

    const params = (rt as any)._buildInitParams();

    assert.equal(params.runtime_options.allow_remote, false);
  });

  it("omits the http exposure runtime options when unset", () => {
    const { rt } = createMockRuntime();

    const params = (rt as any)._buildInitParams();

    assert.equal("http_listen" in params.runtime_options, false);
    assert.equal("http_public_base_url" in params.runtime_options, false);
    assert.equal("allow_remote" in params.runtime_options, false);
  });

  it("rustHttpPublicBaseUrl prefers the advertised base and falls back to the local one", () => {
    const { rt } = createMockRuntime();
    assert.equal(rt.rustHttpPublicBaseUrl, null);
    rt.setRustHttpBase("http://127.0.0.1:8081");
    assert.equal(rt.rustHttpPublicBaseUrl, "http://127.0.0.1:8081");
    (rt as any)._rustHttpPublicBase = "https://mob.example.com";
    assert.equal(rt.rustHttpPublicBaseUrl, "https://mob.example.com");
    assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:8081");
  });

  // Twin of the Python test_connect_records_local_and_advertised_base_urls:
  // drives the real connect()/_bootstrap path against a stand-in gateway, so
  // the init-result parse block is what sets both base URLs (a test that
  // assigns _rustHttpPublicBase directly cannot notice that block vanishing).
  it("connect() records the local and advertised base URLs from the init result", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-echo-gateway-"));
    const gateway = writeEchoGateway(dir, {
      http_base_url: "http://127.0.0.1:1",
      http_public_base_url: "https://mob.example.com",
    });
    const rt = new MetaRuntime(
      (Meta.builder().gateway(gateway) as any)._config,
    );
    try {
      await rt.connect();
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:1");
      assert.equal(rt.rustHttpPublicBaseUrl, "https://mob.example.com");
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("connect() falls back to the local base when the init result declares no advertised base", async () => {
    const dir = mkdtempSync(join(tmpdir(), "meta-echo-gateway-"));
    const gateway = writeEchoGateway(dir, {
      http_base_url: "http://127.0.0.1:1",
      http_public_base_url: null,
    });
    const rt = new MetaRuntime(
      (Meta.builder().gateway(gateway) as any)._config,
    );
    try {
      await rt.connect();
      assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:1");
      assert.equal(rt.rustHttpPublicBaseUrl, "http://127.0.0.1:1");
    } finally {
      await rt.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("builds agent_memory runtime option", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.agentMemoryConfig = {
      selection: "contextual",
      max_entries: 8,
      recall_timeout_ms: 1200,
      recall_failure_policy: "fail",
    };

    const params = (rt as any)._buildInitParams();

    assert.deepEqual(params.runtime_options.agent_memory, {
      selection: "contextual",
      max_entries: 8,
      recall_timeout_ms: 1200,
      recall_failure_policy: "fail",
    });
  });

  it("builds workgraph runtime option when explicitly set", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.workgraphEnabled = false;

    const params = (rt as any)._buildInitParams();

    assert.equal(params.runtime_options.workgraph, false);
  });

  it("passes a workgraph durable-store directory string through", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.workgraphEnabled = "/var/lib/mob/workgraph";

    const params = (rt as any)._buildInitParams();

    assert.equal(params.runtime_options.workgraph, "/var/lib/mob/workgraph");
  });

  it("omits workgraph runtime option when unset", () => {
    const { rt } = createMockRuntime();

    const params = (rt as any)._buildInitParams();

    assert.equal("workgraph" in params.runtime_options, false);
  });

  it("omits experimental live registration unless explicitly configured", () => {
    const { rt } = createMockRuntime();

    const params = (rt as any)._buildInitParams();

    assert.equal("experimental_live" in params.runtime_options, false);
    assert.equal("openai_live" in params.runtime_options, false);
  });

  it("builds the public openai live registration", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.openaiLiveConfig = {
      principal: "user:luka",
      realm: "example",
      authBinding: { realm: "example", binding: "openai-api-key", profile: "luka" },
      voice: "marin",
      sessionInstructions: "You are a test voice agent.",
    };

    const params = (rt as any)._buildInitParams();

    assert.deepEqual(params.runtime_options.openai_live, {
      principal: "user:luka",
      realm: "example",
      auth_binding: { realm: "example", binding: "openai-api-key", profile: "luka" },
      voice: "marin",
      session_instructions: "You are a test voice agent.",
    });
    assert.equal("experimental_live" in params.runtime_options, false);
  });

  it("builds the explicit experimental live registration", () => {
    const { rt } = createMockRuntime();
    (rt as any)._config.experimentalLiveConfig = {
      principal: "user:luka",
      realm: "example",
      factoryKind: "openai-gpt-live",
      factoryVersion: "v1",
      gate0Qualification: "gate0-v1",
      authBinding: {
        realm: "example",
        binding: "chatgpt-oauth",
        profile: "luka",
      },
      voice: "marin",
      executionProfiles: [
        {
          profileId: "example.device.open-room.v1",
          sessionInstructions: "You are a test voice agent.",
        },
      ],
    };

    const params = (rt as any)._buildInitParams();

    assert.deepEqual(params.runtime_options.experimental_live, {
      principal: "user:luka",
      realm: "example",
      factory_kind: "openai-gpt-live",
      factory_version: "v1",
      gate0_qualification: "gate0-v1",
      auth_binding: {
        realm: "example",
        binding: "chatgpt-oauth",
        profile: "luka",
      },
      voice: "marin",
      execution_profiles: [
        {
          profile_id: "example.device.open-room.v1",
          session_instructions: "You are a test voice agent.",
        },
      ],
    });
  });
});

describe("MobHandle.status()", () => {
  it("sends meta/status and parses the result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: ["mod-a", "mod-b"],
    }));

    const result = await handle.status();
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "meta/status");
    assert.equal(result.contractVersion, "0.5.0");
    assert.equal(result.running, true);
    assert.deepEqual(result.loadedModules, ["mod-a", "mod-b"]);
  });
});

describe("MobHandle.capabilities()", () => {
  it("sends meta/capabilities and parses the result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      contract_version: "0.5.0",
      methods: ["meta/status", "meta/capabilities"],
      loaded_modules: ["mod-a"],
    }));

    const result = await handle.capabilities();
    assert.equal(calls[0].method, "meta/capabilities");
    assert.equal(result.contractVersion, "0.5.0");
    assert.deepEqual(result.methods, ["meta/status", "meta/capabilities"]);
    assert.deepEqual(result.loadedModules, ["mod-a"]);
  });
});

describe("MobHandle.storageDoctor()", () => {
  it("sends meta/storage/doctor with snake_case params and parses the result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      state_dir: "/var/lib/meta/state",
      diagnosis: {
        findings: [
          {
            severity: "error",
            code: "file-name-twins",
            message: "2 spellings of the 'sessions' store exist side by side",
            path: "/var/lib/meta/state/sessions.db",
          },
        ],
        inventory: [{ realm: "state", root: "/var/lib/meta/state" }],
      },
      storage: { blob_durability: "persistent_disk" },
    }));

    const result = await handle.storageDoctor({
      stateDir: "/var/lib/meta/state",
      identity: "domain:security",
    });
    assert.equal(calls[0].method, "meta/storage/doctor");
    assert.deepEqual(calls[0].params, {
      state_dir: "/var/lib/meta/state",
      identity: "domain:security",
    });
    assert.equal(result.stateDir, "/var/lib/meta/state");
    assert.equal(result.findings.length, 1);
    assert.equal(result.findings[0].code, "file-name-twins");
    assert.equal(result.findings[0].severity, "error");
    assert.equal(result.findings[0].path, "/var/lib/meta/state/sessions.db");
    assert.equal(result.findings[0].realm, undefined);
    assert.equal(result.inventory.length, 1);
    assert.deepEqual(result.storage, { blob_durability: "persistent_disk" });
  });

  it("omits params left unset and maps a null storage summary", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      state_dir: "/s",
      diagnosis: { findings: [], inventory: [] },
      storage: null,
    }));

    const result = await handle.storageDoctor();
    assert.deepEqual(calls[0].params, {});
    assert.equal(result.storage, null);
    assert.deepEqual(result.findings, []);
  });
});

describe("MobHandle Rust gateway parity wrappers", () => {
  it("sends session store BigQuery RPC name", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ rows: 1 }));

    const result = await handle.sessionStoreBigQuery({ operation: "probe" });

    assert.equal(calls[0].method, "meta/session_store/bigquery");
    assert.deepEqual(calls[0].params, { operation: "probe" });
    assert.deepEqual(result, { rows: 1 });
  });

  it("sends split Meta editor catalog RPC names", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse((method) => {
      if (method === "meta/tools/catalog") {
        return {
          schema_version: "mobpack.editor.v1",
          runtime_backed: false,
          source: "meta/tool-config",
          tool_catalog: [{ id: "shell" }],
        };
      }
      if (method === "meta/skills/catalog") {
        return {
          schema_version: "mobpack.editor.v1",
          runtime_backed: false,
          source: "meta/authoring-skill-realms",
          skill_realms: [{ id: "meta/authoring" }],
        };
      }
      if (method === "meta/agent_definitions/list") {
        return {
          schema_version: "mobpack.editor.v1",
          runtime_backed: false,
          source: "meta/authoring-agent-definitions",
          agent_definitions: [{ id: "authoring:reviewer" }],
        };
      }
      if (method === "meta/mobpacks/templates") {
        return {
          schema_version: "mobpack.editor.v1",
          source: "meta/mobpack-templates",
          blank_mobpack: { document: {} },
          sample_mobpacks: [{ id: "sample" }],
          sample_agent_definitions: [{ id: "sample:reviewer" }],
          templates: { blank_mobpack: { document: {} } },
        };
      }
      return {
        schema_version: "mobpack.editor.v1",
        runtime_backed: false,
        sources: { tools: "meta/tools/catalog" },
        templates: {},
        tool_catalog: [{ id: "shell" }],
        skill_realms: [{ id: "meta/authoring" }],
        blank_mobpack: { document: {} },
        sample_mobpacks: [{ id: "sample" }],
        agent_definitions: [{ id: "authoring:reviewer" }],
        sample_agent_definitions: [{ id: "sample:reviewer" }],
        models: [],
        provider_defaults: [],
      };
    });

    assert.deepEqual((await handle.toolsCatalog()).toolCatalog, [{ id: "shell" }]);
    assert.deepEqual((await handle.skillsCatalog()).skillRealms, [{ id: "meta/authoring" }]);
    assert.deepEqual((await handle.agentDefinitions()).agentDefinitions, [{ id: "authoring:reviewer" }]);
    assert.deepEqual((await handle.mobpackTemplates()).sampleMobpacks, [{ id: "sample" }]);
    assert.deepEqual((await handle.mobpackCatalogs()).sources, { tools: "meta/tools/catalog" });
    assert.deepEqual(calls.map((call) => call.method), [
      "meta/tools/catalog",
      "meta/skills/catalog",
      "meta/agent_definitions/list",
      "meta/mobpacks/templates",
      "meta/mobpacks/catalogs",
    ]);
  });
});

describe("MobHandle mobpack authoring wrappers", () => {
  const validationPayload = {
    ok: true,
    diagnostics: [],
    display_rows: [
      { kind: "ok", glyph: "✓", head: "valid", sub: "", meta: "" },
    ],
    mob_id: "demo",
    flow_ids: ["flow_a"],
    validation_source: "meta/mobpacks/validate",
    deploy_command: "rkat mob deploy",
  };

  const draftRowPayload = (revision = 1, canUndo = false, canRedo = false) => ({
    id: "f_demo",
    name: "Demo",
    version: "mobpack.editor.v1",
    stage: "draft",
    trigger: "Meta authoring draft",
    source: "meta/mobpacks/create",
    revision,
    etag: `f_demo:${revision}`,
    updated_at_unix_ms: 1700000000000,
    document: { mob_id: "demo" },
    validation: { ok: true },
    can_undo: canUndo,
    can_redo: canRedo,
  });

  function setMobpackResponses(
    setResponse: (
      fn: (method: string, params?: Record<string, unknown>) => unknown,
    ) => void,
  ): void {
    const responses: Record<string, unknown> = {
      "meta/mobpacks/validate": validationPayload,
      "meta/mobpacks/source": {
        filename: "demo.mobpack",
        media_type: "application/vnd.meerkat.mobpack",
        mob_toml: '[mob]\nid = "demo"\n',
        source_files: [
          {
            path: "mob.toml",
            media_type: "text/x-toml",
            size_bytes: 18,
            content_base64: "W21vYl0=",
            sha256: "abc",
          },
        ],
        validation: validationPayload,
        source: "meta/mobpacks/source",
      },
      "meta/mobpacks/export": {
        filename: "demo.mobpack",
        media_type: "application/vnd.meerkat.mobpack",
        content_base64: "UEsDBA==",
        mob_toml: '[mob]\nid = "demo"\n',
        source_files: [],
        validation: validationPayload,
      },
      "meta/mobpacks/import": {
        document: { mob_id: "demo" },
        validation: validationPayload,
        source: "meta/mobpacks/import:mob.toml",
        source_label: "demo.toml",
        source_media_type: "text/x-toml",
      },
      "meta/mobpacks/list": {
        source: "meta/mobpacks/list",
        store_path: "/tmp/drafts.json",
        runtime_backed: false,
        rows: [draftRowPayload()],
      },
      "meta/mobpacks/get": {
        source: "meta/mobpacks/get",
        store_path: "/tmp/drafts.json",
        runtime_backed: false,
        row: draftRowPayload(),
      },
      "meta/mobpacks/create": {
        source: "meta/mobpacks/create",
        store_path: "/tmp/drafts.json",
        row: draftRowPayload(),
        rows: [draftRowPayload()],
      },
      "meta/mobpacks/save": {
        source: "meta/mobpacks/save",
        store_path: "/tmp/drafts.json",
        row: draftRowPayload(2),
        rows: [draftRowPayload(2)],
      },
      "meta/mobpacks/undo": {
        source: "meta/mobpacks/undo",
        store_path: "/tmp/drafts.json",
        stepped: true,
        row: draftRowPayload(3, false, true),
        rows: [draftRowPayload(3, false, true)],
      },
      "meta/mobpacks/redo": {
        source: "meta/mobpacks/redo",
        store_path: "/tmp/drafts.json",
        stepped: false,
        reason: "nothing to redo",
        row: draftRowPayload(3, false, true),
        rows: [draftRowPayload(3, false, true)],
      },
      "meta/mobpacks/delete": {
        source: "meta/mobpacks/delete",
        store_path: "/tmp/drafts.json",
        id: "f_demo",
        deleted: true,
        rows: [],
      },
      "meta/mobpacks/apply_operation": {
        source: "meta/mobpacks/apply_operation",
        operation: "add_member",
        ok: true,
        document: { mob_id: "demo", members: [{ id: "reviewer" }] },
        selection: { kind: "agent", id: "reviewer" },
        validation: validationPayload,
      },
      "meta/mobpacks/deploy_command": {
        command: "rkat mob deploy demo.mobpack",
        argv: ["rkat", "mob", "deploy", "demo.mobpack"],
        deploy_command: "rkat mob deploy",
        filename: "demo.mobpack",
        validation: validationPayload,
        source: "meerkat_meta::mobpack::deploy_argv",
      },
      "meta/mobpacks/deploy": {
        filename: "demo.mobpack",
        pack_path: "/tmp/demo.mobpack",
        pack_sha256: "deadbeef",
        command: "rkat mob deploy /tmp/demo.mobpack",
        argv: ["rkat", "mob", "deploy", "/tmp/demo.mobpack"],
        plan_trace: [{ step: "validate" }],
        executed: false,
        success: false,
        validation: validationPayload,
        display_rows: [],
      },
    };
    setResponse((method) => responses[method]);
  }

  it("sends mobpack authoring RPC names with snake_case params", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setMobpackResponses(setResponse);
    const document = { mob_id: "demo", members: [] };

    const validation = await handle.mobpackValidate(document, true);
    assert.equal(validation.ok, true);
    assert.equal(validation.deployCommand, "rkat mob deploy");
    assert.equal(validation.displayRows[0].kind, "ok");
    assert.deepEqual(validation.flowIds, ["flow_a"]);

    const source = await handle.mobpackSource(document);
    assert.equal(source.sourceFiles[0].path, "mob.toml");
    assert.equal(source.validation.ok, true);

    const exported = await handle.mobpackExport(document);
    assert.equal(exported.contentBase64, "UEsDBA==");
    assert.equal(exported.filename, "demo.mobpack");

    const imported = await handle.mobpackImport({
      mobToml: '[mob]\nid = "demo"\n',
      sourceName: "demo.toml",
    });
    assert.deepEqual(imported.document, { mob_id: "demo" });
    assert.equal(imported.source, "meta/mobpacks/import:mob.toml");

    const listed = await handle.mobpackList();
    assert.equal(listed.rows[0].id, "f_demo");
    assert.equal(listed.rows[0].revision, 1);
    assert.equal(listed.storePath, "/tmp/drafts.json");

    const got = await handle.mobpackGet("f_demo");
    assert.equal(got.row.etag, "f_demo:1");
    assert.equal(got.row.stage, "draft");

    const created = await handle.mobpackCreate({
      template: "blank",
      name: "Demo",
    });
    assert.equal(created.row.id, "f_demo");

    const saved = await handle.mobpackSave("f_demo", document, {
      stage: "draft",
      expectedRevision: 1,
      expectedEtag: "f_demo:1",
    });
    assert.equal(saved.row.revision, 2);

    const undone = await handle.mobpackUndo("f_demo", {
      expectedRevision: 2,
      expectedEtag: "f_demo:2",
    });
    assert.equal(undone.stepped, true);
    assert.equal(undone.reason, null);
    assert.equal(undone.row.revision, 3);
    assert.equal(undone.row.canUndo, false);
    assert.equal(undone.row.canRedo, true);

    const redone = await handle.mobpackRedo("f_demo");
    assert.equal(redone.stepped, false);
    assert.equal(redone.reason, "nothing to redo");
    assert.equal(redone.row.etag, "f_demo:3");

    const deleted = await handle.mobpackDelete("f_demo", 2);
    assert.equal(deleted.deleted, true);
    assert.equal(deleted.id, "f_demo");

    const applied = await handle.mobpackApplyOperation(
      document,
      { type: "add_member", member: { id: "reviewer" } },
      "snap-1",
    );
    assert.equal(applied.ok, true);
    assert.deepEqual(applied.selection, { kind: "agent", id: "reviewer" });
    assert.equal(applied.validation.ok, true);

    const preview = await handle.mobpackDeployCommand(document);
    assert.deepEqual(preview.argv, ["rkat", "mob", "deploy", "demo.mobpack"]);

    const deployed = await handle.mobpackDeploy(document, false);
    assert.equal(deployed.executed, false);
    assert.equal(deployed.packSha256, "deadbeef");

    assert.deepEqual(calls.map((call) => call.method), [
      "meta/mobpacks/validate",
      "meta/mobpacks/source",
      "meta/mobpacks/export",
      "meta/mobpacks/import",
      "meta/mobpacks/list",
      "meta/mobpacks/get",
      "meta/mobpacks/create",
      "meta/mobpacks/save",
      "meta/mobpacks/undo",
      "meta/mobpacks/redo",
      "meta/mobpacks/delete",
      "meta/mobpacks/apply_operation",
      "meta/mobpacks/deploy_command",
      "meta/mobpacks/deploy",
    ]);
    assert.deepEqual(calls[0].params, { document, rkat_validate: true });
    assert.deepEqual(calls[1].params, { document });
    assert.deepEqual(calls[2].params, { document });
    assert.deepEqual(calls[3].params, {
      mob_toml: '[mob]\nid = "demo"\n',
      source_name: "demo.toml",
    });
    assert.deepEqual(calls[4].params, {});
    assert.deepEqual(calls[5].params, { id: "f_demo" });
    assert.deepEqual(calls[6].params, { template: "blank", name: "Demo" });
    assert.deepEqual(calls[7].params, {
      id: "f_demo",
      document,
      stage: "draft",
      expected_revision: 1,
      expected_etag: "f_demo:1",
    });
    assert.deepEqual(calls[8].params, {
      id: "f_demo",
      expected_revision: 2,
      expected_etag: "f_demo:2",
    });
    assert.deepEqual(calls[9].params, { id: "f_demo" });
    assert.deepEqual(calls[10].params, { id: "f_demo", expected_revision: 2 });
    assert.deepEqual(calls[11].params, {
      document,
      operation: { type: "add_member", member: { id: "reviewer" } },
      expected_catalog_snapshot_id: "snap-1",
    });
    assert.deepEqual(calls[12].params, { document });
    assert.deepEqual(calls[13].params, { document, execute: false });
  });

  it("omits optional params when not provided", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setMobpackResponses(setResponse);

    await handle.mobpackValidate({ mob_id: "demo" });
    await handle.mobpackCreate();
    await handle.mobpackDelete("f_demo");
    await handle.mobpackUndo("f_demo");
    await handle.mobpackRedo("f_demo");
    await handle.mobpackDeploy({ mob_id: "demo" });

    assert.deepEqual(calls[0].params, { document: { mob_id: "demo" } });
    assert.deepEqual(calls[1].params, {});
    assert.deepEqual(calls[2].params, { id: "f_demo" });
    assert.deepEqual(calls[3].params, { id: "f_demo" });
    assert.deepEqual(calls[4].params, { id: "f_demo" });
    assert.deepEqual(calls[5].params, { document: { mob_id: "demo" } });
  });
});

describe("MobHandle.spawn()", () => {
  it("sends meta/spawn_member with discovery spec", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      module_id: "mod-x",
      agent_identity: "m-1",
      role: "assistant",
    }));

    const result = await handle.spawn({
      role: "assistant",
      agentIdentity: "m-1",
      labels: { role: "helper" },
    });

    assert.equal(calls[0].method, "meta/spawn_member");
    assert.equal(calls[0].params!.role, "assistant");
    assert.equal(calls[0].params!.agent_identity, "m-1");
    assert.deepEqual(calls[0].params!.labels, { role: "helper" });
    assert.equal(result.accepted, true);
    assert.equal(result.moduleId, "mod-x");
    assert.equal(result.agentIdentity, "m-1");
    assert.equal(result.role, "assistant");
  });
});

describe("MobHandle.spawnMember()", () => {
  it("sends meta/spawn_member with module_id", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      module_id: "mod-y",
      agent_identity: null,
      role: null,
    }));

    const result = await handle.spawnMember("mod-y");
    assert.equal(calls[0].method, "meta/spawn_member");
    assert.deepEqual(calls[0].params, { module_id: "mod-y" });
    assert.equal(result.accepted, true);
    assert.equal(result.moduleId, "mod-y");
    assert.equal(result.agentIdentity, null);
    assert.equal(result.role, null);
  });
});

describe("MobHandle.reconcile()", () => {
  it("sends meta/reconcile with modules array", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      reconciled_modules: ["mod-a", "mod-b"],
      added: 2,
    }));

    const result = await handle.reconcile(["mod-a", "mod-b"]);
    assert.equal(calls[0].method, "meta/reconcile");
    assert.deepEqual(calls[0].params, { modules: ["mod-a", "mod-b"] });
    assert.equal(result.accepted, true);
    assert.deepEqual(result.reconciledModules, ["mod-a", "mod-b"]);
    assert.equal(result.added, 2);
  });
});

describe("MobHandle.subscribeEvents()", () => {
  it("sends meta/events/subscribe with scope", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      scope: "mob",
      replay_from_event_id: null,
      keep_alive: { interval_ms: 30000, event: "ping" },
      keep_alive_comment: "keep-alive",
      event_frames: ["frame-1"],
      events: [
        {
          event_id: "e-1",
          source: "system",
          timestamp_ms: 1000,
          event: { type: "test" },
        },
      ],
    }));

    const result = await handle.subscribeEvents("mob", "evt-0", "agent-1");
    assert.equal(calls[0].method, "meta/events/subscribe");
    assert.equal(calls[0].params!.scope, "mob");
    assert.equal(calls[0].params!.last_event_id, "evt-0");
    assert.equal(calls[0].params!.agent_id, "agent-1");
    assert.equal(result.scope, "mob");
    assert.equal(result.replayFromEventId, null);
    assert.equal(result.keepAlive.intervalMs, 30000);
    assert.equal(result.keepAlive.event, "ping");
    assert.equal(result.events.length, 1);
    assert.equal(result.events[0].eventId, "e-1");
  });

  it("defaults scope to mob with no optional params", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      scope: "mob",
      replay_from_event_id: null,
      keep_alive: { interval_ms: 30000, event: "ping" },
      keep_alive_comment: "",
      event_frames: [],
      events: [],
    }));

    await handle.subscribeEvents();
    assert.deepEqual(calls[0].params, { scope: "mob" });
  });
});

describe("MobHandle native review subscription compatibility", () => {
  for (const surface of ["agent", "mob"] as const) {
    for (const approvalId of [undefined, "B6503200-06B6-4157-AFC0-2A3C8D124178"]) {
      it(`${surface} subscription preserves ${approvalId ? "referenced" : "kind-only"} review feedback and subsequent events`, async () => {
        const { rt, handle, calls } = createMockRuntime();
        const memberId = "native-review-agent";
        const callId = "  native-review-call  ";
        const refusal = JSON.stringify({
          error: "review_unsatisfied",
          message: "operation review remains unsatisfied",
          data: {
            kind: approvalId ? "human_consent_required" : "denied",
            ...(approvalId === undefined ? {} : { approval_id: approvalId }),
          },
        });
        const unavailable = JSON.stringify({
          error: "review_unavailable",
          message: "operation reviewer is not installed",
          data: { kind: "reviewer_missing" },
        });
        const modelNotices = [
          { code: "REVIEW_UNSATISFIED", review: { review: "unsatisfied", kind: "denied" } },
          { code: "REVIEW_UNAVAILABLE", review: { review: "unavailable", kind: "reviewer_missing" } },
        ].map((payload) => ({
          type: "system_notice",
          message: {
            role: "system_notice", kind: "generic", body: "Review feedback",
            blocks: [{ type: "runtime_notice", category: "operation_review_required", detail: null, payload }],
          },
          future_detail: { version: 99, values: [null, "opaque"] },
        }));
        // This unknown event is deliberately not a native consent state. Its
        // label/reference must not become a decision or an execution result.
        const unknown = {
          type: "future_review_diagnostic", status: "Approved",
          approval_id: "B6503200-06B6-4157-AFC0-2A3C8D124178",
          future_detail: { values: [null, { label: "opaque" }] },
        };
        // `result` is the Meta projection of native tool content on the result
        // event; the execution fact for the same call carries no body.
        // The optional reference is fixture wire data, not proof of issuance.
        const payloads = [
          { type: "run_started", session_id: "native-review-session", prompt: "Use permitted work" },
          { type: "tool_execution_completed", id: callId, name: "write_file", is_error: true, duration_ms: 0 },
          { type: "tool_result_received", id: callId, name: "write_file",
            content: [{ type: "text", text: refusal }], result: refusal, is_error: true },
          { type: "tool_result_received", id: "unavailable-call", name: "write_file",
            content: [{ type: "text", text: unavailable }], result: unavailable, is_error: true },
          ...modelNotices,
          unknown,
          { type: "tool_execution_completed", id: "permitted-call", name: "read_file", is_error: false, duration_ms: 1 },
          { type: "tool_result_received", id: "permitted-call", name: "read_file",
            content: [{ type: "text", text: "permitted sibling" }], result: "permitted sibling", is_error: false },
          { type: "text_delta", delta: "Continuing with permitted work." },
          { type: "run_completed", session_id: "native-review-session", result: "Actual terminal packet" },
        ];
        const originalPayloads = JSON.stringify(payloads);
        const subscriptions: Array<{ scope: string; memberId?: string }> = [];
        let closedSources = 0;
        // Only the source is a fixture. Both public methods and their actual
        // agent/mob envelope parsers run; this does not exercise HTTP/native execution.
        async function* source(scope: "agent" | "mob", requestedMember?: string) {
          subscriptions.push({ scope, ...(requestedMember === undefined ? {} : { memberId: requestedMember }) });
          try {
            for (const [index, payload] of payloads.entries()) {
              yield {
                id: String(index), event: payload.type,
                data: JSON.stringify(scope === "agent" ? payload : {
                  member_id: memberId, timestamp_ms: 1700000000000 + index, payload,
                }),
              };
            }
          } finally {
            closedSources += 1;
          }
        }
        const bridge = rt.sseBridge();
        bridge.agentEvents = (id) => source("agent", id);
        bridge.mobEvents = () => source("mob");
        rt.sseBridge = () => bridge;

        const events: AgentEvent[] = [];
        const stream = surface === "agent" ? handle.subscribeAgent(memberId) : handle.subscribeMob();
        for await (const envelope of stream) {
          assert.equal("memberId" in envelope, surface === "mob");
          if ("memberId" in envelope) {
            assert.equal(envelope.memberId, memberId);
            assert.equal(envelope.timestampMs, 1700000000000 + events.length);
          } else {
            assert.equal(envelope.eventType, payloads[events.length].type);
          }
          events.push(envelope.event);
        }

        assert.deepEqual(events.map((event) => event.type), payloads.map((payload) => payload.type));
        assert.deepEqual(events[1], { type: "tool_execution_completed", id: callId, name: "write_file", isError: true, durationMs: 0 });
        const blocked = events[2];
        assert.ok("id" in blocked && "result" in blocked && "isError" in blocked);
        assert.deepEqual([blocked.id, blocked.result, blocked.isError], [callId, refusal, true]);
        const detail = JSON.parse(blocked.result).data;
        assert.equal(Object.hasOwn(detail, "approval_id"), approvalId !== undefined);
        assert.equal(detail.approval_id, approvalId);
        const unavailableResult = events[3];
        assert.ok("result" in unavailableResult && "isError" in unavailableResult);
        assert.deepEqual([unavailableResult.result, unavailableResult.isError], [unavailable, true]);
        for (const [index, notice] of [...modelNotices, unknown].entries()) {
          const projected = events[4 + index];
          assert.ok("data" in projected);
          assert.deepEqual(projected.data, notice);
        }
        assert.deepEqual(events[7], { type: "tool_execution_completed", id: "permitted-call", name: "read_file",
          isError: false, durationMs: 1 });
        assert.deepEqual(events[8], { type: "tool_result_received", id: "permitted-call", name: "read_file",
          result: "permitted sibling", isError: false });
        assert.deepEqual(events[9], { type: "text_delta", delta: "Continuing with permitted work." });
        assert.deepEqual(events[10], { type: "run_completed", sessionId: "native-review-session", result: "Actual terminal packet" });
        assert.equal(events.some((event) => event.type === "run_failed"), false);
        assert.deepEqual(subscriptions, [surface === "agent" ? { scope: "agent", memberId } : { scope: "mob" }]);
        assert.equal(closedSources, 1);
        assert.deepEqual(calls, []); // No automatic approval, retry, send or other RPC.
        assert.equal(JSON.stringify(payloads), originalPayloads);
        assert.equal(rt.isRunning, true);
      });
    }
  }
});

describe("MobHandle.send()", () => {
  it("sends meta/send_message and parses result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      member_id: "m-1",
      session_id: "sess-1",
    }));

    const result = await handle.send("m-1", "Hello!");
    assert.equal(calls[0].method, "meta/send_message");
    assert.deepEqual(calls[0].params, { member_id: "m-1", message: "Hello!" });
    assert.equal(result.accepted, true);
    assert.equal(result.memberId, "m-1");
    assert.equal(result.sessionId, "sess-1");
  });

  it("sends image content blocks with strict source shape", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      member_id: "m-1",
      session_id: "sess-1",
    }));

    await handle.send("m-1", [
      { type: "image", mediaType: "image/png", data: "abc" },
    ]);

    assert.deepEqual(calls[0].params, {
      member_id: "m-1",
      content: [
        {
          type: "image",
          media_type: "image/png",
          source: "inline",
          data: "abc",
        },
      ],
    });
  });

  it("uses multipart RPC for attachments", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async (
        url: string | URL | Request,
        init?: RequestInit,
      ) => {
        assert.equal(
          String(url),
          "http://127.0.0.1:8765/console/rpc/multipart",
        );
        const form = init?.body as FormData;
        const payload = JSON.parse(String(form.get("payload"))) as Record<
          string,
          any
        >;
        assert.equal(payload.method, "meta/send_message");
        assert.deepEqual(payload.params, {
          member_id: "m-1",
          content: [
            { type: "text", text: "See attached" },
            {
              type: "image_upload",
              upload_id: "upload-1",
              media_type: "image/png",
            },
          ],
        });
        const file = form.get("file:upload-1") as Blob;
        assert.equal(file.type, "image/png");
        return new Response(
          JSON.stringify({
            jsonrpc: "2.0",
            id: payload.id,
            result: { accepted: true, member_id: "m-1", session_id: "sess-1" },
          }),
          { headers: { "content-type": "application/json" } },
        );
      }) as typeof fetch;

      const result = await handle.send("m-1", "See attached", {
        attachments: [new Blob(["png"], { type: "image/png" })],
      });
      assert.equal(result.accepted, true);
      assert.equal(result.sessionId, "sess-1");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("appends attachments after structured content and forwards handling mode", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async (
        _url: string | URL | Request,
        init?: RequestInit,
      ) => {
        const form = init?.body as FormData;
        const payload = JSON.parse(String(form.get("payload"))) as Record<
          string,
          any
        >;
        assert.equal(payload.params.handling_mode, "steer");
        assert.deepEqual(payload.params.content, [
          { type: "text", text: "first" },
          {
            type: "image_upload",
            upload_id: "upload-1",
            media_type: "image/jpeg",
            alt: "photo",
          },
        ]);
        return new Response(
          JSON.stringify({
            jsonrpc: "2.0",
            id: payload.id,
            result: { accepted: true, member_id: "m-1", session_id: "sess-2" },
          }),
          { headers: { "content-type": "application/json" } },
        );
      }) as typeof fetch;

      const result = await handle.send(
        "m-1",
        [{ type: "text", text: "first" }],
        {
          handlingMode: "steer",
          attachments: [
            {
              blob: new Blob(["jpg"], { type: "image/jpeg" }),
              alt: "photo",
              filename: "photo.jpg",
            },
          ],
        },
      );
      assert.equal(result.sessionId, "sess-2");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("requires rustHttpBaseUrl before multipart send", async () => {
    const { handle } = createMockRuntime();
    await assert.rejects(
      () =>
        handle.send("m-1", "x", {
          attachments: [new Blob(["x"], { type: "image/png" })],
        }),
      NotConnectedError,
    );
  });
});

describe("MobHandle.sendMessage()", () => {
  it("is an alias for send()", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      accepted: true,
      member_id: "m-2",
      session_id: "sess-2",
    }));

    const result = await handle.sendMessage("m-2", "Hi");
    assert.equal(calls[0].method, "meta/send_message");
    assert.deepEqual(calls[0].params, { member_id: "m-2", message: "Hi" });
    assert.equal(result.accepted, true);
    assert.equal(result.memberId, "m-2");
  });
});

describe("MobHandle.uploadBlob()", () => {
  it("uploads one blob through multipart RPC", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765/");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async (
        _url: string | URL | Request,
        init?: RequestInit,
      ) => {
        const form = init?.body as FormData;
        const payload = JSON.parse(String(form.get("payload"))) as Record<
          string,
          any
        >;
        assert.equal(payload.method, "meta/blob/upload");
        assert.deepEqual(payload.params, {
          upload: {
            type: "image_upload",
            upload_id: "upload-1",
            media_type: "image/webp",
          },
        });
        return new Response(
          JSON.stringify({
            jsonrpc: "2.0",
            id: payload.id,
            result: {
              blob_id: "sha256:abc",
              media_type: "image/webp",
              size: 3,
            },
          }),
          { headers: { "content-type": "application/json" } },
        );
      }) as typeof fetch;

      const result = await handle.uploadBlob(
        new Blob(["web"], { type: "image/webp" }),
      );
      assert.equal(result.blobId, "sha256:abc");
      assert.equal(result.mediaType, "image/webp");
      assert.equal(result.size, 3);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("exposes upload_blob alias", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async (_url: string | URL | Request) =>
        new Response(
          JSON.stringify({
            jsonrpc: "2.0",
            id: "upload",
            result: { blob_id: "sha256:def", media_type: "image/png", size: 1 },
          }),
          { headers: { "content-type": "application/json" } },
        )) as typeof fetch;

      const result = await handle.upload_blob(
        new Blob(["x"], { type: "image/png" }),
      );
      assert.equal(result.blobId, "sha256:def");
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("maps multipart JSON-RPC errors to RpcError", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async (_url: string | URL | Request) =>
        new Response(
          JSON.stringify({
            jsonrpc: "2.0",
            id: "upload",
            error: {
              code: -32602,
              message: "bad upload",
              data: { reason: "test" },
            },
          }),
          { headers: { "content-type": "application/json" } },
        )) as typeof fetch;

      await assert.rejects(
        () => handle.uploadBlob(new Blob(["x"], { type: "image/png" })),
        (err: unknown) =>
          err instanceof RpcError &&
          err.code === -32602 &&
          err.method === "meta/blob/upload",
      );
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("maps multipart non-JSON HTTP failures to TransportError", async () => {
    const { rt, handle } = createMockRuntime();
    rt.setRustHttpBase("http://127.0.0.1:8765");
    const originalFetch = globalThis.fetch;
    try {
      globalThis.fetch = (async () =>
        new Response("payload too large", {
          status: 413,
          statusText: "Payload Too Large",
          headers: { "content-type": "text/plain" },
        })) as typeof fetch;

      await assert.rejects(
        () => handle.uploadBlob(new Blob(["x"], { type: "image/png" })),
        (err: unknown) =>
          err instanceof TransportError &&
          err.message.includes("status=413") &&
          err.message.includes("payload too large"),
      );
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it("requires rustHttpBaseUrl before upload", async () => {
    const { handle } = createMockRuntime();
    await assert.rejects(
      () => handle.uploadBlob(new Blob(["x"], { type: "image/png" })),
      NotConnectedError,
    );
  });
});

describe("MobHandle.ensureMember()", () => {
  it("sends meta/ensure_member with all options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      agent_identity: "m-1",
      role: "assistant",
      state: "active",
      wired_to: ["m-2"],
      labels: { role: "helper" },
    }));

    const result = await handle.ensureMember("m-1", "assistant", {
      labels: { role: "helper" },
      context: { foo: "bar" },
      resumeSessionId: "sess-old",
      additionalInstructions: ["Be nice"],
      runtimeMode: "turn_driven",
      backend: "external",
      binding: {
        kind: "external",
        address: "tcp://127.0.0.1:4799",
        identity: {
          kind: "ed25519_public_key",
          public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        },
      },
    });

    assert.equal(calls[0].method, "meta/ensure_member");
    assert.equal(calls[0].params!.role, "assistant");
    assert.equal(calls[0].params!.agent_identity, "m-1");
    assert.deepEqual(calls[0].params!.labels, { role: "helper" });
    assert.deepEqual(calls[0].params!.context, { foo: "bar" });
    assert.equal(calls[0].params!.resume_session_id, "sess-old");
    assert.deepEqual(calls[0].params!.additional_instructions, ["Be nice"]);
    assert.equal(calls[0].params!.runtime_mode, "turn_driven");
    assert.equal(calls[0].params!.backend, "external");
    assert.deepEqual(calls[0].params!.binding, {
      kind: "external",
      address: "tcp://127.0.0.1:4799",
      identity: {
        kind: "ed25519_public_key",
        public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
      },
    });
    assert.equal(result.agentIdentity, "m-1");
    assert.equal(result.role, "assistant");
    assert.equal(result.state, "active");
    assert.deepEqual(result.wiredTo, ["m-2"]);
    assert.deepEqual(result.labels, { role: "helper" });
  });

  it("sends without optional options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      agent_identity: "m-1",
      role: "assistant",
      state: "active",
      wired_to: [],
      labels: {},
    }));

    await handle.ensureMember("m-1", "assistant");
    assert.equal(calls[0].params!.role, "assistant");
    assert.equal(calls[0].params!.agent_identity, "m-1");
    assert.equal(calls[0].params!.labels, undefined);
    assert.equal(calls[0].params!.context, undefined);
  });
});

describe("MobHandle.findMembers()", () => {
  it("sends meta/find_members and returns array", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => [
      {
        agent_identity: "m-1",
        role: "a",
        state: "active",
        wired_to: [],
        labels: {},
      },
      {
        agent_identity: "m-2",
        role: "b",
        state: "active",
        wired_to: [],
        labels: {},
      },
    ]);

    const result = await handle.findMembers("role", "helper");
    assert.equal(calls[0].method, "meta/find_members");
    assert.deepEqual(calls[0].params, {
      label_key: "role",
      label_value: "helper",
    });
    assert.equal(result.length, 2);
    assert.equal(result[0].agentIdentity, "m-1");
    assert.equal(result[1].agentIdentity, "m-2");
  });

  it("returns empty array when response is not an array", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => null);

    const result = await handle.findMembers("role", "x");
    assert.deepEqual(result, []);
  });
});

describe("MobHandle.listMembers()", () => {
  it("sends meta/list_members and returns array", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => [
      {
        agent_identity: "m-1",
        role: "a",
        state: "active",
        wired_to: [],
        labels: {},
      },
    ]);

    const result = await handle.listMembers();
    assert.equal(calls[0].method, "meta/list_members");
    assert.equal(result.length, 1);
    assert.equal(result[0].agentIdentity, "m-1");
  });

  it("returns empty array when response is not an array", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => "unexpected");

    const result = await handle.listMembers();
    assert.deepEqual(result, []);
  });
});

describe("MobHandle.getMember()", () => {
  it("sends meta/get_member and parses snapshot", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      agent_identity: "m-1",
      role: "assistant",
      state: "active",
      wired_to: ["m-2"],
      labels: { team: "alpha" },
    }));

    const result = await handle.getMember("m-1");
    assert.equal(calls[0].method, "meta/get_member");
    assert.deepEqual(calls[0].params, { member_id: "m-1" });
    assert.equal(result.agentIdentity, "m-1");
    assert.equal(result.role, "assistant");
    assert.deepEqual(result.labels, { team: "alpha" });
  });
});

describe("MobHandle.retireMember()", () => {
  it("sends meta/retire_member", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({}));

    await handle.retireMember("m-1");
    assert.equal(calls[0].method, "meta/retire_member");
    assert.deepEqual(calls[0].params, { member_id: "m-1" });
  });
});

describe("MobHandle.respawnMember()", () => {
  it("sends meta/respawn_member", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({}));

    await handle.respawnMember("m-1");
    assert.equal(calls[0].method, "meta/respawn_member");
    assert.deepEqual(calls[0].params, { member_id: "m-1" });
  });
});

describe("MobHandle.reloadMember()", () => {
  it("sends meta/reload_member and parses the typed result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      reloaded: true,
      disposition: "discarded",
      session_id: "sess-1",
      generation: 4,
      identity: "review:singleton",
      identity_first: true,
    }));

    const result = await handle.reloadMember("review:singleton");
    assert.equal(calls.length, 1, "never a second (respawn) call");
    assert.equal(calls[0].method, "meta/reload_member");
    assert.deepEqual(calls[0].params, { member_id: "review:singleton" });
    assert.deepEqual(result, {
      reloaded: true,
      disposition: "discarded",
      sessionId: "sess-1",
      generation: 4,
      identity: "review:singleton",
    });
  });

  it("degrades gracefully on a minimal not_current result", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({ reloaded: false, disposition: "not_current" }));
    const result = await handle.reloadMember("m-1");
    assert.equal(result.reloaded, false);
    assert.equal(result.disposition, "not_current");
    assert.equal(result.sessionId, null);
    assert.equal(result.generation, null);
  });
});

describe("MobHandle.memberHealth()", () => {
  for (const stage of ["actor_command_admission", "actor_command_reply"]) {
    it(`preserves ${stage} observations without inventing a fate`, async () => {
      const { handle, calls, setResponse } = createMockRuntime();
      const data = {
        kind: "mob_actor_command_timed_out",
        command_kind: "SubmitWork",
        stage,
        deadline_reached: true,
      };
      setResponse(() => ({
        identity: "x",
        state: "active",
        last_delivery_error: { class: "admission_timeout", detail: "unobserved", data, at_unix_ms: 1 },
      }));
      const health = await handle.memberHealth("x");
      assert.deepEqual(health.lastDeliveryError?.data, data);
      assert.equal("executed" in data, false);
      assert.equal("retryable" in data, false);
      calls.length = 0;
      setResponse(() => {
        throw new RpcError(-32603, "observation deadline", "request-1", "meta/send_message", data);
      });
      await assert.rejects(handle.send("x", "turn"), (error: unknown) => {
        assert.ok(error instanceof RpcError);
        assert.deepEqual(error.data, data);
        return true;
      });
      assert.deepEqual(calls.map(call => call.method), ["meta/send_message"]);
    });
  }

  it("sends meta/member_health and parses the report", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      identity: "review:singleton",
      member_id: "rt:review:singleton:4",
      state: "active",
      bootstrap_state: "active",
      materialization_in_flight: false,
      session_id: "sess-1",
      generation: 4,
      last_delivery_error: {
        class: "reload_required",
        detail: "Runtime recovery is repair-blocked",
        at_unix_ms: 1700000000000,
      },
      actor_loop: { state: "stalled", stall_id: 7, stalled_for_secs: 42 },
      open_stall_id: 7,
    }));

    const health = await handle.memberHealth("review:singleton");
    assert.equal(calls[0].method, "meta/member_health");
    assert.deepEqual(calls[0].params, { member_id: "review:singleton" });
    assert.equal(health.identity, "review:singleton");
    assert.equal(health.memberId, "rt:review:singleton:4");
    assert.equal(health.state, "active");
    assert.equal(health.bootstrapState, "active");
    assert.equal(health.materializationInFlight, false);
    assert.equal(health.sessionId, "sess-1");
    assert.equal(health.generation, 4);
    assert.equal(health.lastDeliveryError?.class, "reload_required");
    assert.equal(health.actorLoop.state, "stalled");
    assert.equal(health.openStallId, 7);
    assert.equal(health.lastReload, null);
    // Absent until the gateway can read meerkat's durability state.
    assert.equal(health.durability, null);
  });

  it("carries durability when present and degrades on a missing actor_loop", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({
      identity: "x",
      state: "dormant",
      durability: { reload_required: { operation: "completed_boundary_commit", reason: "HTTP 0" } },
    }));
    const health = await handle.memberHealth("x");
    assert.deepEqual(health.durability, {
      reload_required: { operation: "completed_boundary_commit", reason: "HTTP 0" },
    });
    assert.deepEqual(health.actorLoop, { state: "unobserved" });
    assert.equal(health.openStallId, null);
    assert.equal(health.lastDeliveryError, null);
  });
});

describe("MobHandle.resolveRouting()", () => {
  it("sends meta/routing/resolve with recipient", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      recipient: "user@example.com",
      route: { sink: "email", target_module: "mailer" },
    }));

    const result = await handle.resolveRouting("user@example.com", {
      hint: "email",
    });
    assert.equal(calls[0].method, "meta/routing/resolve");
    assert.equal(calls[0].params!.recipient, "user@example.com");
    assert.equal(calls[0].params!.hint, "email");
    assert.equal(result.recipient, "user@example.com");
    assert.deepEqual(result.route, { sink: "email", target_module: "mailer" });
  });

  it("works without options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ recipient: "r", route: {} }));

    await handle.resolveRouting("r");
    assert.deepEqual(calls[0].params, { recipient: "r" });
  });
});

describe("MobHandle.listRoutes()", () => {
  it("sends meta/routing/routes/list and parses routes", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      routes: [
        {
          route_key: "rk-1",
          recipient: "user@example.org",
          channel: "email",
          sink: "smtp",
          target_module: "mailer",
        },
      ],
    }));

    const result = await handle.listRoutes();
    assert.equal(calls[0].method, "meta/routing/routes/list");
    assert.equal(result.length, 1);
    assert.equal(result[0].routeKey, "rk-1");
    assert.equal(result[0].recipient, "user@example.org");
    assert.equal(result[0].channel, "email");
    assert.equal(result[0].sink, "smtp");
    assert.equal(result[0].targetModule, "mailer");
  });
});

describe("MobHandle.addRoute()", () => {
  it("sends meta/routing/routes/add with all params", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      route: {
        route_key: "rk-new",
        recipient: "alice",
        channel: "slack",
        sink: "webhook",
        target_module: "notifier",
      },
    }));

    const result = await handle.addRoute(
      "rk-new",
      "alice",
      "webhook",
      "notifier",
      "slack",
    );
    assert.equal(calls[0].method, "meta/routing/routes/add");
    assert.equal(calls[0].params!.route_key, "rk-new");
    assert.equal(calls[0].params!.recipient, "alice");
    assert.equal(calls[0].params!.sink, "webhook");
    assert.equal(calls[0].params!.target_module, "notifier");
    assert.equal(calls[0].params!.channel, "slack");
    assert.equal(result.routeKey, "rk-new");
    assert.equal(result.channel, "slack");
  });

  it("omits channel when not provided", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      route: {
        route_key: "rk",
        recipient: "r",
        channel: null,
        sink: "s",
        target_module: "tm",
      },
    }));

    await handle.addRoute("rk", "r", "s", "tm");
    assert.equal(calls[0].params!.channel, undefined);
  });
});

describe("MobHandle.deleteRoute()", () => {
  it("sends meta/routing/routes/delete", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      deleted: {
        route_key: "rk-del",
        recipient: "bob",
        channel: null,
        sink: "email",
        target_module: "mailer",
      },
    }));

    const result = await handle.deleteRoute("rk-del");
    assert.equal(calls[0].method, "meta/routing/routes/delete");
    assert.deepEqual(calls[0].params, { route_key: "rk-del" });
    assert.equal(result.routeKey, "rk-del");
    assert.equal(result.channel, null);
  });
});

describe("MobHandle.sendDelivery()", () => {
  it("sends meta/delivery/send with options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      delivered: true,
      delivery_id: "dlv-1",
    }));

    const result = await handle.sendDelivery({
      recipient: "alice",
      payload: "hi",
    });
    assert.equal(calls[0].method, "meta/delivery/send");
    assert.deepEqual(calls[0].params, { recipient: "alice", payload: "hi" });
    assert.equal(result.delivered, true);
    assert.equal(result.deliveryId, "dlv-1");
  });
});

describe("MobHandle.deliveryHistory()", () => {
  it("sends meta/delivery/history with defaults", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      deliveries: [{ id: "dlv-1" }],
    }));

    const result = await handle.deliveryHistory();
    assert.equal(calls[0].method, "meta/delivery/history");
    assert.deepEqual(calls[0].params, { limit: 20 });
    assert.equal(result.deliveries.length, 1);
  });

  it("sends with recipient, sink, and custom limit", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ deliveries: [] }));

    await handle.deliveryHistory("alice", "email", 5);
    assert.equal(calls[0].params!.recipient, "alice");
    assert.equal(calls[0].params!.sink, "email");
    assert.equal(calls[0].params!.limit, 5);
  });
});

describe("MobHandle.memoryQuery()", () => {
  it("sends meta/memory/query", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      assertions: [{
        assertion_id: "a-1",
        entity: "foo",
        topic: "bar",
        store: "knowledge_graph",
        fact: "Fact",
        indexed_at_ms: 10,
      }],
      conflicts: [],
    }));

    const result = await handle.memoryQuery({ entity: "foo", topic: "bar", store: "main" });
    assert.equal(calls[0].method, "meta/memory/query");
    assert.equal(calls[0].params!.entity, "foo");
    assert.equal(calls[0].params!.topic, "bar");
    assert.equal(calls[0].params!.store, "main");
    assert.equal(result.assertions.length, 1);
    assert.equal(result.results.length, 1);
  });

  it("keeps legacy free-form query callers wire-compatible", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      results: [{ entity: "foo", topic: "bar" }],
    }));

    const result = await handle.memoryQuery("search term", { store: "main" });
    assert.equal(calls[0].method, "meta/memory/query");
    assert.equal(calls[0].params!.query, "search term");
    assert.equal(calls[0].params!.store, "main");
    assert.equal(result.results.length, 1);
  });
});

describe("MobHandle.memoryStores()", () => {
  it("sends meta/memory/stores and parses stores", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      stores: [
        { store: "main", record_count: 42 },
        { store: "archive", record_count: 100 },
      ],
    }));

    const result = await handle.memoryStores();
    assert.equal(calls[0].method, "meta/memory/stores");
    assert.equal(result.length, 2);
    assert.equal(result[0].store, "main");
    assert.equal(result[0].recordCount, 42);
    assert.equal(result[1].store, "archive");
    assert.equal(result[1].recordCount, 100);
  });
});

describe("MobHandle.memoryIndex()", () => {
  it("sends meta/memory/index", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      entity: "user-1",
      topic: "preferences",
      store: "main",
      assertion_id: "assert-1",
    }));

    const result = await handle.memoryIndex("user-1", "preferences", "main", {
      extra: true,
    });
    assert.equal(calls[0].method, "meta/memory/index");
    assert.equal(calls[0].params!.entity, "user-1");
    assert.equal(calls[0].params!.topic, "preferences");
    assert.equal(calls[0].params!.store, "main");
    assert.equal(calls[0].params!.extra, true);
    assert.equal(result.entity, "user-1");
    assert.equal(result.topic, "preferences");
    assert.equal(result.store, "main");
    assert.equal(result.assertionId, "assert-1");
  });
});

describe("MobHandle.rememberAgentMemory()", () => {
  it("sends meta/agent_memory/remember and parses the record", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      memory_id: "mem-1",
      title: "School pickup",
      body: "Pickup is before calendar planning.",
      tags: ["calendar", "team"],
      created_at_ms: 10,
      updated_at_ms: 20,
    }));

    const result = await handle.rememberAgentMemory("identity:luka", {
      realm: "example",
      title: "School pickup",
      body: "Pickup is before calendar planning.",
      tags: ["team", "calendar"],
    });

    assert.equal(calls[0].method, "meta/agent_memory/remember");
    assert.deepEqual(calls[0].params, {
      identity: "identity:luka",
      realm: "example",
      title: "School pickup",
      body: "Pickup is before calendar planning.",
      tags: ["team", "calendar"],
    });
    assert.equal(result.memoryId, "mem-1");
    assert.equal(result.title, "School pickup");
    assert.equal(result.body, "Pickup is before calendar planning.");
    assert.deepEqual(result.tags, ["calendar", "team"]);
    assert.equal(result.createdAtMs, 10);
    assert.equal(result.updatedAtMs, 20);
  });
});

describe("MobHandle.recallAgentMemory()", () => {
  it("sends meta/agent_memory/recall and parses records", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      records: [{
        memory_id: "mem-1",
        title: "School pickup",
        body: "Pickup is before calendar planning.",
        tags: ["calendar", "team"],
        created_at_ms: 10,
        updated_at_ms: 20,
      }],
    }));

    const result = await handle.recallAgentMemory("identity:luka", {
      realm: "example",
      selection: "contextual",
      queryText: "Where is pickup?",
      queryTerms: ["pickup"],
      maxEntries: 4,
    });

    assert.equal(calls[0].method, "meta/agent_memory/recall");
    assert.deepEqual(calls[0].params, {
      identity: "identity:luka",
      realm: "example",
      selection: "contextual",
      query_text: "Where is pickup?",
      query_terms: ["pickup"],
      max_entries: 4,
    });
    assert.equal(result.length, 1);
    assert.equal(result[0]!.memoryId, "mem-1");
    assert.equal(result[0]!.title, "School pickup");
  });
});

describe("MobHandle.forgetAgentMemory()", () => {
  it("sends meta/agent_memory/forget and parses the result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      memory_id: "mem-1",
      deleted: true,
    }));

    const result = await handle.forgetAgentMemory("identity:luka", "mem-1", {
      realm: "example",
    });

    assert.equal(calls[0].method, "meta/agent_memory/forget");
    assert.deepEqual(calls[0].params, {
      identity: "identity:luka",
      memory_id: "mem-1",
      realm: "example",
    });
    assert.equal(result.memoryId, "mem-1");
    assert.equal(result.deleted, true);
  });
});

describe("MobHandle.updateAgentMemory()", () => {
  it("sends meta/agent_memory/update and parses the result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      memory_id: "mem-2",
      supersedes: "mem-1",
    }));

    const result = await handle.updateAgentMemory("identity:luka", "mem-1", {
      realm: "example",
      title: "School pickup",
      body: "Pickup moved to 15:30.",
      tags: ["team"],
    });

    assert.equal(calls[0].method, "meta/agent_memory/update");
    assert.deepEqual(calls[0].params, {
      identity: "identity:luka",
      memory_id: "mem-1",
      title: "School pickup",
      body: "Pickup moved to 15:30.",
      realm: "example",
      tags: ["team"],
    });
    assert.equal(result.memoryId, "mem-2");
    assert.equal(result.supersedes, "mem-1");
  });
});

describe("MobHandle.manifestAgentMemory()", () => {
  it("sends meta/agent_memory/manifest and parses record metas", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      records: [{
        id: "mem-2",
        kind: "fact",
        title: "School pickup",
        description: "When planning the family calendar",
        age_days: 3,
        rank: 1,
      }],
    }));

    const result = await handle.manifestAgentMemory("identity:luka", {
      realm: "example",
      tier: "working_set",
      k: 4,
    });

    assert.equal(calls[0].method, "meta/agent_memory/manifest");
    assert.deepEqual(calls[0].params, {
      identity: "identity:luka",
      realm: "example",
      tier: "working_set",
      k: 4,
    });
    assert.equal(result.length, 1);
    assert.equal(result[0]!.id, "mem-2");
    assert.equal(result[0]!.kind, "fact");
    assert.equal(result[0]!.ageDays, 3);
    assert.equal(result[0]!.rank, 1);
  });

  it("omits optional params and parses rank-less rows", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      records: [{
        id: "mem-3",
        kind: "gotcha",
        title: "Unranked",
        age_days: 0,
      }],
    }));

    const result = await handle.manifestAgentMemory("identity:luka");

    assert.equal(calls[0].method, "meta/agent_memory/manifest");
    assert.deepEqual(calls[0].params, { identity: "identity:luka" });
    assert.equal(result[0]!.rank, null);
    assert.equal(result[0]!.description, "");
  });
});

describe("MobHandle.callTool()", () => {
  it("sends meta/call_tool with args", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      module_id: "google-workspace",
      tool: "gmail_search",
      result: { messages: ["msg-1"] },
    }));

    const result = await handle.callTool("google-workspace", "gmail_search", {
      query: "is:unread",
    });
    assert.equal(calls[0].method, "meta/call_tool");
    assert.equal(calls[0].params!.module_id, "google-workspace");
    assert.equal(calls[0].params!.tool, "gmail_search");
    assert.deepEqual(calls[0].params!.arguments, { query: "is:unread" });
    assert.equal(result.moduleId, "google-workspace");
    assert.equal(result.tool, "gmail_search");
    assert.deepEqual(result.result, { messages: ["msg-1"] });
  });

  it("omits arguments when not provided", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ module_id: "m", tool: "t", result: null }));

    await handle.callTool("m", "t");
    assert.equal(calls[0].params!.arguments, undefined);
  });
});

describe("MobHandle.toolCaller()", () => {
  it("returns a ToolCaller that calls the right module", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      module_id: "ws",
      tool: "search",
      result: { items: [1, 2] },
    }));

    const caller = handle.toolCaller("ws");
    assert.ok(caller instanceof ToolCaller);

    const result = await caller.call("search", { q: "test" });
    assert.equal(calls[0].method, "meta/call_tool");
    assert.equal(calls[0].params!.module_id, "ws");
    assert.equal(calls[0].params!.tool, "search");
    assert.deepEqual(calls[0].params!.arguments, { q: "test" });
    // ToolCaller.call returns result.result, not the full CallToolResult
    assert.deepEqual(result, { items: [1, 2] });
  });
});

describe("MobHandle.gatingEvaluate()", () => {
  it("sends meta/gating/evaluate", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      action_id: "act-1",
      action: "delete_account",
      actor_id: "user-1",
      risk_tier: "high",
      outcome: "pending",
      pending_id: "pend-1",
    }));

    const result = await handle.gatingEvaluate("delete_account", "user-1", {
      context: "admin",
    });
    assert.equal(calls[0].method, "meta/gating/evaluate");
    assert.equal(calls[0].params!.action, "delete_account");
    assert.equal(calls[0].params!.actor_id, "user-1");
    assert.equal(calls[0].params!.context, "admin");
    assert.equal(result.actionId, "act-1");
    assert.equal(result.action, "delete_account");
    assert.equal(result.actorId, "user-1");
    assert.equal(result.riskTier, "high");
    assert.equal(result.outcome, "pending");
    assert.equal(result.pendingId, "pend-1");
  });
});

describe("MobHandle.gatingPending()", () => {
  it("sends meta/gating/pending and parses entries", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      pending: [
        {
          pending_id: "p-1",
          action_id: "a-1",
          action: "delete",
          actor_id: "u-1",
          risk_tier: "high",
          created_at_ms: 1000,
        },
      ],
    }));

    const result = await handle.gatingPending();
    assert.equal(calls[0].method, "meta/gating/pending");
    assert.equal(result.length, 1);
    assert.equal(result[0].pendingId, "p-1");
    assert.equal(result[0].actionId, "a-1");
    assert.equal(result[0].action, "delete");
    assert.equal(result[0].actorId, "u-1");
    assert.equal(result[0].riskTier, "high");
    assert.equal(result[0].createdAtMs, 1000);
  });
});

describe("MobHandle.gatingDecide()", () => {
  for (const [decision, outcome, nextPendingId] of [
    ["approve", "allowed", null],
    ["reject", "safe_draft", null],
    ["escalate", "pending_approval", "p-2"],
  ] as const) {
    it(`returns the ${decision} result without automatic follow-up`, async () => {
      const { handle, calls, setResponse } = createMockRuntime();
      setResponse(() => ({
        pending_id: "p-1",
        action_id: "a-1",
        decision,
        approver_id: "admin-1",
        outcome,
        decided_at_ms: 1700000000000,
        reason: "Reviewed action",
        next_pending_id: nextPendingId,
      }));

      const result = await handle.gatingDecide("p-1", decision, "admin-1", {
        reason: "Reviewed action",
      });
      assert.deepEqual(calls, [{
        method: "meta/gating/decide",
        params: {
          pending_id: "p-1",
          decision,
          approver_id: "admin-1",
          reason: "Reviewed action",
        },
      }]);
      assert.deepEqual(result, {
        pendingId: "p-1",
        actionId: "a-1",
        decision,
        approverId: "admin-1",
        outcome,
        decidedAtMs: 1700000000000,
        reason: "Reviewed action",
        nextPendingId,
      });
    });
  }

  it("returns an unknown outcome without making another RPC", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      pending_id: "p-1",
      action_id: "a-1",
      decision: "approve",
      outcome: "future_outcome",
    }));

    const result = await handle.gatingDecide("p-1", "approve", "admin-1");
    assert.deepEqual(calls, [{
      method: "meta/gating/decide",
      params: {
        pending_id: "p-1",
        decision: "approve",
        approver_id: "admin-1",
      },
    }]);
    assert.equal(result.outcome, "future_outcome");
    assert.equal(result.approverId, null);
    assert.equal(result.decidedAtMs, null);
    assert.equal(result.reason, null);
    assert.equal(result.nextPendingId, null);
  });
});

describe("MobHandle.gatingAudit()", () => {
  it("sends meta/gating/audit with default limit", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      entries: [
        {
          audit_id: "aud-1",
          timestamp_ms: 2000,
          event_type: "decision",
          action_id: "a-1",
          actor_id: "u-1",
          risk_tier: "medium",
          outcome: "approved",
        },
      ],
    }));

    const result = await handle.gatingAudit();
    assert.equal(calls[0].method, "meta/gating/audit");
    assert.deepEqual(calls[0].params, { limit: 100 });
    assert.equal(result.length, 1);
    assert.equal(result[0].auditId, "aud-1");
    assert.equal(result[0].timestampMs, 2000);
    assert.equal(result[0].eventType, "decision");
    assert.equal(result[0].riskTier, "medium");
  });

  it("sends with custom limit", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ entries: [] }));

    await handle.gatingAudit(10);
    assert.deepEqual(calls[0].params, { limit: 10 });
  });
});

describe("MobHandle.rediscover()", () => {
  it("returns RediscoverReport on normal response", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      spawned: ["mod-a"],
      edges: {
        desired_edges: [],
        wired_edges: [{ from: "a", to: "b" }],
        unwired_edges: [],
        retained_edges: [],
        preexisting_edges: [],
        skipped_missing_members: [],
        pruned_stale_managed_edges: [],
        failures: [],
      },
    }));

    const result = await handle.rediscover();
    assert.equal(calls[0].method, "meta/rediscover");
    assert.notEqual(result, null);
    assert.deepEqual(result!.spawned, ["mod-a"]);
    assert.equal(result!.edges.wiredEdges.length, 1);
    assert.equal(result!.edges.isComplete, true);
  });

  it("returns null when status response", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({ status: "no_discovery_configured" }));

    const result = await handle.rediscover();
    assert.equal(result, null);
  });
});

describe("MobHandle.reconcileEdges()", () => {
  it("sends meta/reconcile_edges and parses report", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      desired_edges: [{ from: "a", to: "b" }],
      wired_edges: [{ from: "a", to: "b" }],
      unwired_edges: [],
      retained_edges: [],
      preexisting_edges: [],
      skipped_missing_members: [],
      pruned_stale_managed_edges: [],
      failures: [],
    }));

    const result = await handle.reconcileEdges();
    assert.equal(calls[0].method, "meta/reconcile_edges");
    assert.equal(result.desiredEdges.length, 1);
    assert.equal(result.wiredEdges.length, 1);
    assert.equal(result.isComplete, true);
  });

  it("isComplete is false when there are failures", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({
      desired_edges: [],
      wired_edges: [],
      unwired_edges: [],
      retained_edges: [],
      preexisting_edges: [],
      skipped_missing_members: [],
      pruned_stale_managed_edges: [],
      failures: [{ error: "some error" }],
    }));

    const result = await handle.reconcileEdges();
    assert.equal(result.isComplete, false);
  });
});

describe("MobHandle.queryEvents()", () => {
  it("sends meta/query_events and returns parsed events", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => [
      {
        id: "evt-1",
        seq: 1,
        timestamp_ms: 5000,
        member_id: "m-1",
        event: { Agent: { agent_id: "a-1", event_type: "run_started" } },
      },
      {
        id: "evt-2",
        seq: 2,
        timestamp_ms: 6000,
        member_id: null,
        event: {
          Module: { module: "mod-x", event_type: "loaded", payload: {} },
        },
      },
    ]);

    const result = await handle.queryEvents({ sinceMs: 1000, limit: 50 });
    assert.equal(calls[0].method, "meta/query_events");
    assert.equal(calls[0].params!.since_ms, 1000);
    assert.equal(calls[0].params!.limit, 50);
    assert.equal(result.length, 2);
    assert.equal(result[0].id, "evt-1");
    assert.equal(result[0].seq, 1);
    assert.equal(result[0].memberId, "m-1");
    assert.equal(result[0].event.kind, "agent");
    assert.equal(result[1].event.kind, "module");
  });

  it("returns empty array when no query is passed", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => []);

    const result = await handle.queryEvents();
    assert.equal(calls[0].method, "meta/query_events");
    assert.deepEqual(calls[0].params, {});
    assert.deepEqual(result, []);
  });

  it("returns fallback events when no_event_log_configured includes events", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({
      status: "no_event_log_configured",
      events: [
        {
          id: "evt-9",
          seq: 9,
          timestamp_ms: 9000,
          member_id: "m-9",
          event: {
            Agent: {
              agent_id: "a-9",
              event_type: "text_delta",
              payload: { delta: "hi" },
            },
          },
        },
      ],
    }));

    const result = await handle.queryEvents({ limit: 10 });
    assert.equal(result.length, 1);
    assert.equal(result[0].id, "evt-9");
    assert.equal(result[0].event.kind, "agent");
  });

  it("returns empty array when no_event_log_configured omits events", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({ status: "no_event_log_configured" }));

    const result = await handle.queryEvents({ limit: 10 });
    assert.deepEqual(result, []);
  });

  it("returns empty array when response is not an array and not status", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse(() => ({ something: "else" }));

    const result = await handle.queryEvents();
    assert.deepEqual(result, []);
  });
});

// ---------------------------------------------------------------------------
// WorkGraph
// ---------------------------------------------------------------------------

const WG_ITEM_WIRE = {
  id: "work_1",
  realm_id: "realm-1",
  namespace: "default",
  title: "Ship the thing",
  status: "open",
  completion_policy: { kind: "self_attest" },
  priority: "medium",
  labels: ["a"],
  machine_state: { lifecycle_phase: "open", revision: 1 },
  revision: 1,
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const WG_EDGE_WIRE = {
  realm_id: "realm-1",
  namespace: "default",
  kind: "blocks",
  from_id: "work_1",
  to_id: "work_2",
  created_at: "2026-01-01T00:00:00Z",
};

const WG_ATTENTION_WIRE = {
  binding_id: "attention_1",
  work_ref: { item_id: "work_1", realm_id: "realm-1", namespace: "default" },
  target: { kind: "session", session_id: "sess-1" },
  mode: "pursue",
  status: { state: "active" },
  delegated_authority: "add_evidence",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

describe("MobHandle.workgraphSnapshot()", () => {
  it("sends meta/workgraph/snapshot with converted filter params", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      realm_id: "realm-1",
      namespace: "default",
      all_namespaces: false,
      captured_at: "2026-01-01T00:00:00Z",
      items: [WG_ITEM_WIRE],
      edges: [WG_EDGE_WIRE],
      attention: [WG_ATTENTION_WIRE],
      ready_item_ids: ["work_1"],
    }));

    const result = await handle.workgraphSnapshot({
      namespace: "default",
      allNamespaces: false,
      statuses: ["open"],
      includeTerminal: true,
      limit: 50,
    });
    assert.equal(calls[0].method, "meta/workgraph/snapshot");
    assert.deepEqual(calls[0].params, {
      namespace: "default",
      all_namespaces: false,
      statuses: ["open"],
      include_terminal: true,
      limit: 50,
    });
    assert.equal(result.realmId, "realm-1");
    assert.equal(result.items.length, 1);
    assert.equal(result.edges.length, 1);
    assert.equal(result.attention.length, 1);
    assert.deepEqual(result.readyItemIds, ["work_1"]);
  });

  it("sends empty params when no options given", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ realm_id: "r", all_namespaces: true, captured_at: "t" }));
    await handle.workgraphSnapshot();
    assert.deepEqual(calls[0].params, {});
  });
});

describe("MobHandle.workgraphList()", () => {
  it("sends meta/workgraph/list and returns unwrapped items", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ items: [WG_ITEM_WIRE] }));

    const result = await handle.workgraphList({ labels: ["a"] });
    assert.equal(calls[0].method, "meta/workgraph/list");
    assert.deepEqual(calls[0].params, { labels: ["a"] });
    assert.equal(result.length, 1);
    assert.equal(result[0].id, "work_1");
  });
});

describe("MobHandle.workgraphGet()", () => {
  it("sends id/namespace and unwraps item", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    const result = await handle.workgraphGet("work_1", { namespace: "default" });
    assert.equal(calls[0].method, "meta/workgraph/get");
    assert.deepEqual(calls[0].params, { id: "work_1", namespace: "default" });
    assert.equal(result.id, "work_1");
  });
});

describe("MobHandle.workgraphReady()", () => {
  it("sends meta/workgraph/ready with converted options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ items: [WG_ITEM_WIRE] }));

    const result = await handle.workgraphReady({ labels: ["urgent"], limit: 5 });
    assert.equal(calls[0].method, "meta/workgraph/ready");
    assert.deepEqual(calls[0].params, { labels: ["urgent"], limit: 5 });
    assert.equal(result.length, 1);
  });
});

describe("MobHandle.workgraphEvents()", () => {
  it("sends meta/workgraph/events and unwraps events array", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      events: [
        {
          seq: 1,
          realm_id: "realm-1",
          namespace: "default",
          item_id: "work_1",
          kind: "created",
          at: "2026-01-01T00:00:00Z",
          payload: {},
        },
      ],
    }));

    const result = await handle.workgraphEvents({ afterSeq: 0, limit: 20 });
    assert.equal(calls[0].method, "meta/workgraph/events");
    assert.deepEqual(calls[0].params, { after_seq: 0, limit: 20 });
    assert.equal(result.length, 1);
    assert.equal(result[0].kind, "created");
  });
});

describe("MobHandle.workgraphAttentionList()", () => {
  it("sends meta/workgraph/attention/list and unwraps attention array", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ attention: [WG_ATTENTION_WIRE] }));

    const result = await handle.workgraphAttentionList({ status: "active" });
    assert.equal(calls[0].method, "meta/workgraph/attention/list");
    assert.deepEqual(calls[0].params, { status: "active" });
    assert.equal(result.length, 1);
    assert.equal(result[0].bindingId, "attention_1");
  });
});

describe("MobHandle.workgraphGoalStatus()", () => {
  it("sends binding_id/namespace and parses item+attention", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphGoalStatus("attention_1", {
      namespace: "default",
    });
    assert.equal(calls[0].method, "meta/workgraph/goal/status");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      namespace: "default",
    });
    assert.equal(result.item.id, "work_1");
    assert.equal(result.attention.bindingId, "attention_1");
  });
});

describe("MobHandle.workgraphCreate()", () => {
  it("sends title plus converted options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    const result = await handle.workgraphCreate("Ship the thing", {
      priority: "high",
      labels: ["backend"],
      status: "blocked",
    });
    assert.equal(calls[0].method, "meta/workgraph/create");
    assert.deepEqual(calls[0].params, {
      title: "Ship the thing",
      priority: "high",
      labels: ["backend"],
      status: "blocked",
    });
    assert.equal(result.id, "work_1");
  });

  it("sends bare title when no options given", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));
    await handle.workgraphCreate("Minimal");
    assert.deepEqual(calls[0].params, { title: "Minimal" });
  });
});

describe("MobHandle.workgraphUpdate()", () => {
  it("sends id/expected_revision plus converted options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    const result = await handle.workgraphUpdate("work_1", 3, {
      title: "New title",
      labels: [],
    });
    assert.equal(calls[0].method, "meta/workgraph/update");
    assert.deepEqual(calls[0].params, {
      id: "work_1",
      expected_revision: 3,
      title: "New title",
      labels: [],
    });
    assert.equal(result.id, "work_1");
  });
});

describe("MobHandle.workgraphClaim()", () => {
  it("sends id/expected_revision/owner plus options", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    const result = await handle.workgraphClaim(
      "work_1",
      1,
      { kind: "agent", id: "agent-1", displayName: "Agent One" },
      { leaseSeconds: 60 },
    );
    assert.equal(calls[0].method, "meta/workgraph/claim");
    assert.deepEqual(calls[0].params, {
      id: "work_1",
      expected_revision: 1,
      owner: { kind: "agent", id: "agent-1", display_name: "Agent One" },
      lease_seconds: 60,
    });
    assert.equal(result.id, "work_1");
  });
});

describe("MobHandle.workgraphRelease()", () => {
  it("sends id/expected_revision/namespace", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    await handle.workgraphRelease("work_1", 2, { namespace: "default" });
    assert.equal(calls[0].method, "meta/workgraph/release");
    assert.deepEqual(calls[0].params, {
      id: "work_1",
      expected_revision: 2,
      namespace: "default",
    });
  });
});

describe("MobHandle.workgraphClose()", () => {
  it("sends id/expected_revision plus status option", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    await handle.workgraphClose("work_1", 4, { status: "cancelled" });
    assert.equal(calls[0].method, "meta/workgraph/close");
    assert.deepEqual(calls[0].params, {
      id: "work_1",
      expected_revision: 4,
      status: "cancelled",
    });
  });

  it("omits status when not given (server defaults to completed)", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));
    await handle.workgraphClose("work_1", 4);
    assert.deepEqual(calls[0].params, { id: "work_1", expected_revision: 4 });
  });
});

describe("MobHandle.workgraphBlock()", () => {
  it("sends id/expected_revision/namespace", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    await handle.workgraphBlock("work_1", 1);
    assert.equal(calls[0].method, "meta/workgraph/block");
    assert.deepEqual(calls[0].params, { id: "work_1", expected_revision: 1 });
  });
});

describe("MobHandle.workgraphLink()", () => {
  it("sends kind/from_id/to_id and parses the {edge} wrapped result", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ edge: WG_EDGE_WIRE }));

    const result = await handle.workgraphLink("blocks", "work_1", "work_2");
    assert.equal(calls[0].method, "meta/workgraph/link");
    assert.deepEqual(calls[0].params, {
      kind: "blocks",
      from_id: "work_1",
      to_id: "work_2",
    });
    assert.equal(result.kind, "blocks");
    assert.equal(result.fromId, "work_1");
    assert.equal(result.toId, "work_2");
  });

  it("sends namespace when given", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ edge: WG_EDGE_WIRE }));

    await handle.workgraphLink("parent", "work_0", "work_1", {
      namespace: "default",
    });
    assert.deepEqual(calls[0].params, {
      kind: "parent",
      from_id: "work_0",
      to_id: "work_1",
      namespace: "default",
    });
  });
});

describe("MobHandle.workgraphAddEvidence()", () => {
  it("sends id/expected_revision/evidence", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    await handle.workgraphAddEvidence("work_1", 2, {
      kind: "self_attest",
      id: "ev-1",
      summary: "done",
    });
    assert.equal(calls[0].method, "meta/workgraph/evidence/add");
    assert.deepEqual(calls[0].params, {
      id: "work_1",
      expected_revision: 2,
      evidence: { kind: "self_attest", id: "ev-1", summary: "done" },
    });
  });
});

describe("MobHandle.workgraphEscalatePolicy()", () => {
  it("sends binding_id/id/expected_revision/completion_policy", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE }));

    await handle.workgraphEscalatePolicy("attention_1", "work_1", 1, {
      kind: "host_confirmed",
    });
    assert.equal(calls[0].method, "meta/workgraph/policy/escalate");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      id: "work_1",
      expected_revision: 1,
      completion_policy: { kind: "host_confirmed" },
    });
  });
});

describe("MobHandle.workgraphGoalCreate()", () => {
  it("sends title/target plus options (session target)", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphGoalCreate(
      "Track the migration",
      { kind: "session", sessionId: "sess-1" },
      { mode: "review" },
    );
    assert.equal(calls[0].method, "meta/workgraph/goal/create");
    assert.deepEqual(calls[0].params, {
      title: "Track the migration",
      target: { kind: "session", session_id: "sess-1" },
      mode: "review",
    });
    assert.equal(result.item.id, "work_1");
    assert.equal(result.attention.bindingId, "attention_1");
  });

  it("sends an identity target", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    await handle.workgraphGoalCreate("Track it", {
      kind: "identity",
      identity: "identity:luka",
    });
    assert.deepEqual(calls[0].params!.target, {
      kind: "identity",
      identity: "identity:luka",
    });
  });

  it("sends an owner target", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    await handle.workgraphGoalCreate("Track it", {
      kind: "owner",
      ownerKey: { kind: "mob", id: "mob-1" },
    });
    assert.deepEqual(calls[0].params!.target, {
      kind: "owner",
      owner_key: { kind: "mob", id: "mob-1" },
    });
  });
});

describe("MobHandle.workgraphGoalConfirm()", () => {
  it("sends binding_id/expected_revision/evidence", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphGoalConfirm("attention_1", 2, {
      evidence: { kind: "self_attest", id: "ev-1" },
    });
    assert.equal(calls[0].method, "meta/workgraph/goal/confirm");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      expected_revision: 2,
      evidence: { kind: "self_attest", id: "ev-1" },
    });
    assert.equal(result.item.id, "work_1");
  });
});

describe("MobHandle.workgraphGoalRequestClose()", () => {
  it("sends binding_id/expected_revision plus status option", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ item: WG_ITEM_WIRE, attention: WG_ATTENTION_WIRE }));

    await handle.workgraphGoalRequestClose("attention_1", 3, {
      status: "failed",
    });
    assert.equal(calls[0].method, "meta/workgraph/goal/request_close");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      expected_revision: 3,
      status: "failed",
    });
  });
});

describe("MobHandle.workgraphAttentionPause()", () => {
  it("sends binding_id/expected_revision plus until option", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphAttentionPause("attention_1", 1, {
      until: "2026-03-01T00:00:00Z",
    });
    assert.equal(calls[0].method, "meta/workgraph/attention/pause");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      expected_revision: 1,
      until: "2026-03-01T00:00:00Z",
    });
    assert.equal(result.bindingId, "attention_1");
  });
});

describe("MobHandle.workgraphAttentionResume()", () => {
  it("sends binding_id/expected_revision/namespace", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphAttentionResume("attention_1", 2, {
      namespace: "default",
    });
    assert.equal(calls[0].method, "meta/workgraph/attention/resume");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      expected_revision: 2,
      namespace: "default",
    });
    assert.equal(result.bindingId, "attention_1");
  });
});

describe("MobHandle.workgraphAttentionReassign()", () => {
  it("sends binding_id/expected_revision/target and parses previous+attention", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    const previous = { ...WG_ATTENTION_WIRE, binding_id: "attention_0" };
    setResponse(() => ({ previous, attention: WG_ATTENTION_WIRE }));

    const result = await handle.workgraphAttentionReassign(
      "attention_1",
      3,
      { kind: "session", sessionId: "sess-2" },
    );
    assert.equal(calls[0].method, "meta/workgraph/attention/reassign");
    assert.deepEqual(calls[0].params, {
      binding_id: "attention_1",
      expected_revision: 3,
      target: { kind: "session", session_id: "sess-2" },
    });
    assert.equal(result.previous.bindingId, "attention_0");
    assert.equal(result.attention.bindingId, "attention_1");
  });
});

describe("MobHandle.workgraphAttentionPrune()", () => {
  it("sends updated_before and returns the pruned count", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ pruned: 3 }));

    const pruned = await handle.workgraphAttentionPrune({
      updatedBefore: "2026-07-01T00:00:00Z",
    });
    assert.equal(calls[0].method, "meta/workgraph/attention/prune");
    assert.deepEqual(calls[0].params, {
      updated_before: "2026-07-01T00:00:00Z",
    });
    assert.equal(pruned, 3);
  });

  it("sends an empty filter and tolerates a malformed count", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({}));

    const pruned = await handle.workgraphAttentionPrune();
    assert.deepEqual(calls[0].params, {});
    assert.equal(pruned, 0);
  });
});


describe("MobHandle live methods", () => {
  it("sends identity + options and parses the bootstrap", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      channel_id: "ch-1",
      transport: { type: "websocket", url: "ws://x/live/ws", token: "t" },
    }));

    const opened = await handle.liveOpen("robot", {
      model: "gpt-realtime-2",
      instructions: "Use the current room voice.",
    });
    assert.equal(calls[0].method, "meta/live/open");
    assert.deepEqual(calls[0].params, {
      identity: "robot",
      model: "gpt-realtime-2",
      instructions: "Use the current room voice.",
    });
    assert.equal((opened.transport as Record<string, unknown>).type, "websocket");

    setResponse(() => ({ open: true }));
    await handle.liveStatus("robot");
    assert.equal(calls[1].method, "meta/live/status");

    setResponse(() => ({ closed: true }));
    await handle.liveClose("live-channel-1");
    assert.equal(calls[2].method, "meta/live/close");
    assert.deepEqual(calls[2].params, { channel_id: "live-channel-1" });

    setResponse(() => ({ refreshed: true }));
    await handle.liveRefresh("robot");
    assert.equal(calls[3].method, "meta/live/refresh");

    setResponse(() => ({ accepted: true }));
    await handle.liveSendInputImage("robot", "frame-0001", "image/jpeg", "aGVsbG8=");
    assert.equal(calls[4].method, "meta/live/send_input");
    assert.deepEqual(calls[4].params, {
      identity: "robot",
      chunk: {
        kind: "image",
        idempotency_key: "frame-0001",
        mime: "image/jpeg",
        data: "aGVsbG8=",
      },
    });

    setResponse(() => ({ status: "truncated" }));
    const active = {
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge" as const,
      activationReceipt: "active-receipt",
    };
    await handle.liveTruncate(
      active,
      { channelId: "chan-1", outputId: "opaque-output-1", contentIndex: 0 },
      1200,
    );
    assert.equal(calls[5].method, "meta/live/truncate");
    assert.deepEqual(calls[5].params, {
      identity: "identity:robot",
      channel_id: "chan-1",
      activation_receipt: "active-receipt",
      output_id: "opaque-output-1",
      audio_played_ms: 1200,
    });
  });

  it("serializes v1 execution identity and returns a typed handle", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse((method) => method === "meta/capabilities" ? {
      contract_version: "0.5.0",
      methods: ["meta/live/open"],
      loaded_modules: [],
      feature_capabilities: [
        "live.execution_identity.v1",
        "live.execution.function_bridge.v1",
      ],
    } : ({
      channel_id: "ch-typed",
      target_identity: "identity:robot",
      execution_mode: "function_bridge",
      pending_receipt: "pending-receipt",
      transport: { transport: "webrtc", token: "t", answer_method: "live/webrtc/answer" },
      capabilities: {
        audio_in: true,
        audio_out: true,
        text_in: true,
        text_out: true,
        image_in: false,
        video_in: false,
        transcript_supported: true,
        barge_in_supported: true,
        provider_native_resume: false,
      },
      continuity: { mode: "transcript_only" },
    }));

    const opened = await handle.liveOpenTyped("identity:robot", {
      profileId: "example.device.open-room.v1",
    });

    assert.equal(opened.channelId, "ch-typed");
    assert.equal(opened.targetIdentity, "identity:robot");
    assert.equal(calls[0].method, "meta/capabilities");
    assert.deepEqual(calls[1].params, {
      identity: "identity:robot",
      execution_identity: {
        version: "v1",
        profile_id: "example.device.open-room.v1",
      },
    });
  });

  it("refuses execution identity before opening against an old gateway", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({
      contract_version: "0.5.0",
      methods: ["meta/live/open"],
      loaded_modules: [],
    }));

    await assert.rejects(
      handle.liveOpenTyped("identity:robot", {
        profileId: "example.device.open-room.v1",
      }),
      CapabilityUnavailableError,
    );
    assert.deepEqual(calls.map((call) => call.method), ["meta/capabilities"]);
  });

  it("rejects catalog and Responses bridge overrides before strict open", async () => {
    for (const [field, value] of [
      ["execution_mode", "responses"],
      ["profile_id", "gpt-live-function-bridge-v1"],
      ["responses_model", "gpt-5.5"],
      ["responses_tools", []],
      ["responses_instructions", "delegate"],
      ["auth_binding", { realm: "example", binding: "other" }],
      ["self_hosted_server_id", "server"],
      ["provider_params", {}],
      ["tools", []],
      ["instructions", "delegate"],
    ] as const) {
      const { handle, calls } = createMockRuntime();
      await assert.rejects(
        handle.liveOpenTyped(
          "identity:robot",
          {
            profileId: "example.device.open-room.v1",
          },
          { [field]: value },
        ),
        (error: unknown) => {
          assert.match(
            String(error),
            /experimental live\/open does not accept/,
            `expected ${field} to be rejected before capability discovery`,
          );
          return true;
        },
      );
      assert.deepEqual(calls, []);
    }
  });

  it("refuses a strict open response missing canonical target identity", async () => {
    const { handle, setResponse } = createMockRuntime();
    setResponse((method) => method === "meta/capabilities" ? {
      contract_version: "0.5.0",
      methods: ["meta/live/open"],
      loaded_modules: [],
      feature_capabilities: ["live.execution_identity.v1"],
    } : ({
      channel_id: "ch-typed",
      transport: { transport: "webrtc", token: "t", answer_method: "live/webrtc/answer" },
      capabilities: {
        audio_in: true, audio_out: true, text_in: true, text_out: true,
        image_in: false, video_in: false, transcript_supported: true,
        barge_in_supported: true, provider_native_resume: false,
      },
      continuity: { mode: "transcript_only" },
    }));
    await assert.rejects(
      handle.liveOpenTyped("caller-alias", {
        profileId: "example.device.open-room.v1",
      }),
      /unknown field|non-empty string/,
    );
  });

  it("answers the WebRTC bootstrap through the advertised method", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ answer_sdp: "v=0\r\nanswer" }));
    const answer = await handle.liveWebrtcAnswer(
      "chan-1",
      "token-1",
      "v=0\r\noffer",
    );
    assert.equal(answer, "v=0\r\nanswer");
    assert.deepEqual(calls[0], {
      method: "live/webrtc/answer",
      params: {
        channel_id: "chan-1",
        token: "token-1",
        offer_sdp: "v=0\r\noffer",
      },
    });
  });

  it("connects only after owner readiness, answer, and active authority", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    const events: string[] = [];
    setResponse((method) => {
      if (method === "meta/capabilities") return {
        contract_version: "0.5.0",
        methods: [],
        loaded_modules: [],
        feature_capabilities: [
          "live.execution_identity.v1",
          "live.execution.function_bridge.v1",
        ],
      };
      if (method === "meta/live/open") return {
        channel_id: "chan-1",
        target_identity: "identity:robot",
        execution_mode: "function_bridge",
        pending_receipt: "pending-receipt",
        transport: {
          transport: "webrtc",
          token: "token-1",
          answer_method: "live/webrtc/answer",
        },
        capabilities: {
          audio_in: true,
          audio_out: true,
          text_in: false,
          text_out: true,
          image_in: false,
          video_in: false,
          transcript_supported: true,
          barge_in_supported: true,
          provider_native_resume: false,
        },
        continuity: { mode: "transcript_only" },
      };
      if (method === "meta/live/playback_owner/register") return {
        channel_id: "chan-1",
        readiness_receipt: "ready-receipt",
      };
      if (method === "live/webrtc/answer") return { answer_sdp: "v=0\r\nanswer" };
      if (method === "meta/live/status") return {
        phase: "active",
        handle: {
          channel_id: "chan-1",
          target_identity: "identity:robot",
          execution_mode: "function_bridge",
          activation_receipt: "active-receipt",
        },
      };
      if (method === "meta/live/playback_owner/revoke") return {
        phase: "revoked",
      };
      throw new Error(`unexpected method ${method}`);
    });

    const active = await handle.liveConnect(
      "identity:robot",
      {
        profileId: "example.device.open-room.v1",
      },
      {
        async prepare(pending) {
          events.push(`prepare:${pending.pendingReceipt}`);
          return "v=0\r\noffer";
        },
        async acceptAnswer(answerSdp) {
          events.push(`answer:${answerSdp}`);
        },
        async activate(activated) {
          events.push(`activate:${activated.activationReceipt}`);
        },
        async abort() {
          events.push("abort");
        },
      },
      { activationPollIntervalMs: 0 },
    );

    assert.equal(active.activationReceipt, "active-receipt");
    assert.equal(active.pendingReceipt, "pending-receipt");
    assert.equal(active.readinessReceipt, "ready-receipt");
    assert.deepEqual(events, [
      "prepare:pending-receipt",
      "answer:v=0\r\nanswer",
      "activate:active-receipt",
    ]);
    assert.deepEqual(calls.map((call) => call.method), [
      "meta/capabilities",
      "meta/live/open",
      "meta/live/playback_owner/register",
      "live/webrtc/answer",
      "meta/live/status",
    ]);
    assert.deepEqual(calls[2].params, {
      identity: "identity:robot",
      channel_id: "chan-1",
      pending_receipt: "pending-receipt",
    });
    assert.equal(calls[3].params?.readiness_receipt, "ready-receipt");
    assert.deepEqual(calls[4].params, {
      identity: "identity:robot",
      channel_id: "chan-1",
      pending_receipt: "pending-receipt",
    });
    assert.equal((await active.ownerLost()).phase, "revoked");
    assert.equal(events.at(-1), "abort");
    assert.deepEqual(calls.at(-1), {
      method: "meta/live/playback_owner/revoke",
      params: {
        identity: "identity:robot",
        channel_id: "chan-1",
        pending_receipt: "pending-receipt",
        readiness_receipt: "ready-receipt",
        activation_receipt: "active-receipt",
      },
    });
  });

  it("aborts and closes the pending channel when owner authority is revoked", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    const events: string[] = [];
    setResponse((method) => {
      if (method === "meta/capabilities") return {
        contract_version: "0.5.0",
        methods: [],
        loaded_modules: [],
        feature_capabilities: [
          "live.execution_identity.v1",
          "live.execution.function_bridge.v1",
        ],
      };
      if (method === "meta/live/open") return {
        channel_id: "chan-1",
        target_identity: "identity:robot",
        execution_mode: "function_bridge",
        pending_receipt: "pending-receipt",
        transport: {
          transport: "webrtc",
          token: "token-1",
          answer_method: "live/webrtc/answer",
        },
        capabilities: {
          audio_in: true,
          audio_out: true,
          text_in: false,
          text_out: true,
          image_in: false,
          video_in: false,
          transcript_supported: true,
          barge_in_supported: true,
          provider_native_resume: false,
        },
        continuity: { mode: "transcript_only" },
      };
      if (method === "meta/live/playback_owner/register") return {
        channel_id: "chan-1",
        readiness_receipt: "ready-receipt",
      };
      if (method === "live/webrtc/answer") return { answer_sdp: "v=0\r\nanswer" };
      if (method === "meta/live/status") return { phase: "revoked" };
      if (method === "meta/live/close") return { closed: true };
      throw new Error(`unexpected method ${method}`);
    });

    await assert.rejects(
      handle.liveConnect(
        "identity:robot",
        {
          profileId: "example.device.open-room.v1",
        },
        {
          async prepare() {
            events.push("prepare");
            return "v=0\r\noffer";
          },
          async acceptAnswer() {
            events.push("answer");
          },
          async activate() {
            events.push("activate");
          },
          async abort() {
            events.push("abort");
          },
        },
        { activationPollIntervalMs: 0 },
      ),
      /revoked before activation/,
    );

    assert.deepEqual(events, ["prepare", "answer", "abort"]);
    assert.deepEqual(calls.map((call) => call.method), [
      "meta/capabilities",
      "meta/live/open",
      "meta/live/playback_owner/register",
      "live/webrtc/answer",
      "meta/live/status",
      "meta/live/close",
    ]);
    assert.deepEqual(calls.at(-1)?.params, {
      identity: "identity:robot",
      channel_id: "chan-1",
      pending_receipt: "pending-receipt",
    });
  });

  it("rejects pending handles before active provider operations", async () => {
    const { handle, calls } = createMockRuntime();
    const pending = {
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge" as const,
      pendingReceipt: "pending-receipt",
      transport: {
        transport: "webrtc" as const,
        token: "token-1",
        answerMethod: "live/webrtc/answer",
      },
      capabilities: {
        audioIn: true,
        audioOut: true,
        textIn: false,
        textOut: true,
        imageIn: false,
        videoIn: false,
        transcriptSupported: true,
        bargeInSupported: true,
        providerNativeResume: false,
      },
      continuity: { mode: "transcript_only" as const },
    };
    await assert.rejects(
      handle.liveInterruptActive(pending as never),
      /activationReceipt must be a non-empty string/,
    );
    assert.deepEqual(calls, []);
  });

  it("pulls replacement signaling under active channel authority", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ required: false }));
    const active = {
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge" as const,
      activationReceipt: "active-receipt",
    };
    const result = await handle.liveReplacementRequired(active);
    assert.deepEqual(result, { required: false });
    assert.deepEqual(calls[0], {
      method: "meta/live/replacement_required",
      params: {
        identity: "identity:robot",
        channel_id: "chan-1",
        activation_receipt: "active-receipt",
      },
    });
  });

  it("reports playback completion without caller interaction identity", async () => {
    const { handle, calls, setResponse } = createMockRuntime();
    setResponse(() => ({ status: "completed" }));
    const active = {
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge" as const,
      activationReceipt: "active-receipt",
    };
    const result = await handle.livePlaybackComplete(active, {
      channelId: "chan-1",
      outputId: "opaque-output-1",
      contentIndex: 0,
    });
    assert.deepEqual(result, { status: "completed" });
    assert.deepEqual(calls[0], {
      method: "meta/live/playback_complete",
      params: {
        identity: "identity:robot",
        channel_id: "chan-1",
        activation_receipt: "active-receipt",
        output_id: "opaque-output-1",
      },
    });
  });

  it("streams an acknowledged output to playback completion and closes on teardown", async () => {
    const { rt, handle, calls, setResponse } = createMockRuntime();
    setResponse((method) => method === "meta/live/playback_complete"
      ? { status: "completed" }
      : { status: "closed" });
    const active = {
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge" as const,
      activationReceipt: "active-receipt",
    };
    const outputs = handle.liveOutputs(active, { capacity: 1 });
    const nextOutput = outputs.next();
    await Promise.resolve();

    const accepted = await (rt as any)._dispatcher.handleCallback(
      "meta/live/assistant_output_available",
      {
        channel_id: "chan-1",
        output_id: "opaque-output-1",
        content_index: 0,
      },
    );
    assert.deepEqual(accepted, { accepted: true });
    const output = await nextOutput;
    assert.equal(output.done, false);
    assert.equal(output.value?.outputId, "opaque-output-1");
    assert.deepEqual(
      await handle.livePlaybackComplete(active, output.value!),
      { status: "completed" },
    );
    await outputs.return();

    assert.deepEqual(calls, [
      {
        method: "meta/live/playback_complete",
        params: {
          identity: "identity:robot",
          channel_id: "chan-1",
          activation_receipt: "active-receipt",
          output_id: "opaque-output-1",
        },
      },
      {
        method: "meta/live/close",
        params: {
          identity: "identity:robot",
          channel_id: "chan-1",
          activation_receipt: "active-receipt",
        },
      },
    ]);
  });

  it("rejects output queue overflow and consumer teardown", async () => {
    const { rt, handle, setResponse } = createMockRuntime();
    setResponse(() => ({ status: "closed" }));
    const outputs = handle.liveOutputs({
      channelId: "chan-1",
      targetIdentity: "identity:robot",
      executionMode: "function_bridge",
      activationReceipt: "active-receipt",
    }, { capacity: 1 });
    const first = outputs.next();
    await Promise.resolve();
    const dispatch = (outputId: string) => (rt as any)._dispatcher.handleCallback(
      "meta/live/assistant_output_available",
      {
        channel_id: "chan-1",
        output_id: outputId,
        content_index: 0,
      },
    );

    await dispatch("opaque-output-1");
    await first;
    await dispatch("opaque-output-2");
    await assert.rejects(
      dispatch("opaque-output-3"),
      /live output consumer queue is full/,
    );
    await outputs.return();
    await assert.rejects(
      dispatch("opaque-output-4"),
      /no live output consumer registered/,
    );
  });
});
