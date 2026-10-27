import assert from "node:assert/strict";
import test from "node:test";

import { migrateConsoleWorkbenchTarget, type ConsoleWorkbenchTarget } from "@console-core";
import {
  CONSOLE_RPC_METHODS as SHARED_CONSOLE_RPC_METHODS,
} from "../../../packages/console-core/src/contract";
import {
  CONSOLE_COMMAND_NAMES as SHARED_CONSOLE_COMMAND_NAMES,
  consoleCommandMethod as sharedConsoleCommandMethod,
  createHttpConsoleTransport as createSharedHttpConsoleTransport,
  createMetaConsoleController as createSharedMetaConsoleController,
} from "../../../packages/console-core/src/headless";
import { CONSOLE_REST_PATHS, CONSOLE_RPC_METHODS, CONSOLE_RPC_PATHS } from "./contract";
import {
  CONSOLE_COMMAND_NAMES,
  createHttpConsoleTransport,
  createMetaConsoleController,
  type ConsoleCapabilities,
  type MetaConsoleTransport,
} from "./headless";
import type { ConsoleFrame, ConsoleTimelineAccepted, ConsoleTimelinePage } from "../types";
import type { ConsoleTransportFailure } from "./network";

function identityTarget(): ConsoleWorkbenchTarget {
  const target = migrateConsoleWorkbenchTarget({
    id: "chat:identity:lead",
    kind: "meta/identity-chat",
    title: "Lead",
    identity: "identity:lead",
  });
  assert.ok(target);
  return target;
}

function hostTarget(): ConsoleWorkbenchTarget {
  const target = migrateConsoleWorkbenchTarget({
    id: "project:alpha",
    kind: "host/project",
    title: "Project Alpha",
    payloadVersion: 1,
    payload: { projectId: "alpha" },
  });
  assert.ok(target);
  return target;
}

function controlTarget(kind: "routing" | "gating" | "topology"): ConsoleWorkbenchTarget {
  const target = migrateConsoleWorkbenchTarget({
    id: kind,
    kind,
    title: kind,
  });
  assert.ok(target);
  return target;
}

function createFakeTransport(options: {
  capabilities?: ConsoleCapabilities | ConsoleCapabilities[];
  queryPages?: ConsoleTimelinePage[];
  accepted?: ConsoleTimelineAccepted;
} = {}): MetaConsoleTransport & {
  capabilityCalls: number;
  live?: (frame: ConsoleFrame) => void;
  sends: unknown[];
  subscriptions: unknown[];
  commands: unknown[];
} {
  const queryPages = [...(options.queryPages || [])];
  const capabilitiesQueue = Array.isArray(options.capabilities)
    ? [...options.capabilities]
    : [];
  const fake = {
    capabilityCalls: 0,
    sends: [] as unknown[],
    subscriptions: [] as unknown[],
    commands: [] as unknown[],
    loadExperience: async () => ({ contract_version: "fake" }),
    capabilities: async () => {
      fake.capabilityCalls += 1;
      return capabilitiesQueue.shift()
        || (!Array.isArray(options.capabilities) && options.capabilities)
        || {
          version: "fake-capabilities",
          methods: [CONSOLE_RPC_METHODS.send, CONSOLE_RPC_METHODS.inspectIdentity],
        };
    },
    queryTimeline: async () => queryPages.shift() || { frames: [], available: true },
    subscribeTimeline: (input, onFrame) => {
      fake.subscriptions.push(input);
      fake.live = onFrame;
      return () => {
        fake.live = undefined;
      };
    },
    send: async (input) => {
      fake.sends.push(input);
      return options.accepted || {
        interaction_id: "turn-1",
        identity: input.identity,
        cursor: "console:2",
      };
    },
    executeCommand: async (input) => {
      fake.commands.push(input);
      return {
        command: input.command,
        accepted: true,
        result: { ok: true },
      };
    },
    upload: async () => ({ blob_id: "blob-1" }),
    blobUrl: (blobId) => `/blobs/${blobId}`,
  } satisfies MetaConsoleTransport & {
    capabilityCalls: number;
    live?: (frame: ConsoleFrame) => void;
    sends: unknown[];
    subscriptions: unknown[];
    commands: unknown[];
  };
  return fake;
}

test("headless timeline controller seeds, subscribes after cursor, backfills replay gaps, and deduplicates frames", async () => {
  const seed: ConsoleFrame = { id: "seed", event: "interaction_started", cursor: "console:1", data: {} };
  const backfill: ConsoleFrame = { id: "backfill", event: "text_delta", cursor: "console:2", data: "a" };
  const live: ConsoleFrame = { id: "live", event: "text_delta", cursor: "console:3", data: "b" };
  const transport = createFakeTransport({
    queryPages: [
      { frames: [seed], available: true, latestCursor: "console:1" },
      { frames: [seed, backfill], available: true, latestCursor: "console:2" },
    ],
  });
  const controller = createMetaConsoleController({ transport });
  const delivered: ConsoleFrame[] = [];

  const unsubscribe = await controller.timeline.subscribeWithBackfill(
    { identity: "identity:lead" },
    (frame) => delivered.push(frame.value),
  );

  assert.deepEqual(delivered.map((frame) => frame.id), ["seed"]);
  assert.deepEqual(transport.subscriptions.map((subscription) => { const { signal: _signal, ...scope } = subscription as { signal?: AbortSignal }; return scope; }), [{ identity: "identity:lead", after: "console:1" }]);
  assert.ok((transport.subscriptions[0] as { signal?: AbortSignal }).signal);

  transport.live?.({ id: "gap", event: "replay_unavailable", data: {} });
  await new Promise((resolve) => setTimeout(resolve, 0));
  transport.live?.(live);
  unsubscribe();
  transport.live?.({ id: "after-unsubscribe", event: "text_delta", data: "ignored" });

  assert.deepEqual(delivered.map((frame) => frame.id), ["seed", "backfill", "live"]);
});

test("headless timeline dedup keeps a bounded recent identity window", async () => {
  const seedFrames = Array.from({ length: 1_001 }, (_value, index) => ({
    id: `seed:${index}`,
    event: "text_delta",
    identity: "identity:lead",
    data: { index },
  } satisfies ConsoleFrame));
  const transport = createFakeTransport({
    queryPages: [{ frames: seedFrames, available: true, latestCursor: "console:1001" }],
  });
  const controller = createMetaConsoleController({ transport });
  const delivered: ConsoleFrame[] = [];

  const unsubscribe = await controller.timeline.subscribeWithBackfill(
    { identity: "identity:lead", limit: 1 },
    (frame) => delivered.push(frame.value),
  );
  assert.equal(delivered.length, 1_001);

  transport.live?.({ id: "seed:1000", event: "text_delta", identity: "identity:lead", data: { index: 1_000 } });
  assert.equal(delivered.length, 1_001, "recent duplicate should still be suppressed");

  transport.live?.({ id: "seed:0", event: "text_delta", identity: "identity:lead", data: { index: 0 } });
  assert.equal(delivered.length, 1_002, "oldest key should be evicted once the bounded window advances");
  unsubscribe();
});

test("headless timeline dedup does not collapse anonymous identical frames", async () => {
  const transport = createFakeTransport({
    queryPages: [{ frames: [], available: true, latestCursor: "console:1" }],
  });
  const controller = createMetaConsoleController({ transport });
  const delivered: ConsoleFrame[] = [];

  const unsubscribe = await controller.timeline.subscribeWithBackfill(
    { identity: "identity:lead", limit: 1 },
    (frame) => delivered.push(frame.value),
  );

  transport.live?.({ id: "", event: "text_delta", identity: "identity:lead", data: { text: "same" } });
  transport.live?.({ id: "", event: "text_delta", identity: "identity:lead", data: { text: "same" } });

  assert.equal(delivered.length, 2);
  unsubscribe();
});

test("headless command surface sends only capability-gated Meta identity targets and returns optimistic plus accepted facts", async () => {
  const transport = createFakeTransport();
  const controller = createMetaConsoleController({ transport });

  const result = await controller.commands.sendMessage(identityTarget(), {
    content: "hello",
    origin: "test",
    idempotencyKey: "idem-1",
    handlingMode: "queue",
    attachments: [{} as File],
  });

  assert.equal(result.optimistic.provenance.source, "optimistic");
  assert.equal(result.optimistic.provenance.correlationId, "idem-1");
  assert.equal(result.accepted.provenance.source, "meta-protocol");
  assert.equal(result.accepted.provenance.routeOrMethod, CONSOLE_RPC_METHODS.send);
  assert.equal(result.accepted.provenance.capabilityVersion, "fake-capabilities");
  assert.equal(transport.sends.length, 1);
  assert.equal((transport.sends[0] as { identity?: string }).identity, "identity:lead");
});

test("headless command surface refreshes stale missing capabilities before failing closed", async () => {
  const transport = createFakeTransport({
    capabilities: [
      { version: "empty", methods: [] },
      { version: "fresh", methods: [CONSOLE_RPC_METHODS.send] },
    ],
  });
  const controller = createMetaConsoleController({ transport });

  const result = await controller.commands.sendMessage(identityTarget(), {
    content: "hello",
    origin: "test",
    idempotencyKey: "idem-refresh",
  });

  assert.equal(result.accepted.provenance.capabilityVersion, "fresh");
  assert.equal(transport.sends.length, 1);
});

test("headless command surface coalesces concurrent capability loads", async () => {
  const transport = createFakeTransport();
  const controller = createMetaConsoleController({ transport });

  await Promise.all([
    controller.commands.sendMessage(identityTarget(), {
      content: "first",
      origin: "test",
      idempotencyKey: "idem-coalesce-1",
    }),
    controller.commands.sendMessage(identityTarget(), {
      content: "second",
      origin: "test",
      idempotencyKey: "idem-coalesce-2",
    }),
  ]);

  assert.equal(transport.capabilityCalls, 1);
  assert.equal(transport.sends.length, 2);
});

test("headless command surface refreshes previously-present capabilities before reuse", async () => {
  const transport = createFakeTransport({
    capabilities: [
      { version: "allowed", methods: [CONSOLE_RPC_METHODS.send] },
      { version: "revoked", methods: [] },
      { version: "still-revoked", methods: [] },
    ],
  });
  const controller = createMetaConsoleController({ transport });

  await controller.commands.sendMessage(identityTarget(), {
    content: "first",
    origin: "test",
    idempotencyKey: "idem-revoke-1",
  });
  await assert.rejects(
    () => controller.commands.sendMessage(identityTarget(), {
      content: "second",
      origin: "test",
      idempotencyKey: "idem-revoke-2",
    }),
    /Meta capability missing for meta\/console\/send/,
  );

  assert.equal(transport.capabilityCalls, 3);
  assert.equal(transport.sends.length, 1);
});

test("headless command surface exposes capability-gated blob upload", async () => {
  const controller = createMetaConsoleController({
    transport: createFakeTransport({
      capabilities: {
        version: "blob-capabilities",
        methods: [CONSOLE_RPC_METHODS.blobUpload],
      },
    }),
  });

  const result = await controller.commands.uploadBlob({
    file: {} as File,
    mediaType: "image/png",
  });

  assert.deepEqual(result.value, { blob_id: "blob-1" });
  assert.equal(result.provenance.source, "meta-protocol");
  assert.equal(result.provenance.routeOrMethod, CONSOLE_RPC_METHODS.blobUpload);
  assert.equal(result.provenance.capabilityVersion, "blob-capabilities");
});

test("headless command surface fails closed on missing capabilities and inert host targets", async () => {
  const noSend = createMetaConsoleController({
    transport: createFakeTransport({ capabilities: { methods: [] } }),
  });

  await assert.rejects(
    () => noSend.commands.sendMessage(identityTarget(), {
      content: "hello",
      origin: "test",
      idempotencyKey: "idem-2",
    }),
    /capability missing/i,
  );

  const controller = createMetaConsoleController({ transport: createFakeTransport() });
  await assert.rejects(
    () => controller.commands.sendMessage(hostTarget(), {
      content: "hello",
      origin: "test",
      idempotencyKey: "idem-3",
    }),
    /cannot send/i,
  );
  for (const command of [
    CONSOLE_RPC_METHODS.send,
    "meta/member/retire",
    "meta/member/respawn",
    "meta/member/reset",
    "meta/routing/routes/list",
    "meta/gating/decide",
    "meta/raw/rpc",
  ]) {
    await assert.rejects(
      () => controller.commands.execute({
        command: command as typeof CONSOLE_COMMAND_NAMES.inspectIdentity,
        target: hostTarget(),
      }),
      /host target .* cannot execute/i,
    );
  }
});

test("headless command execution only accepts modeled commands for allowed target kinds", async () => {
  const controller = createMetaConsoleController({ transport: createFakeTransport() });

  assert.deepEqual(await controller.commands.execute({
    command: CONSOLE_COMMAND_NAMES.inspectIdentity,
    target: identityTarget(),
  }), {
    command: CONSOLE_COMMAND_NAMES.inspectIdentity,
    accepted: true,
    result: { ok: true },
  });

  for (const command of [
    "meta/raw/rpc",
    "meta/member/reset",
    "meta/console/list_identities",
    "meta/inspect_identity",
  ]) {
    await assert.rejects(
      () => controller.commands.execute({
        command: command as typeof CONSOLE_COMMAND_NAMES.inspectIdentity,
        target: identityTarget(),
      }),
      /unknown Meta console command/i,
    );
  }

  const noInspect = createMetaConsoleController({
    transport: createFakeTransport({ capabilities: { methods: [CONSOLE_RPC_METHODS.send] } }),
  });
  await assert.rejects(
    () => noInspect.commands.execute({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: identityTarget(),
    }),
    /capability missing.*meta\/console\/inspect_identity/i,
  );

  const hostController = createMetaConsoleController({ transport: createFakeTransport() });
  await assert.rejects(
    () => hostController.commands.execute({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: hostTarget(),
    }),
    /host target .* cannot execute/i,
  );
});

test("headless command execution models lifecycle, routing, and gating commands with target constraints", async () => {
  const methods = [
    CONSOLE_RPC_METHODS.send,
    CONSOLE_RPC_METHODS.inspectIdentity,
    CONSOLE_RPC_METHODS.retireIdentity,
    CONSOLE_RPC_METHODS.respawnIdentity,
    CONSOLE_RPC_METHODS.resetIdentity,
    CONSOLE_RPC_METHODS.stopMemberRun,
    CONSOLE_RPC_METHODS.routingRoutesList,
    CONSOLE_RPC_METHODS.deliveryHistory,
    CONSOLE_RPC_METHODS.gatingPending,
    CONSOLE_RPC_METHODS.gatingAudit,
    CONSOLE_RPC_METHODS.gatingDecide,
  ];
  const transport = createFakeTransport({ capabilities: { methods, version: "cap-all" } });
  const controller = createMetaConsoleController({ transport });

  for (const command of [
    CONSOLE_COMMAND_NAMES.retireIdentity,
    CONSOLE_COMMAND_NAMES.respawnIdentity,
    CONSOLE_COMMAND_NAMES.resetIdentity,
    CONSOLE_COMMAND_NAMES.stopMemberRun,
  ]) {
    assert.equal((await controller.commands.execute({
      command,
      target: identityTarget(),
    })).accepted, true);
  }

  for (const command of [
    CONSOLE_COMMAND_NAMES.listRoutingRoutes,
    CONSOLE_COMMAND_NAMES.listDeliveryHistory,
  ]) {
    assert.equal((await controller.commands.execute({
      command,
      target: controlTarget("routing"),
    })).accepted, true);
    await assert.rejects(
      () => controller.commands.execute({ command, target: controlTarget("gating") }),
      /cannot execute command/i,
    );
  }

  for (const command of [
    CONSOLE_COMMAND_NAMES.listGatingPending,
    CONSOLE_COMMAND_NAMES.listGatingAudit,
    CONSOLE_COMMAND_NAMES.decideGating,
  ]) {
    assert.equal((await controller.commands.execute({
      command,
      target: controlTarget("gating"),
      params: command === CONSOLE_COMMAND_NAMES.decideGating
        ? { pending_id: "pending-1", approver_id: "operator", decision: "approve" }
        : {},
    })).accepted, true);
    await assert.rejects(
      () => controller.commands.execute({ command, target: controlTarget("routing") }),
      /cannot execute command/i,
    );
  }
});

test("headless command execution models the optional topology control surface", async () => {
  const methods = [
    CONSOLE_RPC_METHODS.topologyQuery,
    CONSOLE_RPC_METHODS.topologyPlan,
    CONSOLE_RPC_METHODS.topologyApply,
    CONSOLE_RPC_METHODS.topologyOperationGet,
    CONSOLE_RPC_METHODS.topologyAuditQuery,
  ];
  const transport = createFakeTransport({ capabilities: { methods, version: "topology-v1" } });
  const controller = createMetaConsoleController({ transport });
  const target = controlTarget("topology");

  for (const command of [
    CONSOLE_COMMAND_NAMES.topologyQuery,
    CONSOLE_COMMAND_NAMES.topologyPlan,
    CONSOLE_COMMAND_NAMES.topologyApply,
    CONSOLE_COMMAND_NAMES.topologyOperationGet,
    CONSOLE_COMMAND_NAMES.topologyAuditQuery,
  ]) {
    assert.equal((await controller.commands.execute({ command, target })).accepted, true);
  }
  assert.deepEqual(
    transport.commands.map((entry) => (entry as { command: string }).command),
    [
      CONSOLE_COMMAND_NAMES.topologyQuery,
      CONSOLE_COMMAND_NAMES.topologyPlan,
      CONSOLE_COMMAND_NAMES.topologyApply,
      CONSOLE_COMMAND_NAMES.topologyOperationGet,
      CONSOLE_COMMAND_NAMES.topologyAuditQuery,
    ],
  );

  await assert.rejects(
    () => controller.commands.execute({
      command: CONSOLE_COMMAND_NAMES.topologyApply,
      target: controlTarget("routing"),
    }),
    /cannot execute command topologyApply/,
  );

  const disabled = createMetaConsoleController({
    transport: createFakeTransport({
      capabilities: {
        methods: [CONSOLE_RPC_METHODS.topologyQuery, CONSOLE_RPC_METHODS.topologyOperationGet],
        topologyControl: {
          mode: "disabled",
          can_query: true,
          can_plan: false,
          can_apply: false,
        },
      },
    }),
  });
  await assert.rejects(
    () => disabled.commands.execute({ command: CONSOLE_COMMAND_NAMES.topologyApply, target }),
    /capability missing.*meta\/topology\/apply/i,
  );
});

test("stock and shared-core headless topology contracts cannot drift", async () => {
  assert.deepEqual(SHARED_CONSOLE_RPC_METHODS, CONSOLE_RPC_METHODS);
  assert.deepEqual(SHARED_CONSOLE_COMMAND_NAMES, CONSOLE_COMMAND_NAMES);

  const methods = [
    SHARED_CONSOLE_RPC_METHODS.topologyQuery,
    SHARED_CONSOLE_RPC_METHODS.topologyPlan,
    SHARED_CONSOLE_RPC_METHODS.topologyApply,
    SHARED_CONSOLE_RPC_METHODS.topologyOperationGet,
    SHARED_CONSOLE_RPC_METHODS.topologyAuditQuery,
  ];
  const commands = [
    SHARED_CONSOLE_COMMAND_NAMES.topologyQuery,
    SHARED_CONSOLE_COMMAND_NAMES.topologyPlan,
    SHARED_CONSOLE_COMMAND_NAMES.topologyApply,
    SHARED_CONSOLE_COMMAND_NAMES.topologyOperationGet,
    SHARED_CONSOLE_COMMAND_NAMES.topologyAuditQuery,
  ];
  assert.deepEqual(commands.map(sharedConsoleCommandMethod), methods);

  const transport = createFakeTransport({
    capabilities: {
      methods,
      version: "shared-topology-v1",
      topologyControl: {
        mode: "editable",
        can_query: true,
        can_plan: true,
        can_apply: true,
      },
    },
  });
  const controller = createSharedMetaConsoleController({ transport });
  const target = controlTarget("topology");
  for (const command of commands) {
    assert.equal((await controller.commands.execute({ command, target })).accepted, true);
  }
  assert.deepEqual(
    transport.commands.slice(-commands.length).map((entry) =>
      (entry as { command: string }).command
    ),
    commands,
  );
});

test("headless transport keeps uploads and blob URLs typed optional hooks", async () => {
  const transport = createFakeTransport();
  const upload = await transport.upload?.({ file: {} as File, mediaType: "image/png" });

  assert.deepEqual(upload, { blob_id: "blob-1" });
  assert.equal(transport.blobUrl?.("blob-1"), "/blobs/blob-1");
});

test("headless facts carry all required provenance classes", () => {
  const controller = createMetaConsoleController({ transport: createFakeTransport() });

  assert.equal(controller.facts.meta({}).provenance.source, "meta-protocol");
  assert.equal(controller.facts.derived({}).provenance.source, "controller-derived");
  assert.equal(controller.facts.optimistic({}, "idem").provenance.source, "optimistic");
  assert.equal(controller.facts.host({}).provenance.source, "host-adapter");
});

test("createHttpConsoleTransport uses stock console routes and typed RPC methods", async () => {
  const calls: Array<{ url: string; method: string; body?: string }> = [];
  let multipartPayload: Record<string, unknown> | null = null;
  let multipartHasFile = false;
  const previousFetch = globalThis.fetch;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    const requestUrl = String(url);
    if (init?.body instanceof FormData) {
      const form = init.body;
      multipartPayload = JSON.parse(String(form.get("payload") || "{}")) as Record<string, unknown>;
      multipartHasFile = Boolean(form.get("file:upload-http-0"));
    }
    calls.push({
      url: requestUrl,
      method: init?.method || "GET",
      body: typeof init?.body === "string" ? init.body : undefined,
    });
    if (requestUrl.endsWith(CONSOLE_REST_PATHS.experience)) {
      return new Response(JSON.stringify({ contract_version: "0.5.0" }), { status: 200 });
    }
    if (requestUrl.endsWith(CONSOLE_REST_PATHS.modules)) {
      return new Response(JSON.stringify({ modules: ["mob"] }), { status: 200 });
    }
    if (requestUrl.endsWith(CONSOLE_RPC_PATHS.multipartJsonRpc)) {
      const body = multipartPayload || {};
      return new Response(JSON.stringify({
        jsonrpc: "2.0",
        id: body.id,
        result: { blob_id: "blob-http" },
      }), { status: 200 });
    }
    if (requestUrl.endsWith(CONSOLE_RPC_PATHS.jsonRpc)) {
      const body = JSON.parse(String(init?.body || "{}"));
      if (body.method === CONSOLE_RPC_METHODS.capabilities) {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          result: {
            methods: [CONSOLE_RPC_METHODS.send, CONSOLE_RPC_METHODS.inspectIdentity],
            version: "cap-v1",
          },
        }), { status: 200 });
      }
      if (body.method === CONSOLE_RPC_METHODS.send) {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          result: { interaction_id: "turn-http", identity: body.params.identity },
        }), { status: 200 });
      }
      if (body.method === CONSOLE_RPC_METHODS.inspectIdentity) {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          result: { identity: body.params.identity, status: "ready" },
        }), { status: 200 });
      }
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;

  const originalNow = Date.now;
  try {
    Date.now = () => Number.parseInt("http", 36);
    const transport = createHttpConsoleTransport({ baseUrl: "http://console.test" });
    assert.equal((await transport.loadExperience()).contract_version, "0.5.0");
    assert.deepEqual(await transport.loadModules?.(), { modules: ["mob"] });
    assert.deepEqual(await transport.capabilities(), {
      methods: [CONSOLE_RPC_METHODS.send, CONSOLE_RPC_METHODS.inspectIdentity],
      version: "cap-v1",
      runtime_capabilities: undefined,
      method_capabilities: undefined,
    });
    assert.equal((await transport.send({
      identity: "identity:lead",
      content: "hello",
      origin: "test",
      idempotencyKey: "idem-http",
    })).interaction_id, "turn-http");
    assert.deepEqual(await transport.executeCommand?.({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: identityTarget(),
    }), {
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      accepted: true,
      result: { identity: "identity:lead", status: "ready" },
    });
    assert.deepEqual(await transport.upload?.({
      blobId: "upload-http-0",
      file: new File(["png"], "badge.png", { type: "image/png" }),
    }), { blob_id: "blob-http", url: undefined });
    assert.equal(multipartHasFile, true);
    assert.equal(multipartPayload?.method, CONSOLE_RPC_METHODS.blobUpload);
    assert.deepEqual(calls.map((call) => [call.method, new URL(call.url).pathname]), [
      ["GET", CONSOLE_REST_PATHS.experience],
      ["GET", CONSOLE_REST_PATHS.modules],
      ["POST", CONSOLE_RPC_PATHS.jsonRpc],
      ["POST", CONSOLE_RPC_PATHS.jsonRpc],
      ["POST", CONSOLE_RPC_PATHS.jsonRpc],
      ["POST", CONSOLE_RPC_PATHS.multipartJsonRpc],
    ]);
  } finally {
    Date.now = originalNow;
    globalThis.fetch = previousFetch;
  }
});

test("createHttpConsoleTransport falls back to legacy inspect RPC when the console method is unavailable", async () => {
  const methods: string[] = [];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    const requestUrl = String(url);
    if (requestUrl.endsWith(CONSOLE_RPC_PATHS.jsonRpc)) {
      const body = JSON.parse(String(init?.body || "{}"));
      methods.push(body.method);
      if (body.method === CONSOLE_RPC_METHODS.inspectIdentity) {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          error: { code: -32601, message: "method not found" },
        }), { status: 200 });
      }
      if (body.method === "meta/inspect_identity") {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          result: { identity: body.params.identity, status: "legacy-ready" },
        }), { status: 200 });
      }
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;

  try {
    const transport = createHttpConsoleTransport({ baseUrl: "http://console.test" });
    assert.deepEqual(await transport.executeCommand?.({
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      target: identityTarget(),
    }), {
      command: CONSOLE_COMMAND_NAMES.inspectIdentity,
      accepted: true,
      result: { identity: "identity:lead", status: "legacy-ready" },
    });
    assert.deepEqual(methods, [
      CONSOLE_RPC_METHODS.inspectIdentity,
      "meta/inspect_identity",
    ]);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

test("createHttpConsoleTransport does not fall back on non-method-missing inspect errors", async () => {
  const methods: string[] = [];
  const previousFetch = globalThis.fetch;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    const requestUrl = String(url);
    if (requestUrl.endsWith(CONSOLE_RPC_PATHS.jsonRpc)) {
      const body = JSON.parse(String(init?.body || "{}"));
      methods.push(body.method);
      if (body.method === CONSOLE_RPC_METHODS.inspectIdentity) {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          error: { code: -32000, message: "backend unavailable" },
        }), { status: 200 });
      }
      if (body.method === "meta/inspect_identity") {
        return new Response(JSON.stringify({
          jsonrpc: "2.0",
          id: body.id,
          result: { identity: body.params.identity, status: "legacy-ready" },
        }), { status: 200 });
      }
    }
    return new Response("not found", { status: 404 });
  }) as typeof fetch;

  try {
    const transport = createHttpConsoleTransport({ baseUrl: "http://console.test" });
    await assert.rejects(
      () => transport.executeCommand?.({
        command: CONSOLE_COMMAND_NAMES.inspectIdentity,
        target: identityTarget(),
      }),
      /backend unavailable/,
    );
    assert.deepEqual(methods, [CONSOLE_RPC_METHODS.inspectIdentity]);
  } finally {
    globalThis.fetch = previousFetch;
  }
});

for (const [label, create] of [["stock", createHttpConsoleTransport], ["shared", createSharedHttpConsoleTransport]] as const) {
  test(`${label} HTTP transport reads a stored image same-origin, with no token in the URL, and refuses anything else`, async () => {
    const previousFetch = globalThis.fetch;
    const requests: Array<{ url: string; credentials?: RequestCredentials; authorization: string | null }> = [];
    let reply = () => new Response(new Uint8Array([137, 80, 78, 71]), { status: 200, headers: { "content-type": "image/png" } });
    let hang = false;
    globalThis.fetch = ((url: string | URL | Request, init?: RequestInit) => {
      requests.push({ url: String(url), credentials: init?.credentials, authorization: new Headers(init?.headers).get("authorization") });
      if (hang) {
        return new Promise<Response>((_, reject) => {
          init?.signal?.addEventListener("abort", () => reject(init.signal?.reason), { once: true });
        });
      }
      return Promise.resolve(reply());
    }) as typeof fetch;
    try {
      const transport = create({ baseUrl: "http://gateway.test" });
      const bytes = await transport.readBlob!({ blobId: "sha256:abc" });
      assert.equal(bytes.type, "image/png");
      assert.equal(bytes.size, 4);
      // The browser's same-origin credentials (a session cookie, an
      // authenticating front); the Console adds no header and no URL token.
      assert.deepEqual(requests, [{ url: "http://gateway.test/blobs/sha256%3Aabc", credentials: "same-origin", authorization: null }]);

      // A media failure and an HTTP failure keep their own errors: neither is a
      // timeout or a connection failure.
      const untyped = (pattern: RegExp) => (error: ConsoleTransportFailure) => pattern.test(error.message) && error.transportFailure === undefined;
      for (const contentType of ["text/html", "image/svg+xml", "application/octet-stream"]) {
        reply = () => new Response("<svg onload=alert(1)>", { status: 200, headers: { "content-type": contentType } });
        await assert.rejects(transport.readBlob!({ blobId: "sha256:abc" }), untyped(/not a displayable image/), contentType);
      }
      reply = () => new Response(JSON.stringify({ error: "unauthorized" }), { status: 401 });
      await assert.rejects(transport.readBlob!({ blobId: "sha256:abc" }), untyped(/Blob read failed 401/));

      hang = true;
      const controller = new AbortController();
      const pending = transport.readBlob!({ blobId: "sha256:abc", signal: controller.signal });
      controller.abort();
      await assert.rejects(pending, (error: unknown) => (error as Error).name === "AbortError");
      assert.ok(requests.every(request => !request.url.includes("auth_token")));
    } finally {
      globalThis.fetch = previousFetch;
    }
  });
}

for (const [label, create] of [["stock", createHttpConsoleTransport], ["shared", createSharedHttpConsoleTransport]] as const) {
  test(`${label} HTTP transport times out a stored image whose body stalls after its headers`, { timeout: 5000 }, async () => {
    const previousFetch = globalThis.fetch;
    // 200 raster headers at once, then a body that never arrives. Aborting the
    // request errors the body, as a real fetch does.
    globalThis.fetch = (async (_url: string | URL | Request, init?: RequestInit) => new Response(new ReadableStream<Uint8Array>({
      start(body) {
        init?.signal?.addEventListener("abort", () => body.error(init.signal?.reason), { once: true });
      },
    }), { status: 200, headers: { "content-type": "image/png" } })) as typeof fetch;
    try {
      await assert.rejects(
        create({ baseUrl: "http://gateway.test", fetchTimeoutMs: 50 }).readBlob!({ blobId: "sha256:abc" }),
        (error: ConsoleTransportFailure) => error.transportFailure === "timeout" && error.timeoutMs === 50
          && error.message === "console blob read timeout after 50 ms",
      );
      // The caller cancelling while the body stalls keeps the caller's abort
      // error; it is not reported as a timeout.
      const controller = new AbortController();
      const pending = create({ baseUrl: "http://gateway.test", fetchTimeoutMs: 60_000 })
        .readBlob!({ blobId: "sha256:abc", signal: controller.signal });
      await new Promise(resolve => setTimeout(resolve, 10));
      controller.abort();
      await assert.rejects(pending, (error: ConsoleTransportFailure) => error.name === "AbortError" && error.transportFailure === undefined);
    } finally {
      globalThis.fetch = previousFetch;
    }
  });
}
