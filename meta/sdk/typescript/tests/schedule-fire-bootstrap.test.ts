import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { Meta, type MetaRuntime } from "../src/index.js";

const fixture = fileURLToPath(new URL("./fixtures/schedule-fire-bootstrap-gateway.cjs", import.meta.url));
const builder = () => Meta.builder().gateway(fixture).hostRunnables(["bootstrap"]);

describe("host runnable bootstrap registration", () => {
  it("registers through the public builder before the first init callback, with no-return success", async () => {
    let returned = false;
    const seen: unknown[] = [];
    const configured = builder();
    assert.equal(configured.onScheduleFire("bootstrap", (occurrence) => {
      assert.equal(returned, false, "callback must finish before build returns");
      seen.push(occurrence);
    }), configured);
    const runtime = await configured.build();
    returned = true;
    try {
      assert.deepEqual(seen, [{ schedule_id: "stored", occurrence_id: "due-at-boot", payload: { boot: true } }]);
      assert.deepEqual(await runtime._rpc("test/bootstrap"), {
        initOptions: { host_runnables: ["bootstrap"] },
        callback: { jsonrpc: "2.0", id: "init-fire", result: null },
      }, "handler functions remain host-local and are never serialized");
    } finally {
      await runtime.shutdown();
    }
  });

  it("keeps a runtime replacement across reconnect instead of reinstalling builder defaults", async () => {
    const seen: string[] = [];
    const configured = builder().onScheduleFire("bootstrap", () => { seen.push("initial"); });
    const runtime = await configured.build();
    try {
      await runtime.shutdown();
      runtime.onScheduleFire("bootstrap", () => { seen.push("replacement"); });
      configured.onScheduleFire("bootstrap", () => { seen.push("later-builder-default"); });
      await runtime.connect();
      assert.deepEqual(seen, ["initial", "replacement"]);
    } finally {
      await runtime.shutdown();
    }
  });

  it("validates before replacing, preserves exact names, and copies handlers into independent builds", async () => {
    const seen: string[] = [];
    const configured = builder().hostRunnables([" bootstrap "])
      .onScheduleFire(" bootstrap ", () => { seen.push("old"); })
      .onScheduleFire(" bootstrap ", () => { seen.push("first"); });
    for (const name of ["", " \t", null, 12]) {
      assert.throws(() => configured.onScheduleFire(name as never, () => {}), /non-empty string/);
    }
    for (const handler of [null, "not-a-function", {}]) {
      assert.throws(() => configured.onScheduleFire(" bootstrap ", handler as never), /handler must be callable/);
    }
    const first = await configured.build();
    let second: MetaRuntime | undefined;
    try {
      second = await configured.onScheduleFire(" bootstrap ", () => { seen.push("second"); }).build();
      await first.shutdown();
      await first.connect();
      assert.deepEqual(seen, ["first", "second", "first"]);
      first.onScheduleFire(" bootstrap ", () => { seen.push("first-only"); });
      await second.shutdown();
      await second.connect();
      assert.deepEqual(seen, ["first", "second", "first", "second"]);
    } finally {
      await first.shutdown();
      await second?.shutdown();
    }
  });
});
