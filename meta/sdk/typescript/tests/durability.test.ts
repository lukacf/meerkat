/**
 * Tests for the M5 storage durability vocabulary: the typed config blocks
 * (`runtimeStore` / `eventLog`), their exact `runtime_options` wire forms,
 * the typed storage census model (`StorageSummary` / `StorageSlotSummary`)
 * parsed from `meta/status` / `meta/capabilities`, and the
 * `StorageResolutionError` (-32014) reification for the gateway's
 * fail-closed init refusals.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  Meta,
  MetaRuntime,
  STORAGE_RESOLUTION_CODE,
  StorageResolutionError,
  RpcError,
  MetaError,
  eventLog,
  runtimeStore,
  isRpcError,
  parseStatusResult,
  parseCapabilitiesResult,
  parseStorageSummary,
} from "../dist/index.js";

const CENSUS_WIRE = {
  blob_durability: "persistent_disk",
  blob_store_persistent: true,
  session_store_incremental: false,
  slots: [
    {
      domain: "runtime",
      class: "durable",
      resolution: "persistent",
      backend: "SqliteRuntimeStore",
      degraded: false,
    },
    {
      domain: "schedule",
      class: "durable",
      resolution: "non_persistent",
      backend: "disabled",
      degraded: true,
      detail: "schedule store failed to open",
    },
    {
      domain: "gating_audit",
      class: "scratch",
      resolution: "declared_ephemeral",
      backend: "in-process ring buffer",
      degraded: false,
      detail: "drop-oldest retention (512 entries)",
    },
  ],
};

function initParams(builder: unknown): Record<string, any> {
  const config = (builder as { _config: unknown })._config;
  const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
  return (rt as any)._buildInitParams();
}

describe("prepared application carrier", () => {
  it("is sent as prepared_application, never as mob_config", () => {
    const params = initParams(
      Meta.builder().preparedApplication(new Uint8Array([0, 112, 255]), "sha256:abc"),
    );
    assert.deepEqual(params.prepared_application, {
      bundle_base64: "AHD/",
      digest: "sha256:abc",
    });
    assert.equal(params.mob_config, undefined);
  });
});

describe("runtimeStore config", () => {
  it("memory() produces the exact wire form", () => {
    assert.deepEqual(runtimeStore.memory().toDict(), { storage: "memory" });
  });

  it("builder emits runtime_options.runtime_store", () => {
    const b = Meta.builder().runtimeStore(runtimeStore.memory());
    const params = initParams(b);
    assert.deepEqual(params.runtime_options.runtime_store, {
      storage: "memory",
    });
  });

  it("builder accepts the raw wire dict", () => {
    const b = Meta.builder().runtimeStore({ storage: "memory" });
    const params = initParams(b);
    assert.deepEqual(params.runtime_options.runtime_store, {
      storage: "memory",
    });
  });

  it("is omitted by default", () => {
    const params = initParams(Meta.builder());
    assert.equal("runtime_store" in params.runtime_options, false);
  });
});

describe("eventLog config", () => {
  it("memory() produces the exact wire form", () => {
    assert.deepEqual(eventLog.memory().toDict(), { storage: "memory" });
    assert.deepEqual(
      eventLog.memory({ batchSize: 1, flushIntervalMs: 10 }).toDict(),
      { storage: "memory", batch_size: 1, flush_interval_ms: 10 },
    );
  });

  it("nullStore() produces the exact wire form", () => {
    assert.deepEqual(eventLog.nullStore().toDict(), { storage: "null" });
  });

  it("memory() rejects a non-positive flushIntervalMs", () => {
    // A zero interval would panic the gateway's ingestion task.
    assert.throws(() => eventLog.memory({ flushIntervalMs: 0 }), /flushIntervalMs/);
    assert.throws(() => eventLog.memory({ flushIntervalMs: -5 }), /flushIntervalMs/);
  });

  it("memory() rejects a non-finite or non-integer flushIntervalMs", () => {
    assert.throws(
      () => eventLog.memory({ flushIntervalMs: Number.NaN }),
      /flushIntervalMs/,
    );
    assert.throws(
      () => eventLog.memory({ flushIntervalMs: Number.POSITIVE_INFINITY }),
      /flushIntervalMs/,
    );
    assert.throws(() => eventLog.memory({ flushIntervalMs: 10.5 }), /flushIntervalMs/);
  });

  it("builder accepts the typed declaration", () => {
    const b = Meta.builder().eventLog(eventLog.nullStore());
    const params = initParams(b);
    assert.deepEqual(params.runtime_options.event_log, { storage: "null" });
  });

  it("builder keeps the legacy raw form working", () => {
    const b = Meta.builder().eventLog({ storage: "memory", batch_size: 2 });
    const params = initParams(b);
    assert.deepEqual(params.runtime_options.event_log, {
      storage: "memory",
      batch_size: 2,
    });
  });
});

describe("storage census model", () => {
  it("parses the M4 wire shape", () => {
    const summary = parseStorageSummary(CENSUS_WIRE);
    assert.equal(summary.blobDurability, "persistent_disk");
    assert.equal(summary.blobStorePersistent, true);
    assert.equal(summary.sessionStoreIncremental, false);
    assert.equal(summary.slots.length, 3);
    const runtimeSlot = summary.slots.find((s) => s.domain === "runtime");
    assert.ok(runtimeSlot);
    assert.equal(runtimeSlot.durabilityClass, "durable");
    assert.equal(runtimeSlot.resolution, "persistent");
    assert.equal(runtimeSlot.backend, "SqliteRuntimeStore");
    assert.equal(runtimeSlot.degraded, false);
    assert.equal(runtimeSlot.detail, undefined);
    const scheduleSlot = summary.slots.find((s) => s.domain === "schedule");
    assert.ok(scheduleSlot);
    assert.equal(scheduleSlot.degraded, true);
    assert.equal(scheduleSlot.detail, "schedule store failed to open");
  });

  it("is forward-tolerant", () => {
    const summary = parseStorageSummary({
      blob_durability: "some_future_kind",
      blob_store_persistent: true,
      slots: [
        {
          domain: "future",
          class: "warm_tier",
          resolution: "replicated",
          backend: "S3",
          future_field: { nested: true },
        },
      ],
      future_top_level: 7,
    });
    assert.equal(summary.blobDurability, "some_future_kind");
    assert.equal(summary.sessionStoreIncremental, null);
    assert.equal(summary.slots[0].durabilityClass, "warm_tier");
    assert.equal(summary.slots[0].resolution, "replicated");
  });

  it("parses the pre-census shape with empty slots", () => {
    const summary = parseStorageSummary({
      blob_durability: "declared_ephemeral",
      blob_store_persistent: false,
    });
    assert.deepEqual(summary.slots, []);
  });

  it("rides StatusResult when present", () => {
    const status = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
      storage: CENSUS_WIRE,
    });
    assert.ok(status.storage);
    assert.equal(
      status.storage.slots.find((s) => s.domain === "runtime")?.backend,
      "SqliteRuntimeStore",
    );
  });

  it("ignores StatusResult fields it does not model", () => {
    const status = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
      reserved_identity_members: [
        { member_id: "worker-1", runtime_id: "worker-1:0", identity: "_system" },
      ],
    });
    assert.deepEqual(status, {
      contractVersion: "0.5.0",
      running: true,
      loadedModules: [],
    });
  });

  it("ignores the non-expiring console principals StatusResult does not model", () => {
    const status = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
      non_expiring_console_principals: [
        { principal: "svc:reporter", non_expiring: true },
      ],
    });
    assert.deepEqual(status, {
      contractVersion: "0.5.0",
      running: true,
      loadedModules: [],
    });
  });

  it("ignores the mob failure StatusResult does not model", () => {
    const status = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
      mob_state: "Some(Running)",
      mob_failure: "the mob's actor stopped",
    });
    assert.deepEqual(status, {
      contractVersion: "0.5.0",
      running: true,
      loadedModules: [],
    });
  });

  it("is absent from StatusResult when the wire has none", () => {
    const status = parseStatusResult({
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
    });
    assert.equal(status.storage, undefined);
  });

  it("rides CapabilitiesResult when present", () => {
    const capabilities = parseCapabilitiesResult({
      contract_version: "0.5.0",
      methods: ["meta/status"],
      loaded_modules: [],
      storage: CENSUS_WIRE,
    });
    assert.ok(capabilities.storage);
    assert.equal(capabilities.storage.blobDurability, "persistent_disk");
  });
});

describe("StorageResolutionError", () => {
  it("is a typed RpcError with the pinned code", () => {
    const err = new StorageResolutionError(
      "file-name twins for the sessions store: run the storage doctor (meta/storage/doctor)",
      "init:1",
      "meta/init",
    );
    assert.ok(err instanceof StorageResolutionError);
    assert.ok(err instanceof RpcError);
    assert.ok(err instanceof MetaError);
    assert.equal(err.code, STORAGE_RESOLUTION_CODE);
    assert.equal(STORAGE_RESOLUTION_CODE, -32014);
    assert.equal(err.name, "StorageResolutionError");
  });

  it("passes the structural isRpcError check across module splits", () => {
    const structural = {
      name: "StorageResolutionError",
      code: STORAGE_RESOLUTION_CODE,
    };
    assert.equal(isRpcError(structural), true);
  });
});
