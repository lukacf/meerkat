import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { Meta, PersistentTransport, type MetaRuntime } from "../src/index.js";

const fixture = fileURLToPath(new URL("./fixtures/schedule-fire-gateway.cjs", import.meta.url));
const fire = (runtime: MetaRuntime, runnable: string) => runtime._rpc("test/fire", {
  runnable, occurrence: { occurrence_id: "builder-validation" },
});

describe("builder schedule handlers require a declaration at build time", () => {
  it("refuses an undeclared handler before creating a transport and names the exact offending name", async () => {
    for (const declarations of [undefined, [], ["missing"]]) {
      let transports = 0;
      let runtime: MetaRuntime | undefined;
      const name = " missing ";
      const builder = Meta.builder().gateway(fixture)
        .gatewayTransport(({ gatewayBin }) => {
          transports += 1;
          return new PersistentTransport(gatewayBin);
        })
        .onScheduleFire(name, () => null);
      if (declarations !== undefined) builder.hostRunnables(declarations);
      try {
        await assert.rejects(async () => { runtime = await builder.build(); }, (error: unknown) => {
          assert.ok(error instanceof Error);
          assert.match(error.message, /undeclared|not declared/);
          assert.ok(error.message.includes(JSON.stringify(name)), "error must preserve the exact runnable name");
          return true;
        });
        assert.equal(transports, 0, "invalid builder must refuse before transport creation/startup");
      } finally {
        await runtime?.shutdown();
      }
    }
  });

  it("allows handler registration before a later declaration", async () => {
    const runtime = await Meta.builder().gateway(fixture)
      .onScheduleFire("later", async () => "handled")
      .hostRunnables(["later"])
      .build();
    try {
      assert.equal((await fire(runtime, "later") as { result: unknown }).result, "handled");
    } finally {
      await runtime.shutdown();
    }
  });

  it("allows a declared name without a handler", async () => {
    const runtime = await Meta.builder().gateway(fixture).hostRunnables(["no-handler"]).build();
    try {
      assert.deepEqual(await runtime._rpc("test/init-options"), { host_runnables: ["no-handler"] });
      assert.deepEqual((await fire(runtime, "no-handler") as { error: unknown }).error,
        { code: -32000, message: "no schedule-fire handler registered for runnable: no-handler" });
    } finally {
      await runtime.shutdown();
    }
  });

  it("keeps the later duplicate handler for a declared name", async () => {
    const runtime = await Meta.builder().gateway(fixture).hostRunnables(["replace"])
      .onScheduleFire("replace", () => "old")
      .onScheduleFire("replace", () => "new")
      .build();
    try {
      assert.equal((await fire(runtime, "replace") as { result: unknown }).result, "new");
    } finally {
      await runtime.shutdown();
    }
  });

  it("leaves runtime registration and replacement independent of declarations", async () => {
    const runtime = await Meta.builder().gateway(fixture).build();
    try {
      runtime.onScheduleFire("runtime-only", () => "first");
      assert.equal((await fire(runtime, "runtime-only") as { result: unknown }).result, "first");
      runtime.onScheduleFire("runtime-only", async () => "replacement");
      assert.equal((await fire(runtime, "runtime-only") as { result: unknown }).result, "replacement");
      assert.deepEqual(await runtime._rpc("test/init-options"), {}, "registration does not declare a gateway target");
    } finally {
      await runtime.shutdown();
    }
  });
});
