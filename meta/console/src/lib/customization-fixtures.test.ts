import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

import {
  SIDEBAR_PINS_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  applyConsoleSidebarOrder,
  migrateConsoleWorkbenchTarget,
  readSidebarStringList,
  readSidebarStringSet,
  sidebarStorageKey,
  writeSidebarStringList,
  writeSidebarStringSet,
} from "@console-core";
import { CONSOLE_REST_PATHS, CONSOLE_RPC_METHODS, CONSOLE_RPC_PATHS } from "./contract";
import {
  createMetaConsoleController,
  type ConsoleCapabilities,
  type MetaConsoleTransport,
} from "./headless";
import type { ConsoleFrame, ConsoleTimelineAccepted, ConsoleTimelinePage } from "../types";

const root = path.basename(process.cwd()) === "console"
  ? path.resolve(process.cwd(), "..")
  : process.cwd();

function fixture(name: string): Record<string, unknown> {
  return JSON.parse(fs.readFileSync(
    path.join(root, "console", "fixtures", name, "fixture.json"),
    "utf8",
  )) as Record<string, unknown>;
}

test("reference-wrapper fixture keeps host status separate from Meta console protocol", () => {
  const wrapper = fixture("reference-wrapper");
  const consoleRoutes = wrapper.consoleRoutes as Record<string, string>;
  const hostRoutes = wrapper.hostRoutes as Record<string, Record<string, unknown>>;
  const dispatch = createReferenceWrapperDispatch(wrapper);

  for (const route of [
    "/console",
    "/console/rpc",
    "/console/timeline",
    "/console/timeline/stream",
    "/blobs/{blob_id}",
  ]) {
    assert.equal(consoleRoutes[route], "meta-reference-console");
  }
  assert.equal(hostRoutes["/host/status"].provenance, "host-adapter");
  assert.equal(hostRoutes["/host/status"].consumedByMetaHeadless, false);

  assert.deepEqual(dispatch({ method: "GET", path: "/host/status" }), {
    source: "host-adapter",
    status: 200,
    headers: {},
    body: hostRoutes["/host/status"],
  });

  const jsResponse = dispatch({ method: "GET", path: "/console/assets/console-app.js" });
  assert.equal(jsResponse.source, "meta-reference-console");
  assert.equal(jsResponse.status, 200);
  assert.equal(jsResponse.headers["access-control-allow-origin"], undefined);
  assert.equal(
    (jsResponse.body as Record<string, unknown>).javascript,
    fs.readFileSync(path.join(root, "crates", "meerkat-meta", "console-dist", "console-app.js"), "utf8"),
    "reference wrapper must serve the embedded console JS without rewriting it",
  );

  const rpcResponse = dispatch({ method: "POST", path: CONSOLE_RPC_PATHS.jsonRpc });
  assert.equal(rpcResponse.source, "meta-reference-console");
  assert.equal(rpcResponse.headers["access-control-allow-origin"], undefined);
  assert.equal((rpcResponse.body as Record<string, unknown>).route, CONSOLE_RPC_PATHS.jsonRpc);
  assert.equal((rpcResponse.body as Record<string, unknown>).hostStatus, undefined);

  const timelineResponse = dispatch({ method: "GET", path: CONSOLE_REST_PATHS.timeline });
  assert.equal(timelineResponse.source, "meta-reference-console");
  assert.equal(timelineResponse.headers["access-control-allow-origin"], undefined);
  assert.deepEqual(timelineResponse.body, {
    route: CONSOLE_REST_PATHS.timeline,
    frames: [],
  });
});

test("custom-host-shell fixture maps host records to explicit Meta targets through a non-HTTP adapter", async () => {
  const custom = fixture("custom-host-shell");
  const transport = custom.rendererTransport as Record<string, unknown>;
  const records = custom.hostRecords as Array<Record<string, unknown>>;
  const navigation = custom.hostNavigation as Record<string, unknown>;
  const components = custom.metaComponents as string[];

  assert.equal(transport.kind, "host-adapter");
  assert.equal(transport.directBrowserRpc, false);
  assert.equal(transport.rawRuntimeCalls, false);
  assert.equal(transport.preservesTypedErrors, true);
  assert.equal(transport.preservesCursors, true);
  assert.equal(transport.preservesBlobs, true);
  assert.equal(navigation.orientation, "horizontal");
  assert.equal(navigation.bindsMetaTargets, true);
  assert.deepEqual(components.sort(), [
    "ConsoleComposer",
    "ConsoleDock",
    "ConversationPane",
    "ConversationTranscript",
  ].sort());

  for (const record of records) {
    assert.match(String(record.kind), /^host\//);
    const target = record.metaTarget as Record<string, unknown>;
    assert.equal(target.kind, "meta/identity-chat");
    assert.match(String(target.identity), /^identity:/);
  }

  const adapter = createFixtureHostAdapter(records);
  const controller = createMetaConsoleController({ transport: adapter });
  const selectedRecord = records[1]!;
  const selectedTarget = migrateConsoleWorkbenchTarget(selectedRecord.metaTarget);
  assert.ok(selectedTarget);

  const page = await controller.timeline.query({ identity: String((selectedRecord.metaTarget as Record<string, unknown>).identity) });
  assert.equal(page.provenance.source, "meta-protocol");
  assert.equal(page.value.frames[0]?.data.identity, "identity:thread-alpha-plan");

  const accepted = await controller.commands.sendMessage(selectedTarget, {
    content: "hello from host shell",
    origin: "fixture",
    idempotencyKey: "fixture-send-1",
  });
  assert.equal(accepted.accepted.value.identity, "identity:thread-alpha-plan");
  assert.deepEqual(adapter.sends, [{
    identity: "identity:thread-alpha-plan",
    content: "hello from host shell",
    origin: "fixture",
    idempotencyKey: "fixture-send-1",
  }]);
});

test("configured-host-shell fixture keeps host mutations out of console.toml buttons and protects direct console routes", () => {
  const configured = fixture("configured-host-shell");
  const toolbar = configured.hostToolbarActions as Array<Record<string, unknown>>;
  const sidebarButtons = configured.sidebarButtons as Record<string, unknown>;
  const persistedPreferences = configured.persistedPreferences as Record<string, unknown>;
  const routeProtection = configured.directRouteProtectionWhenMetaAuthDisabled as Record<string, string>;
  const proxy = createConfiguredHostProxy(routeProtection);
  const storage = new MemoryStorage();
  const namespace = String(persistedPreferences.namespace);
  const sectionKey = sidebarStorageKey(SIDEBAR_SECTION_ORDER_STORAGE_PREFIX, namespace);
  const pinKey = sidebarStorageKey(SIDEBAR_PINS_STORAGE_PREFIX, namespace);

  assert.equal(toolbar[0].provenance, "host-adapter");
  assert.equal(toolbar[0].requiresConfirmation, true);
  assert.match(String(toolbar[0].postRoute), /^\/host\/actions\//);
  assert.equal(sidebarButtons.mutationSemantics, false);
  assert.deepEqual(sidebarButtons.allowedFields, ["id", "label", "href", "target", "control", "icon_name"]);

  writeSidebarStringList(storage, sectionKey, persistedPreferences.storedSectionOrder as string[]);
  writeSidebarStringSet(storage, pinKey, new Set(persistedPreferences.storedPins as string[]));
  assert.deepEqual(
    applyConsoleSidebarOrder(
      persistedPreferences.configuredSectionOrder as string[],
      readSidebarStringList(storage, sectionKey),
    ),
    persistedPreferences.expectedSectionOrder,
  );
  assert.deepEqual(
    Array.from(readSidebarStringSet(storage, pinKey) || []),
    persistedPreferences.expectedPins,
  );

  for (const route of [
    "/console",
    "/console/",
    "/console/assets/console-app.js",
    "/console/assets/console-app.css",
    CONSOLE_REST_PATHS.experience,
    CONSOLE_REST_PATHS.modules,
    CONSOLE_REST_PATHS.identities,
    "/console/rpc",
    "/console/rpc/multipart",
    CONSOLE_REST_PATHS.timeline,
    CONSOLE_REST_PATHS.timelineStream,
    CONSOLE_REST_PATHS.identityTimelineStreamTemplate,
    CONSOLE_REST_PATHS.legacySend,
    "/blobs/{blob_id}",
  ]) {
    assert.equal(routeProtection[route], "host-proxy");
    assert.deepEqual(proxy({ method: "GET", path: route }), {
      source: "host-proxy",
      status: 401,
      body: { error: "host_auth_required" },
      headers: { "x-meta-console-protected": "true" },
    });
    assert.deepEqual(proxy({ method: "GET", path: route, hostAuthorized: true }), {
      source: "meta-reference-console",
      status: 200,
      body: { route },
      headers: {},
    });
  }
});

type WrapperResponse = {
  source: string;
  status: number;
  headers: Record<string, string>;
  body: unknown;
};

function createReferenceWrapperDispatch(wrapper: Record<string, unknown>) {
  const consoleRoutes = wrapper.consoleRoutes as Record<string, string>;
  const hostRoutes = wrapper.hostRoutes as Record<string, Record<string, unknown>>;
  const consoleIndex = fs.readFileSync(
    path.join(root, "crates", "meerkat-meta", "console-dist", "index.html"),
    "utf8",
  );

  const consoleJs = fs.readFileSync(
    path.join(root, "crates", "meerkat-meta", "console-dist", "console-app.js"),
    "utf8",
  );

  return ({ path: requestPath }: { method: "GET" | "POST"; path: string }): WrapperResponse => {
    if (hostRoutes[requestPath]) {
      return { source: "host-adapter", status: 200, headers: {}, body: hostRoutes[requestPath] };
    }
    if (!consoleRoutes[requestPath]) {
      return { source: "wrapper", status: 404, headers: {}, body: { error: "not_found" } };
    }
    if (requestPath === "/console") {
      return { source: consoleRoutes[requestPath], status: 200, headers: {}, body: { html: consoleIndex } };
    }
    if (requestPath === "/console/assets/console-app.js") {
      return { source: consoleRoutes[requestPath], status: 200, headers: {}, body: { javascript: consoleJs } };
    }
    if (requestPath === CONSOLE_REST_PATHS.timeline) {
      return { source: consoleRoutes[requestPath], status: 200, headers: {}, body: { route: requestPath, frames: [] } };
    }
    return { source: consoleRoutes[requestPath], status: 200, headers: {}, body: { route: requestPath } };
  };
}

function createConfiguredHostProxy(routeProtection: Record<string, string>) {
  return ({ path: requestPath, hostAuthorized }: { method: "GET" | "POST"; path: string; hostAuthorized?: boolean }): WrapperResponse => {
    if (routeProtection[requestPath] === "host-proxy" && !hostAuthorized) {
      return {
        source: "host-proxy",
        status: 401,
        headers: { "x-meta-console-protected": "true" },
        body: { error: "host_auth_required" },
      };
    }
    return {
      source: "meta-reference-console",
      status: 200,
      headers: {},
      body: { route: requestPath },
    };
  };
}

function createFixtureHostAdapter(records: Array<Record<string, unknown>>): MetaConsoleTransport & {
  sends: Array<Record<string, unknown>>;
} {
  const targets = records.map((record) => record.metaTarget as Record<string, unknown>);
  const identities = new Set(targets.map((target) => String(target.identity)));
  const adapter = {
    sends: [] as Array<Record<string, unknown>>,
    loadExperience: async () => ({ contract_version: "fixture" }),
    capabilities: async (): Promise<ConsoleCapabilities> => ({
      version: "fixture-adapter",
      methods: [CONSOLE_RPC_METHODS.send, CONSOLE_RPC_METHODS.queryTimeline],
    }),
    queryTimeline: async (input): Promise<ConsoleTimelinePage> => {
      assert.ok(input.identity && identities.has(input.identity));
      const frame: ConsoleFrame = {
        id: `fixture:${input.identity}:1`,
        event: "text_delta",
        cursor: "fixture:1",
        data: { identity: input.identity },
      };
      return { frames: [frame], latestCursor: "fixture:1", available: true };
    },
    subscribeTimeline: () => () => undefined,
    send: async (input): Promise<ConsoleTimelineAccepted> => {
      assert.ok(identities.has(input.identity));
      adapter.sends.push({
        identity: input.identity,
        content: input.content,
        origin: input.origin,
        idempotencyKey: input.idempotencyKey,
      });
      return {
        interaction_id: `fixture:${input.idempotencyKey}`,
        identity: input.identity,
        cursor: "fixture:2",
      };
    },
  } satisfies MetaConsoleTransport & { sends: Array<Record<string, unknown>> };
  return adapter;
}

class MemoryStorage {
  private values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }
}
