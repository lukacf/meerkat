/**
 * #306 (0.9.4): Meta hosts no continuity or leases, so neither does the SDK.
 *
 * Core's HeadCanonical store pair is the one session authority, its mob
 * machine and journal are the one identity authority, and Core's realm lock
 * and hosting claim give host exclusivity. The SDK-hosted continuity store,
 * lease provider and scratch directory, their gateway callbacks and init
 * flags, the continuity repair trigger and the lease-loss error code were
 * Meta's second copy of that truth. They are gone. These guards keep them
 * gone and pin that an old call fails loudly with a typed error instead of
 * being accepted and ignored.
 *
 * The session store `sessionService` used to accept, and the config helpers
 * that built one, are gone for the same reason: nothing ever read them, and
 * Core's own session store holds every session.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import * as sdk from "../src/index.js";
import { CallbackDispatcher } from "../src/agent-builder.js";
import { Meta } from "../src/builder.js";
import { RpcError } from "../src/errors.js";
import { MetaRuntime } from "../src/runtime.js";
import { parseIdentityStatus } from "../src/types.js";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..", "src");

// Whole-word identifiers of the removed surfaces. Comments count: a mention
// left behind documents an API that no longer exists.
const REMOVED_NAMES = [
  "continuityStore",
  "leaseProvider",
  "scratchDir",
  "continuity_store",
  "lease_provider",
  "scratch_dir",
  "has_continuity_store",
  "has_lease_provider",
  "registerContinuityStore",
  "registerLeaseProvider",
  "requestContinuityRepair",
  "request_continuity_repair",
  "LEASE_LOST_CODE",
  "LeaseLostError",
  "ContinuityRecord",
  "parseContinuityRecord",
  "continuityRecordToDict",
  "ContinuityFailure",
  "parseContinuityFailure",
  "ContinuityResolveState",
  "parseContinuityResolveState",
  "SessionSnapshot",
  "parseSessionSnapshot",
  "sessionSnapshotToDict",
  "ContinuityStore",
  "LeaseProvider",
  "LeaseGrant",
  "parseLeaseGrant",
  "leaseGrantToDict",
  "LeaseAcquireResult",
  "parseLeaseAcquireResult",
  "leaseAcquireResultToDict",
  "LeaseRenewResult",
  "parseLeaseRenewResult",
  "leaseRenewResultToDict",
  "LeaseInfo",
  "parseLeaseInfo",
  "ContinuityHealth",
  "parseContinuityHealth",
  "continuityHealth",
  "continuity_health",
  "DurabilityPolicy",
  "sessionStore",
  "JsonSessionStoreConfig",
  "jsonSessionStoreConfigToDict",
  "BigQuerySessionStoreConfig",
  "bigquerySessionStoreConfigToDict",
];

const REMOVED_CALLBACKS = [
  "callback/continuity_store/resolve_many",
  "callback/continuity_store/load_session_snapshot",
  "callback/continuity_store/resolve_record_by_session",
  "callback/continuity_store/session_owner",
  "callback/continuity_store/delete_session_snapshot_if_current_revision",
  "callback/continuity_store/save_session_snapshot",
  "callback/continuity_store/upsert_continuity_record",
  "callback/continuity_store/delete_continuity_record",
  "callback/lease_provider/acquire_leases",
  "callback/lease_provider/renew_leases",
  "callback/lease_provider/release_leases",
];

function sourceHits(pattern: RegExp): string[] {
  const hits: string[] = [];
  const walk = (dir: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else if (entry.name.endsWith(".ts")) {
        readFileSync(path, "utf-8")
          .split("\n")
          .forEach((line, index) => {
            if (pattern.test(line)) {
              hits.push(`${path.slice(SRC.length + 1)}:${index + 1}: ${line.trim()}`);
            }
          });
      }
    }
  };
  walk(SRC);
  return hits;
}

function plainRuntime(): MetaRuntime {
  return new MetaRuntime((Meta.builder() as unknown as { _config: never })._config);
}

describe("#306: removed continuity and lease surfaces", () => {
  for (const name of REMOVED_NAMES) {
    it(`no source names ${name}`, () => {
      assert.deepEqual(sourceHits(new RegExp(`\\b${name}\\b`)), []);
    });
  }

  it("no source uses the retired lease-lost code", () => {
    assert.deepEqual(sourceHits(/\b32005\b/), []);
  });

  for (const option of ["continuityStore", "leaseProvider", "scratchDir"]) {
    it(`the builder offers no ${option}()`, () => {
      const builder = Meta.builder() as unknown as Record<string, unknown>;
      assert.equal(builder[option], undefined);
      assert.ok(!(option in (builder._config as Record<string, unknown>)));
    });
  }

  it("the runtime has no continuity repair trigger", () => {
    assert.equal(
      (MetaRuntime.prototype as unknown as Record<string, unknown>).requestContinuityRepair,
      undefined,
    );
  });

  for (const name of ["LEASE_LOST_CODE", "LeaseLostError"]) {
    it(`the package exports no ${name}`, () => {
      assert.ok(!(name in sdk));
    });
  }

  it("a retired -32005 answer is the plain RpcError", async () => {
    const rt = plainRuntime() as unknown as Record<string, unknown>;
    rt._running = true;
    rt._transport = {
      sendAsync: async (request: Record<string, unknown>) => ({
        jsonrpc: "2.0",
        id: request.id,
        error: { code: -32005, message: "lease lost" },
      }),
    };
    await assert.rejects(
      (rt as unknown as { _rpc(method: string, params: unknown): Promise<unknown> })._rpc(
        "meta/send",
        {},
      ),
      (error: unknown) =>
        error instanceof RpcError &&
        Object.getPrototypeOf(error) === RpcError.prototype &&
        error.code === -32005,
    );
  });

  for (const name of ["registerContinuityStore", "registerLeaseProvider"]) {
    it(`the dispatcher has no ${name}()`, () => {
      assert.equal(
        (CallbackDispatcher.prototype as unknown as Record<string, unknown>)[name],
        undefined,
      );
    });
  }

  for (const method of REMOVED_CALLBACKS) {
    it(`the dispatcher refuses ${method} as an unknown callback`, async () => {
      // An older gateway that still calls a provider gets the dispatcher's
      // typed unknown-method refusal, never an empty answer.
      await assert.rejects(
        new CallbackDispatcher().handleCallback(method, {}),
        /unknown callback method/,
      );
    });
  }

  it("sessionService takes no session store", () => {
    // The store was accepted and never read; an old call now fails at once.
    const builder = Meta.builder();
    const call = builder.sessionService as unknown as (...args: unknown[]) => unknown;
    assert.throws(
      () => call.call(builder, { buildAgent: async () => {} }, { type: "json" }),
      TypeError,
    );
    assert.ok(!("sessionStore" in (builder._config as unknown as Record<string, unknown>)));
  });

  it("the package exports no sessionStore config helpers", () => {
    // sessionStore.json() and sessionStore.bigquery() built a store
    // declaration that nothing ever sent.
    assert.ok(!("sessionStore" in sdk));
  });

  it("identity-first init params carry no continuity or lease flags", () => {
    const builder = Meta.builder().rosterProvider({ async roster() { return []; } });
    const params = (
      new MetaRuntime((builder as unknown as { _config: never })._config) as unknown as {
        _buildInitParams(): Record<string, unknown>;
      }
    )._buildInitParams();
    assert.equal(params.has_roster_provider, true);
    for (const key of ["has_continuity_store", "has_lease_provider", "scratch_dir"]) {
      assert.ok(!(key in params), key);
    }
  });

  it("IdentityStatus has no lease or continuityHealth", () => {
    // A gateway that still sends them is read without them.
    const status = parseIdentityStatus({
      identity: "w",
      state: "active",
      lease: { fencing_token: 3, ttl_remaining_ms: 10, healthy: true },
      continuity_health: {
        store_reachable: true,
        durability_policy: { kind: "sync_write_through" },
      },
    }) as unknown as Record<string, unknown>;
    assert.ok(!("lease" in status));
    assert.ok(!("continuityHealth" in status));
  });
});
