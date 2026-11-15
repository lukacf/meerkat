/**
 * Queued-input recall wrapper (`session/recall_input`) sends the exact RPC
 * literal with the `session/input_status` selector and validates the typed
 * answer union on its `outcome` discriminator.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { MeerkatError } from "../dist/index.js";
import { DeferredSession, Session } from "../dist/session.js";

const INPUT_ID = "01936f8b-0000-7000-8000-0000000000a1";
const RUN_ID = "01936f8b-0000-7000-8000-000000000042";

function fakeClient(results) {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push({ method, params });
    const result = results[method];
    if (!result) {
      throw new Error(`no canned result for ${method}`);
    }
    return structuredClone(result);
  };
  return { client, calls };
}

function sessionOver(client) {
  return new Session(client, { sessionId: "s-1", sessionRef: undefined });
}

describe("queued-input recall wrapper", () => {
  it("Session.recallInput by input id issues session/recall_input and returns abandoned", async () => {
    const recall = { outcome: "abandoned", input_id: INPUT_ID, attempt_count: 0 };
    const { client, calls } = fakeClient({
      "session/recall_input": { session_id: "s-1", recall },
    });

    const result = await sessionOver(client).recallInput({ inputId: INPUT_ID });

    assert.deepEqual(calls, [
      {
        method: "session/recall_input",
        params: { session_id: "s-1", selector: { by: "input_id", value: INPUT_ID } },
      },
    ]);
    assert.deepEqual(result, { session_id: "s-1", recall });
  });

  it("Session.recallInput by idempotency key sends the key selector", async () => {
    const recall = { outcome: "already_terminal", input_id: INPUT_ID, terminal: "cancelled" };
    const { client, calls } = fakeClient({
      "session/recall_input": { session_id: "s-1", recall },
    });

    const result = await sessionOver(client).recallInput({ idempotencyKey: "inbox:msg-1" });

    assert.deepEqual(calls, [
      {
        method: "session/recall_input",
        params: {
          session_id: "s-1",
          selector: { by: "idempotency_key", value: "inbox:msg-1" },
        },
      },
    ]);
    assert.deepEqual(result.recall, recall);
  });

  it("not_queued carries the phase and the run", async () => {
    const recall = { outcome: "not_queued", input_id: INPUT_ID, phase: "staged", run_id: RUN_ID };
    const { client } = fakeClient({
      "session/recall_input": { session_id: "s-1", recall },
    });
    const result = await sessionOver(client).recallInput({ inputId: INPUT_ID });
    assert.deepEqual(result.recall, recall);
  });

  it("an abandoned answer without an attempt count reads zero", async () => {
    const { client } = fakeClient({
      "session/recall_input": {
        session_id: "s-1",
        recall: { outcome: "abandoned", input_id: INPUT_ID },
      },
    });
    const result = await sessionOver(client).recallInput({ inputId: INPUT_ID });
    assert.equal(result.recall.attempt_count, 0);
  });

  for (const malformed of [
    { session_id: "s-1" },
    { recall: { outcome: "abandoned", input_id: INPUT_ID } },
    { session_id: "s-1", recall: "abandoned" },
    { session_id: "s-1", recall: { outcome: "abandoned" } },
    { session_id: "s-1", recall: { outcome: 7, input_id: INPUT_ID } },
    { session_id: "s-1", recall: { outcome: "", input_id: INPUT_ID } },
    { session_id: "s-1", recall: { outcome: "not_queued", input_id: INPUT_ID } },
    {
      session_id: "s-1",
      recall: { outcome: "not_queued", input_id: INPUT_ID, phase: "staged", run_id: 7 },
    },
    { session_id: "s-1", recall: { outcome: "already_terminal", input_id: INPUT_ID } },
    {
      session_id: "s-1",
      recall: { outcome: "abandoned", input_id: INPUT_ID, attempt_count: "0" },
    },
    {
      session_id: "s-1",
      recall: { outcome: "abandoned", input_id: INPUT_ID, attempt_count: -1 },
    },
  ]) {
    it(`a malformed session/recall_input result fails closed: ${JSON.stringify(malformed)}`, async () => {
      const { client } = fakeClient({ "session/recall_input": malformed });
      await assert.rejects(client._recallInput("s-1", { inputId: INPUT_ID }), {
        code: "INVALID_RESPONSE",
      });
    });
  }

  it("an outcome this SDK does not know is kept as it is", async () => {
    // The list of outcomes can grow: an outcome this SDK does not know is
    // returned unchanged, with every field the server sent, and rejects nothing.
    const recall = { outcome: "kept_for_the_next_start", input_id: INPUT_ID, until: "later" };
    const { client } = fakeClient({
      "session/recall_input": { session_id: "s-1", recall },
    });

    const result = await sessionOver(client).recallInput({ inputId: INPUT_ID });

    assert.deepEqual(result.recall, recall);
  });

  it("the deferred session exposes the same wrapper", () => {
    assert.equal(typeof DeferredSession.prototype.recallInput, "function");
  });

  it("a server without the method surfaces method not found", async () => {
    // A server that does not serve the method answers method not found
    // (-32601). The wrapper sends the one call, reads nothing of the server
    // first, and the transport's error reaches the caller unchanged.
    const client = new MeerkatClient();
    const calls = [];
    client.request = async (method) => {
      calls.push(method);
      throw new MeerkatError("-32601", "Method not found: session/recall_input");
    };

    await assert.rejects(sessionOver(client).recallInput({ inputId: INPUT_ID }), (error) => {
      assert.ok(error instanceof MeerkatError);
      assert.equal(error.code, "-32601");
      assert.equal(error.message, "Method not found: session/recall_input");
      return true;
    });
    assert.deepEqual(calls, ["session/recall_input"]);
  });
});

// The selector of `session/recall_input` and `session/input_status` is
// one-of: it names the input by its input id or by its idempotency key. A
// selector object that names both, or neither, is refused before anything is
// sent. A key that is `undefined` or `null` is not named, as `None` is not
// in the Python wrappers.
describe("the input selector names exactly one of input id and idempotency key", () => {
  const recall = { outcome: "abandoned", input_id: INPUT_ID, attempt_count: 0 };
  const state = { input_id: INPUT_ID, phase: "queued" };
  const canned = {
    "session/recall_input": { session_id: "s-1", recall },
    "session/input_status": { state },
  };
  const wrappers = [
    {
      name: "Session.recallInput",
      needs: "recallInput",
      method: "session/recall_input",
      call: (client, selector) => sessionOver(client).recallInput(selector),
    },
    {
      name: "DeferredSession.recallInput",
      needs: "recallInput",
      method: "session/recall_input",
      call: (client, selector) => new DeferredSession(client, "s-1").recallInput(selector),
    },
    {
      name: "MeerkatClient.inputState",
      needs: "inputState",
      method: "session/input_status",
      call: (client, selector) => client.inputState("s-1", selector),
    },
  ];
  const refused = [
    ["both", { inputId: INPUT_ID, idempotencyKey: "inbox:msg-1" }],
    ["both, the key empty", { inputId: INPUT_ID, idempotencyKey: "" }],
    ["neither", {}],
    ["neither, both undefined", { inputId: undefined, idempotencyKey: undefined }],
    ["neither, both null", { inputId: null, idempotencyKey: null }],
  ];
  const sent = [
    ["the input id", { inputId: INPUT_ID }, { by: "input_id", value: INPUT_ID }],
    [
      "the idempotency key",
      { idempotencyKey: "inbox:msg-1" },
      { by: "idempotency_key", value: "inbox:msg-1" },
    ],
    [
      "the input id beside an undefined key",
      { inputId: INPUT_ID, idempotencyKey: undefined },
      { by: "input_id", value: INPUT_ID },
    ],
    [
      "the idempotency key beside an undefined input id",
      { inputId: undefined, idempotencyKey: "inbox:msg-1" },
      { by: "idempotency_key", value: "inbox:msg-1" },
    ],
    [
      "the idempotency key beside a null input id",
      { inputId: null, idempotencyKey: "inbox:msg-1" },
      { by: "idempotency_key", value: "inbox:msg-1" },
    ],
  ];

  for (const { name, needs, method, call } of wrappers) {
    for (const [label, selector] of refused) {
      it(`${name} refuses a selector that names ${label} and sends nothing`, async () => {
        const { client, calls } = fakeClient(canned);
        await assert.rejects(call(client, selector), (error) => {
          assert.ok(error instanceof MeerkatError);
          assert.equal(error.code, "INVALID_ARGS");
          assert.equal(
            error.message,
            `${needs} needs exactly one of selector.inputId and selector.idempotencyKey`,
          );
          return true;
        });
        assert.deepEqual(calls, []);
      });
    }

    for (const [label, selector, wire] of sent) {
      it(`${name} sends the one selector for ${label}`, async () => {
        const { client, calls } = fakeClient(canned);
        await call(client, selector);
        assert.deepEqual(calls, [{ method, params: { session_id: "s-1", selector: wire } }]);
      });
    }
  }

  it("MeerkatClient.inputState answers the stored state, or null for no match", async () => {
    const found = fakeClient(canned);
    assert.deepEqual(await found.client.inputState("s-1", { inputId: INPUT_ID }), state);
    const missing = fakeClient({ "session/input_status": {} });
    assert.equal(await missing.client.inputState("s-1", { idempotencyKey: "inbox:msg-1" }), null);
  });
});
