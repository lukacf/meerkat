/**
 * Executed contract for `meta/identity/warm` and the `tools_source` of
 * `meta/identity/resolved_tools`.
 *
 * The test drives the real methods against a scripted `_rpc` and asserts on
 * values: the wire method and params the gateway dispatches on, and the parse
 * of every answer shape, an unknown `tools_source` included.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { Meta } from "../src/index.js";
import { MetaRuntime } from "../src/runtime.js";
import { parseIdentityResolvedToolsResult } from "../src/types.js";
import { runtimeConfig } from "./runtime-config.js";

function initParams(builder: unknown): Record<string, any> {
  const config = (builder as { _config: unknown })._config;
  const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
  return (rt as any)._buildInitParams();
}

function makeRuntime(answer: unknown) {
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const rt = new MetaRuntime(runtimeConfig());
  (rt as unknown as Record<string, unknown>)._rpc = async (
    method: string,
    params?: Record<string, unknown>,
  ) => {
    calls.push({ method, params: params ?? {} });
    return answer;
  };
  return { rt, calls };
}

describe("identity warm", () => {
  it("sends meta/identity/warm and reads the written tool record", async () => {
    const { rt, calls } = makeRuntime({
      identity: "domain:security",
      session_id: "sid-1",
      tools: ["fetch", "shell"],
      record_revision: 2,
      published_at_ms: 1700000000000,
    });
    const warmed = await rt.mobHandle().identityWarm("domain:security");

    assert.equal(calls[0].method, "meta/identity/warm");
    assert.deepEqual(calls[0].params, { identity: "domain:security" });
    assert.equal(warmed.sessionId, "sid-1");
    assert.deepEqual(warmed.tools, ["fetch", "shell"]);
    assert.equal(warmed.recordRevision, 2);
    assert.equal(warmed.publishedAtMs, 1700000000000);
  });
});

describe("identity resolved tools sources", () => {
  it("reads a gateway without tools_source as a live answer", () => {
    const result = parseIdentityResolvedToolsResult({
      identity: "domain:security",
      session_id: "sid-1",
      tools: ["shell"],
    });
    assert.equal(result.toolsSource, "live");
    assert.equal(result.recordRevision, null);
    assert.equal(result.publishedAtMs, null);
  });

  it("reads a recorded answer with its record's revision", () => {
    const result = parseIdentityResolvedToolsResult({
      identity: "domain:security",
      session_id: "sid-1",
      tools: ["shell"],
      tools_source: "recorded",
      record_revision: 3,
      published_at_ms: 1700000000000,
    });
    assert.equal(result.toolsSource, "recorded");
    assert.equal(result.recordRevision, 3);
    assert.equal(result.publishedAtMs, 1700000000000);
  });

  it("keeps the session of a member with no build recorded yet", () => {
    const result = parseIdentityResolvedToolsResult({
      identity: "domain:security",
      session_id: "sid-1",
      tools: [],
      tools_source: "no_build_recorded",
    });
    assert.equal(result.toolsSource, "no_build_recorded");
    assert.equal(result.sessionId, "sid-1");
    assert.deepEqual(result.tools, []);
  });

  it("keeps an unknown tools_source as its raw string", () => {
    const result = parseIdentityResolvedToolsResult({
      identity: "domain:security",
      session_id: "sid-1",
      tools: ["shell"],
      tools_source: "a_later_source",
    });
    assert.equal(result.toolsSource, "a_later_source");
    assert.deepEqual(result.tools, ["shell"]);
  });
});

describe("member restore option", () => {
  it("is omitted by default", () => {
    const params = initParams(Meta.builder());
    assert.equal(params.runtime_options?.member_restore, undefined);
  });

  it("is emitted as runtime_options.member_restore", () => {
    for (const restore of ["on_demand", "on_demand_verified", "eager"] as const) {
      const params = initParams(Meta.builder().memberRestore(restore));
      assert.equal(params.runtime_options.member_restore, restore);
    }
  });
});
