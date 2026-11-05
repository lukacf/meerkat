/**
 * Durable decision wrappers (`decision/*`) send the exact RPC literals with
 * snake_case params and pass the server's answer through.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";

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
});
