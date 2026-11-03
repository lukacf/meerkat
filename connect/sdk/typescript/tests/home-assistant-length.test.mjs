import test from "node:test";
import assert from "node:assert/strict";
import {
  HomeAssistantOperations, HomeAssistantError,
} from "@rkat/toolkit/connectors/home-assistant";

// The public host-injected Fetch contract can return a synthetic Response, so
// operation validation cannot depend on a particular network Fetch engine.
for (const operation of ["read", "write"]) {
  for (const delta of [-1, 1, 0, null]) {
    test(`${operation}: declared byte length ${delta === null ? "absent" : delta}`, async () => {
      const entity = { entity_id: "light.kitchen", state: "on", attributes: { name: "kök" } };
      const bytes = new TextEncoder().encode(JSON.stringify(operation === "read" ? entity : [entity]));
      let calls = 0;
      let response;
      const api = new HomeAssistantOperations({
        baseUrl: "https://ha.example.test/selected/",
        fetch: async (request) => {
          calls++;
          assert.equal(request.method, operation === "read" ? "GET" : "POST");
          assert.equal(request.redirect, "manual");
          assert.equal(request.headers.get("accept-encoding"), "identity");
          const headers = { "Content-Type": "application/json" };
          if (delta !== null) headers["Content-Length"] = String(bytes.length + delta);
          response = new Response(new ReadableStream({
            start(controller) {
              controller.enqueue(bytes.subarray(0, bytes.length - 1));
              controller.enqueue(bytes.subarray(bytes.length - 1));
              controller.close();
            },
          }), { status: 200, headers });
          return response;
        },
      });
      const work = operation === "read"
        ? api.getState("light.kitchen") : api.callService("light", "turn_on");
      if (delta === -1 || delta === 1) {
        await assert.rejects(work, (error) => {
          assert.ok(error instanceof HomeAssistantError);
          assert.equal(error.kind, "invalid_response");
          assert.equal(error.writeOutcome, operation === "read" ? "not_applicable" : "unknown");
          assert.equal(error.statusCode, 200);
          assert.equal(error.cause, undefined);
          return true;
        });
      } else {
        const result = await work;
        assert.deepEqual(operation === "read" ? result : result.changedStates, operation === "read" ? entity : [entity]);
      }
      assert.equal(calls, 1);
      assert.equal(response.bodyUsed, true);
      assert.equal(response.body.locked, false);
    });
  }
}
