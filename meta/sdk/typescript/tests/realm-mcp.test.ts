/**
 * `MetaRuntime` wrappers of the stdio gateway's realm MCP administration
 * (`meta/realm_mcp/{servers, install, set_secret, remove_secret}`): each
 * sends its method with exactly the params it names; every typed outcome is
 * its own error (`realm_mcp_refused` with a closed vocabulary of kinds,
 * `realm_mcp_unavailable`, `realm_mcp_outcome_unknown`); and a secret value
 * is sent once, in the `set_secret` request, and kept out of every error.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  REALM_MCP_REFUSAL_KINDS,
  RealmMcpOutcomeUnknownError,
  RealmMcpRefusedError,
  RealmMcpUnavailableError,
  RpcError,
} from "../src/errors.js";
import { runtimeConfig } from "./runtime-config.js";

type Reply = { result: unknown } | { error: Record<string, unknown> };

const SERVER = {
  name: "search",
  command: "sh",
  args: ["-c", "sleep 30"],
  secret_env: ["SEARCH_API_KEY"],
};
const SECRET = "realm-mcp-secret-7f3a9c41";

async function scriptedRuntime(replies: Reply[]) {
  const { MetaRuntime } = await import("../src/runtime.js");
  const rt = new MetaRuntime(runtimeConfig());
  const queue = [...replies];
  const sent: Array<{ method: string; params: unknown }> = [];
  (rt as unknown as Record<string, unknown>)._running = true;
  (rt as unknown as Record<string, unknown>)._transport = {
    sendAsync: async (request: Record<string, unknown>) => {
      await Promise.resolve();
      const method = String(request.method);
      sent.push({ method, params: request.params });
      const reply = queue.shift();
      if (reply === undefined) throw new Error(`unscripted ${method}`);
      return { jsonrpc: "2.0", id: request.id, ...reply };
    },
  };
  return { rt, sent };
}

describe("MetaRuntime realm MCP wrappers", () => {
  it("send each method with exactly its params", async () => {
    const ok = { result: {} };
    const { rt, sent } = await scriptedRuntime([ok, ok, ok, ok, ok]);
    await rt.realmMcpServers();
    await rt.realmMcpInstall(SERVER, { expectedGeneration: 3 });
    await rt.realmMcpSetSecret(SERVER, "SEARCH_API_KEY", SECRET);
    await rt.realmMcpRemoveSecret(SERVER, "SEARCH_API_KEY");
    await rt.realmMcpRemoveSecret(SERVER, "SEARCH_API_KEY", { removal: "obsolete" });
    assert.deepEqual(sent, [
      { method: "meta/realm_mcp/servers", params: {} },
      { method: "meta/realm_mcp/install", params: { server: SERVER, expected_generation: 3 } },
      {
        method: "meta/realm_mcp/set_secret",
        params: { server: SERVER, env: "SEARCH_API_KEY", value: SECRET },
      },
      {
        method: "meta/realm_mcp/remove_secret",
        params: { server: SERVER, env: "SEARCH_API_KEY", removal: "current" },
      },
      {
        method: "meta/realm_mcp/remove_secret",
        params: { server: SERVER, env: "SEARCH_API_KEY", removal: "obsolete" },
      },
    ]);
  });

  it("refuses a removal other than current or obsolete", async () => {
    const { rt, sent } = await scriptedRuntime([]);
    await assert.rejects(
      rt.realmMcpRemoveSecret(SERVER, "SEARCH_API_KEY", {
        removal: "all" as unknown as "current",
      }),
      TypeError,
    );
    assert.deepEqual(sent, []);
  });

  it("maps each outcome to its own typed error", async () => {
    const { rt } = await scriptedRuntime([
      {
        error: {
          code: -32602,
          message: "the realm's own document moved",
          data: {
            kind: "realm_mcp_refused",
            realm_mcp_refusal: { kind: "generation_conflict", expected: 3, current: 4 },
          },
        },
      },
      {
        error: {
          code: -32004,
          message: "the credential owner cannot be held now",
          data: { kind: "realm_mcp_unavailable" },
        },
      },
      {
        error: {
          code: -32603,
          message: "the write's outcome is unknown",
          data: { kind: "realm_mcp_outcome_unknown" },
        },
      },
      { error: { code: -32602, message: "Invalid params" } },
    ]);
    await assert.rejects(
      rt.realmMcpInstall(SERVER, { expectedGeneration: 3 }),
      (error: unknown) =>
        error instanceof RealmMcpRefusedError &&
        error.refusalKind === "generation_conflict" &&
        (error.refusal as Record<string, unknown>).current === 4,
    );
    await assert.rejects(
      rt.realmMcpSetSecret(SERVER, "SEARCH_API_KEY", SECRET),
      (error: unknown) => error instanceof RealmMcpUnavailableError,
    );
    await assert.rejects(
      rt.realmMcpInstall(SERVER, { expectedGeneration: 3 }),
      (error: unknown) => error instanceof RealmMcpOutcomeUnknownError,
    );
    await assert.rejects(
      rt.realmMcpInstall(SERVER, { expectedGeneration: 3 }),
      (error: unknown) => error instanceof RpcError && !(error instanceof RealmMcpRefusedError),
    );
  });

  it("keeps the refusal kinds a closed vocabulary", () => {
    assert.deepEqual(
      [...REALM_MCP_REFUSAL_KINDS].sort(),
      [
        "definition_changed",
        "generation_conflict",
        "invalid_secret_update",
        "invalid_server",
        "no_document",
        "server_exists",
        "server_not_found",
        "still_current",
      ],
    );
  });

  it("keeps the secret value out of the error a refused set_secret rejects with", async () => {
    const { rt } = await scriptedRuntime([
      {
        error: {
          code: -32602,
          message: "MCP server 'search' is no longer installed as reviewed",
          data: {
            kind: "realm_mcp_refused",
            realm_mcp_refusal: { kind: "definition_changed" },
          },
        },
      },
    ]);
    const error = await rt.realmMcpSetSecret(SERVER, "SEARCH_API_KEY", SECRET).then(
      () => assert.fail("the refusal rejects"),
      (rejection: unknown) => rejection,
    );
    assert.ok(error instanceof RealmMcpRefusedError);
    assert.ok(!String(error).includes(SECRET));
    assert.ok(!JSON.stringify(error).includes(SECRET));
    assert.ok(!(error.stack ?? "").includes(SECRET));
  });
});
