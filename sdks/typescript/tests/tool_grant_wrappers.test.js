/**
 * Scoped tool grant wrappers (`tool_grants/*`) send the exact RPC literals
 * with snake_case params, and `grantGatedTools` reaches `session/create` only
 * on a server whose contract takes it.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { CapabilityUnavailableError } from "../dist/index.js";

const SESSION_ID = "019a2b3c-0000-7000-8000-000000000001";
const GRANT_ID = "0b6f9f5e-8f3d-4a51-9a52-3c1f6d1e2a10";
const SUBJECT = { kind: "session", session_id: SESSION_ID };

const GRANT = {
  grant_id: GRANT_ID,
  subject: SUBJECT,
  target: { kind: "family", family: "shell", names: ["shell"] },
  issuer: { kind: "operator" },
  status: "active",
  issued_at_ms: 1000,
  expires_at_ms: 61000,
};

const RUN_RESULT = {
  session_id: SESSION_ID,
  text: "ok",
  turns: 1,
  tool_calls: 0,
  usage: { input_tokens: 1, output_tokens: 1 },
};

function fakeClient(results, contractVersion = "0.9.9") {
  const client = new MeerkatClient();
  client._serverContractVersion = contractVersion;
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

describe("scoped tool grant wrappers", () => {
  it("grantTool sends a family target and its duration", async () => {
    const { client, calls } = fakeClient({ "tool_grants/grant": { grant: GRANT } });

    const result = await client.grantTool(
      SESSION_ID,
      { family: "shell" },
      { durationMs: 60000 },
      { reason: "incident" },
    );

    assert.deepEqual(calls, [
      {
        method: "tool_grants/grant",
        params: {
          subject: SUBJECT,
          target: { kind: "family", family: "shell" },
          duration_ms: 60000,
          reason: "incident",
        },
      },
    ]);
    assert.deepEqual(result, { grant: GRANT });
  });

  it("grantTool sends a tool target with an absolute deadline", async () => {
    const { client, calls } = fakeClient({ "tool_grants/grant": { grant: GRANT } });

    await client.grantTool(SESSION_ID, { tool: "deploy" }, { expiresAtMs: 61000 });

    assert.deepEqual(calls, [
      {
        method: "tool_grants/grant",
        params: {
          subject: SUBJECT,
          target: { kind: "tool", name: "deploy" },
          expires_at_ms: 61000,
        },
      },
    ]);
  });

  it("grantTool refuses both or neither target and window forms", async () => {
    const { client, calls } = fakeClient({ "tool_grants/grant": { grant: GRANT } });

    for (const [target, window] of [
      [{ tool: "deploy", family: "shell" }, { durationMs: 60000 }],
      [{}, { durationMs: 60000 }],
      [{ family: "shell" }, { durationMs: 60000, expiresAtMs: 61000 }],
      [{ family: "shell" }, {}],
    ]) {
      await assert.rejects(
        client.grantTool(SESSION_ID, target, window),
        (error) => error.code === "INVALID_ARGS",
        JSON.stringify({ target, window }),
      );
    }
    assert.deepEqual(calls, []);
  });

  it("revokeToolGrant, listToolGrants and toolGrantAudit send their params", async () => {
    const { client, calls } = fakeClient({
      "tool_grants/revoke": { grant: GRANT, changed: true },
      "tool_grants/list": { grants: [GRANT] },
      "tool_grants/audit": { rows: [], next_sequence: 4 },
    });

    const revoked = await client.revokeToolGrant(GRANT_ID, { reason: "done" });
    const listed = await client.listToolGrants(SESSION_ID, { includeEnded: true });
    const page = await client.toolGrantAudit(SESSION_ID, { afterSequence: 2, limit: 10 });

    assert.deepEqual(calls, [
      { method: "tool_grants/revoke", params: { grant_id: GRANT_ID, reason: "done" } },
      { method: "tool_grants/list", params: { subject: SUBJECT, include_ended: true } },
      {
        method: "tool_grants/audit",
        params: { subject: SUBJECT, after_sequence: 2, limit: 10 },
      },
    ]);
    assert.equal(revoked.changed, true);
    assert.deepEqual(listed.grants, [GRANT]);
    assert.equal(page.next_sequence, 4);
  });

  it("createSession sends its grant gate", async () => {
    const { client, calls } = fakeClient({ "session/create": RUN_RESULT });

    await client.createSession("hello", {
      grantGatedTools: { families: ["shell"], tools: ["deploy"] },
    });

    assert.equal(calls[0].method, "session/create");
    assert.deepEqual(calls[0].params.grant_gated_tools, {
      families: ["shell"],
      tools: ["deploy"],
    });
  });

  it("a grant gate is refused before an older server could drop it", async () => {
    const { client, calls } = fakeClient({ "session/create": RUN_RESULT }, "0.9.7");
    const gated = { grantGatedTools: { families: ["shell"] } };

    await assert.rejects(
      client.createSession("hello", gated),
      /grantGatedTools needs a server contract >= 0\.9\.9/,
    );
    await assert.rejects(client.createDeferredSession("hello", gated), /grantGatedTools/);
    assert.deepEqual(calls, []);

    // A create that names no gate is unaffected.
    await client.createSession("hello");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].params.grant_gated_tools, undefined);
  });

  // Released contract 0.9.8 has no grant gate: its session/create ignores
  // `grant_gated_tools` and builds the session with the gated tools ungated.
  const refusedByReleased098 = (error) =>
    error instanceof CapabilityUnavailableError
    && error.code === "CAPABILITY_UNAVAILABLE"
    && /grantGatedTools needs a server contract >= 0\.9\.9; this server reports 0\.9\.8/
      .test(error.message);

  it("a grant gate is refused before a released 0.9.8 server could drop it", async () => {
    const { client, calls } = fakeClient({ "session/create": RUN_RESULT }, "0.9.8");
    const gated = { grantGatedTools: { families: ["shell"] } };

    await assert.rejects(client.createSession("hello", gated), refusedByReleased098);
    await assert.rejects(client.createDeferredSession("hello", gated), refusedByReleased098);
    assert.deepEqual(calls, []);
  });

  it("a streaming create refuses a grant gate before writing to a released 0.9.8 server", () => {
    const { client } = fakeClient({}, "0.9.8");
    const written = [];
    client.process = { stdin: { write: (line) => written.push(line) } };

    assert.throws(
      () => client.createSessionStreaming("hello", { grantGatedTools: { families: ["shell"] } }),
      refusedByReleased098,
    );
    assert.deepEqual(written, []);
  });

  it("a streaming create refuses a grant gate before writing to an older server", () => {
    const { client } = fakeClient({}, "0.9.7");
    const written = [];
    client.process = { stdin: { write: (line) => written.push(line) } };

    assert.throws(
      () => client.createSessionStreaming("hello", { grantGatedTools: { families: ["shell"] } }),
      /grantGatedTools needs a server contract >= 0\.9\.9/,
    );
    assert.deepEqual(written, []);

    // A streaming create that names no gate is unaffected.
    client.createSessionStreaming("hello");
    assert.equal(written.length, 1);
    assert.equal(JSON.parse(written[0]).params.grant_gated_tools, undefined);
  });
});
