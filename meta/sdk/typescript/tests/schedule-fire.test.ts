/** Host-runnable parity with Python's test_schedule_fire.py, including stdio. */
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

import {
  CallbackDispatcher,
  Meta,
  MetaRuntime,
  type ScheduleFireHandler,
} from "../src/index.js";
import { runtimeConfig } from "./runtime-config.js";

const OCCURRENCE = {
  schedule_id: "sch-1",
  occurrence_id: "occ-1",
  due_at: "2026-07-10T07:00:00+00:00",
  payload: { depth: 3 },
};

function fire(dispatcher: CallbackDispatcher, runnable = "digest", occurrence: unknown = OCCURRENCE) {
  return dispatcher.handleCallback("callback/schedule_fire", { runnable, occurrence });
}

function initOptions(runtime: MetaRuntime): Record<string, unknown> {
  return (runtime as any)._buildInitParams().runtime_options;
}

describe("host runnable declarations", () => {
  it("is chainable and copies the names into runtime_options.host_runnables", () => {
    const names = ["digest", "backup.rotate"];
    const builder = Meta.builder();
    assert.equal(builder.hostRunnables(names), builder);
    names.push("later");
    const runtime = new MetaRuntime(builder._config);
    const options = initOptions(runtime);
    assert.deepEqual(options.host_runnables, ["digest", "backup.rotate"]);
    (options.host_runnables as string[]).push("mutated-wire");
    assert.deepEqual(initOptions(runtime).host_runnables, ["digest", "backup.rotate"]);
  });

  it("omits unset and explicitly empty declarations", () => {
    assert.equal("host_runnables" in initOptions(new MetaRuntime(runtimeConfig())), false);
    const builder = Meta.builder().hostRunnables([]);
    assert.equal("host_runnables" in initOptions(new MetaRuntime(builder._config)), false);
  });

  it("replaces a previous declaration without trimming or deduplicating names", () => {
    const builder = Meta.builder().hostRunnables(["old"]);
    builder.hostRunnables([" digest ", "digest", "digest"]);
    assert.deepEqual(initOptions(new MetaRuntime(builder._config)).host_runnables,
      [" digest ", "digest", "digest"]);
  });

  for (const name of ["", " \t\n", null, 12]) {
    it(`rejects the invalid name ${JSON.stringify(name)} without changing the declaration`, () => {
      const builder = Meta.builder().hostRunnables(["digest"]);
      assert.throws(() => builder.hostRunnables(["backup", name as never]), /non-empty strings/);
      assert.deepEqual(initOptions(new MetaRuntime(builder._config)).host_runnables, ["digest"]);
    });
  }
});

describe("schedule-fire callbacks", () => {
  it("passes the raw occurrence to a sync handler and returns its result", async () => {
    const dispatcher = new CallbackDispatcher();
    const seen: unknown[] = [];
    const handler: ScheduleFireHandler = (occurrence) => {
      seen.push(occurrence);
      return { digest: "sent" };
    };
    dispatcher.registerScheduleFireHandler("digest", handler);
    assert.deepEqual(await fire(dispatcher), { digest: "sent" });
    assert.deepEqual(seen, [OCCURRENCE]);
    assert.equal(seen[0], OCCURRENCE);
  });

  it("awaits an async handler", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerScheduleFireHandler("digest", async (occurrence) => {
      await Promise.resolve();
      return { echo: occurrence.occurrence_id };
    });
    assert.deepEqual(await fire(dispatcher), { echo: "occ-1" });
  });

  it("passes an empty occurrence when omitted, as Python does", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerScheduleFireHandler("digest", (occurrence) => occurrence);
    assert.deepEqual(await dispatcher.handleCallback("callback/schedule_fire", { runnable: "digest" }), {});
  });

  it("preserves explicit occurrence values and extra fields", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerScheduleFireHandler("digest", (occurrence) => occurrence);
    for (const occurrence of [null, 42, ["wire"], { ...OCCURRENCE, future_field: true }]) {
      assert.equal(await fire(dispatcher, "digest", occurrence), occurrence);
    }
  });

  it("awaits thenables and maps a no-return handler to JSON null", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerScheduleFireHandler("digest", () => ({
      then(resolve: (value: string) => void) { resolve("finished"); },
    }));
    assert.equal(await fire(dispatcher), "finished");
    dispatcher.registerScheduleFireHandler("digest", () => {});
    assert.equal(await fire(dispatcher), null);
    dispatcher.registerScheduleFireHandler("digest", async () => {});
    assert.equal(await fire(dispatcher), null);
  });

  it("refuses unknown and missing runnable names", async () => {
    const dispatcher = new CallbackDispatcher();
    await assert.rejects(fire(dispatcher, "nobody"), /no schedule-fire handler registered for runnable: nobody/);
    await assert.rejects(dispatcher.handleCallback("callback/schedule_fire", {}), /no schedule-fire handler/);
  });

  it("propagates sync and async handler errors", async () => {
    const dispatcher = new CallbackDispatcher();
    const failure = new Error("digest failed");
    dispatcher.registerScheduleFireHandler("digest", () => { throw failure; });
    await assert.rejects(fire(dispatcher), (error) => error === failure);
    dispatcher.registerScheduleFireHandler("digest", async () => { throw failure; });
    await assert.rejects(fire(dispatcher), (error) => error === failure);
  });

  it("replaces handlers by name and keeps names and dispatchers independent", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerScheduleFireHandler("digest", () => "old");
    dispatcher.registerScheduleFireHandler("digest", () => "new");
    dispatcher.registerScheduleFireHandler("backup", () => "rotated");
    assert.equal(await fire(dispatcher), "new");
    assert.equal(await fire(dispatcher, "backup"), "rotated");
    await assert.rejects(fire(new CallbackDispatcher()), /no schedule-fire handler/);
  });

  it("validates names and callable handlers at registration", () => {
    const dispatcher = new CallbackDispatcher();
    for (const name of ["", " \t", null, 12]) {
      assert.throws(() => dispatcher.registerScheduleFireHandler(name as never, () => null),
        { name: "TypeError", message: /non-empty string/ });
    }
    for (const handler of [null, "not_a_function", {}]) {
      assert.throws(() => dispatcher.registerScheduleFireHandler("digest", handler as never),
        { name: "TypeError", message: /handler must be callable/ });
    }
  });

  it("registers through MetaRuntime.onScheduleFire", async () => {
    const runtime = new MetaRuntime(runtimeConfig());
    runtime.onScheduleFire("digest", () => ({ ok: true }));
    assert.deepEqual(await fire((runtime as any)._dispatcher), { ok: true });
    assert.throws(() => runtime.onScheduleFire("", () => null), /non-empty string/);
  });
});

describe("host runnable stdio round trip", () => {
  it("sends declarations and returns callback results and failures before and after connect", async () => {
    const fixture = fileURLToPath(new URL("./fixtures/schedule-fire-gateway.cjs", import.meta.url));
    const builder = Meta.builder().hostRunnables(["digest", "empty", "fails"])
      .gateway(fixture);
    const runtime = new MetaRuntime(builder._config);
    runtime.onScheduleFire("digest", (occurrence) => ({ echo: occurrence.occurrence_id }));
    try {
      await runtime.connect();
      assert.deepEqual(await runtime._rpc("test/init-options"), {
        host_runnables: ["digest", "empty", "fails"],
      });
      const request = (runnable: string) => runtime._rpc("test/fire", { runnable, occurrence: OCCURRENCE });
      assert.deepEqual(await request("digest"), { jsonrpc: "2.0", id: "callback-1", result: { echo: "occ-1" } });
      runtime.onScheduleFire("digest", async () => false);
      assert.deepEqual(await request("digest"), { jsonrpc: "2.0", id: "callback-2", result: false });
      runtime.onScheduleFire("empty", () => {});
      assert.deepEqual(await request("empty"), { jsonrpc: "2.0", id: "callback-3", result: null });
      runtime.onScheduleFire("fails", async () => { throw new Error("digest failed"); });
      assert.deepEqual(await request("fails"), {
        jsonrpc: "2.0", id: "callback-4", error: { code: -32000, message: "digest failed" },
      });
      assert.deepEqual(await request("unknown"), {
        jsonrpc: "2.0", id: "callback-5",
        error: { code: -32000, message: "no schedule-fire handler registered for runnable: unknown" },
      });
    } finally {
      await runtime.shutdown();
    }
  });
});
