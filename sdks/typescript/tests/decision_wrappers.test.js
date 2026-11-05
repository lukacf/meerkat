/**
 * Durable decision wrappers (`decision/*`) send the exact RPC literals with
 * snake_case params and pass the server's answer through. A server too old
 * to serve a method answers method not found, raised naming the method.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { MeerkatError, meerkatErrorFromJsonRpcCode } from "../dist/index.js";

const DECISION_ID = `decision:${"fe421f09".repeat(8)}`;
const SUBJECT_DIGEST = `sha256:${"55c20681".repeat(8)}`;
const REQUESTER = 'principal:["personal_agent","example.org","builder"]';

const DECISION = {
  decision_id: DECISION_ID,
  kind: "release",
  subject_key: "build-7",
  operation_id: "release:build-7",
  request_digest: `sha256:${"7c02a5ba".repeat(8)}`,
  requester: REQUESTER,
  approver_id: "team:owners",
  phase: "open",
  stage: "human",
  notice: "not_due",
  deadline_at_ms: 1761739200000,
  created_at_ms: 1761735600000,
  subject_digest: SUBJECT_DIGEST,
  renderer: "render.v1",
  permit_issued: false,
  revision: 1,
};

const PRESENTATION = {
  audience: { kind: "approver", approver_id: "team:owners" },
  subject_digest: SUBJECT_DIGEST,
  parts_shown: [{ index: 0, count: 1, part_digest: SUBJECT_DIGEST }],
  channel_ref: "chat:owners",
  message_ref: "message-1",
  at_ms: 2000,
};

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

describe("durable decision wrappers", () => {
  it("listDecisions sends only the filters it was given", async () => {
    const listing = { decisions: [DECISION], unreadable: [], next_after: DECISION_ID };
    const { client, calls } = fakeClient({ "decision/list": listing });

    assert.deepEqual(await client.listDecisions(), listing);
    const result = await client.listDecisions({
      kind: "release",
      phase: "open",
      approverId: "team:owners",
      requester: REQUESTER,
      after: DECISION_ID,
      limit: 50,
    });

    assert.deepEqual(calls, [
      { method: "decision/list", params: {} },
      {
        method: "decision/list",
        params: {
          kind: "release",
          phase: "open",
          approver_id: "team:owners",
          requester: REQUESTER,
          after: DECISION_ID,
          limit: 50,
        },
      },
    ]);
    assert.deepEqual(result, listing);
  });

  it("getDecision asks for the request only when told", async () => {
    const { client, calls } = fakeClient({ "decision/get": { decision: DECISION } });

    assert.deepEqual(await client.getDecision(DECISION_ID), { decision: DECISION });
    await client.getDecision(DECISION_ID, { includeRequest: true });

    assert.deepEqual(calls, [
      { method: "decision/get", params: { decision_id: DECISION_ID } },
      {
        method: "decision/get",
        params: { decision_id: DECISION_ID, include_request: true },
      },
    ]);
  });

  it("decisionRendering sends its part size", async () => {
    const rendering = {
      decision_id: DECISION_ID,
      rendering: "Release build 7 to production.",
      renderer: "render.v1",
      subject_digest: SUBJECT_DIGEST,
      max_part_bytes: 16,
      parts: [{ index: 0, count: 2, part_digest: SUBJECT_DIGEST }],
    };
    const { client, calls } = fakeClient({ "decision/rendering": rendering });

    await client.decisionRendering(DECISION_ID);
    const result = await client.decisionRendering(DECISION_ID, { maxPartBytes: 16 });

    assert.deepEqual(calls, [
      { method: "decision/rendering", params: { decision_id: DECISION_ID } },
      {
        method: "decision/rendering",
        params: { decision_id: DECISION_ID, max_part_bytes: 16 },
      },
    ]);
    assert.deepEqual(result, rendering);
  });

  it("presentDecision sends the presentation unchanged", async () => {
    const presented = { ...DECISION, first_presented_to_approver_at_ms: 2000 };
    const { client, calls } = fakeClient({ "decision/present": { decision: presented } });

    const result = await client.presentDecision(DECISION_ID, PRESENTATION);

    assert.deepEqual(calls, [
      {
        method: "decision/present",
        params: { decision_id: DECISION_ID, presentation: PRESENTATION },
      },
    ]);
    assert.deepEqual(result, { decision: presented });
  });

  it("withdrawDecision names only the decision", async () => {
    const withdrawn = { ...DECISION, phase: "withdrawn" };
    const { client, calls } = fakeClient({ "decision/withdraw": { decision: withdrawn } });

    const result = await client.withdrawDecision(DECISION_ID);

    assert.deepEqual(calls, [
      { method: "decision/withdraw", params: { decision_id: DECISION_ID } },
    ]);
    assert.deepEqual(result, { decision: withdrawn });
  });

  it("keeps an unknown phase as the server wrote it", async () => {
    // The phase vocabulary is tolerant: a phase a later server sends reaches
    // the caller as the server wrote it, neither refused nor rewritten.
    const archived = { ...DECISION, phase: "archived" };
    const { client } = fakeClient({
      "decision/list": { decisions: [archived], unreadable: [] },
      "decision/get": { decision: archived },
    });

    const listing = await client.listDecisions();
    const read = await client.getDecision(DECISION_ID);

    assert.deepEqual(listing, { decisions: [archived], unreadable: [] });
    assert.deepEqual(read, { decision: archived });
    assert.equal(read.decision.phase, "archived");
  });

  const DECISION_CALLS = [
    ["decision/list", (client) => client.listDecisions()],
    ["decision/get", (client) => client.getDecision(DECISION_ID)],
    ["decision/rendering", (client) => client.decisionRendering(DECISION_ID)],
    ["decision/present", (client) => client.presentDecision(DECISION_ID, PRESENTATION)],
    ["decision/withdraw", (client) => client.withdrawDecision(DECISION_ID)],
  ];

  function answering(error) {
    const client = new MeerkatClient();
    client.request = async () => {
      throw error;
    };
    return client;
  }

  it("names the method a server without it answers method not found for", async () => {
    for (const [method, call] of DECISION_CALLS) {
      // The server answers method not found in its own words, without the
      // method's name: the SDK names the method and the server it needs.
      const client = answering(meerkatErrorFromJsonRpcCode(-32601, undefined, "Method not found"));
      await assert.rejects(call(client), (error) => {
        assert.ok(error instanceof MeerkatError);
        assert.equal(error.code, "-32601");
        assert.equal(
          error.message,
          `${method} needs a Meerkat 0.9.11 or newer server: Method not found`,
        );
        return true;
      });
    }
  });

  it("passes every other error to the caller unchanged", async () => {
    for (const [, call] of DECISION_CALLS) {
      const unavailable = meerkatErrorFromJsonRpcCode(
        -32020,
        "CAPABILITY_UNAVAILABLE",
        "no durable decisions",
      );
      const client = answering(unavailable);
      await assert.rejects(call(client), (error) => {
        assert.equal(error, unavailable);
        return true;
      });
    }
  });
});
