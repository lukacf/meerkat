/**
 * Tests for all models from src/models.ts.
 *
 * Covers:
 *   - discoverySpecToDict: full and minimal fields
 *   - preSpawnDataToDict: full and empty
 *   - sessionQueryToDict: defaults (includeDeleted=false, limit=100)
 *   - SessionBuildOptions: addTools, registerTool, tools getter, toolHandlers getter,
 *     toDict, validation errors
 */
import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  discoverySpecToDict,
  preSpawnDataToDict,
  sessionQueryToDict,
  SessionBuildOptions,
} from "../dist/index.js";

// ---------------------------------------------------------------------------
// discoverySpecToDict
// ---------------------------------------------------------------------------

describe("discoverySpecToDict", () => {
  it("converts all fields to snake_case wire format", () => {
    const result = discoverySpecToDict({
      role: "assistant",
      agentIdentity: "mk-1",
      labels: { role: "lead", tier: "gold" },
      appContext: { theme: "dark" },
      additionalInstructions: ["Be concise", "Use formal tone"],
      resumeSessionId: "sess-old",
    });
    assert.equal(result.role, "assistant");
    assert.equal(result.agent_identity, "mk-1");
    assert.deepEqual(result.labels, { role: "lead", tier: "gold" });
    assert.deepEqual(result.app_context, { theme: "dark" });
    assert.deepEqual(result.additional_instructions, [
      "Be concise",
      "Use formal tone",
    ]);
    assert.equal(result.resume_session_id, "sess-old");
  });

  it("with minimal fields (required only)", () => {
    const result = discoverySpecToDict({
      role: "worker",
      agentIdentity: "mk-2",
    });
    assert.equal(result.role, "worker");
    assert.equal(result.agent_identity, "mk-2");
    // Optional fields should be absent, not null/undefined values
    assert.equal(result.labels, undefined);
    assert.equal(result.app_context, undefined);
    assert.equal(result.additional_instructions, undefined);
    assert.equal(result.resume_session_id, undefined);
  });

  it("omits empty labels object", () => {
    const result = discoverySpecToDict({
      role: "p",
      agentIdentity: "m",
      labels: {},
    });
    assert.equal(result.labels, undefined);
  });

  it("omits empty additionalInstructions array", () => {
    const result = discoverySpecToDict({
      role: "p",
      agentIdentity: "m",
      additionalInstructions: [],
    });
    assert.equal(result.additional_instructions, undefined);
  });

  it("copies labels (no shared reference)", () => {
    const labels = { a: "1" };
    const result = discoverySpecToDict({
      role: "p",
      agentIdentity: "m",
      labels,
    });
    assert.deepEqual(result.labels, { a: "1" });
    assert.notEqual(result.labels, labels);
  });

  it("copies additionalInstructions (no shared reference)", () => {
    const instructions = ["x"];
    const result = discoverySpecToDict({
      role: "p",
      agentIdentity: "m",
      additionalInstructions: instructions,
    });
    assert.deepEqual(result.additional_instructions, ["x"]);
    assert.notEqual(result.additional_instructions, instructions);
  });
});

// ---------------------------------------------------------------------------
// preSpawnDataToDict
// ---------------------------------------------------------------------------

describe("preSpawnDataToDict", () => {
  it("converts all fields to wire format", () => {
    const result = preSpawnDataToDict({
      resumeMap: { "agent-1": "sess-1", "agent-2": "sess-2" },
      moduleId: "mod-5",
      env: { API_KEY: "secret", MODE: "prod" },
    });
    assert.deepEqual(result.resume_map, {
      "agent-1": "sess-1",
      "agent-2": "sess-2",
    });
    assert.equal(result.module_id, "mod-5");
    // env is converted to entries (array of [key, value] pairs)
    assert.ok(Array.isArray(result.env));
    const envEntries = result.env as [string, string][];
    assert.equal(envEntries.length, 2);
    // Check that the entries contain the expected key-value pairs
    const envMap = Object.fromEntries(envEntries);
    assert.equal(envMap.API_KEY, "secret");
    assert.equal(envMap.MODE, "prod");
  });

  it("with empty/no fields", () => {
    const result = preSpawnDataToDict({});
    assert.deepEqual(result, {});
  });

  it("omits empty resumeMap", () => {
    const result = preSpawnDataToDict({ resumeMap: {} });
    assert.equal(result.resume_map, undefined);
  });

  it("omits empty env", () => {
    const result = preSpawnDataToDict({ env: {} });
    assert.equal(result.env, undefined);
  });

  it("copies resumeMap (no shared reference)", () => {
    const resumeMap = { a: "1" };
    const result = preSpawnDataToDict({ resumeMap });
    assert.deepEqual(result.resume_map, { a: "1" });
    assert.notEqual(result.resume_map, resumeMap);
  });
});

// ---------------------------------------------------------------------------
// sessionQueryToDict
// ---------------------------------------------------------------------------

describe("sessionQueryToDict", () => {
  it("converts all fields to snake_case", () => {
    const result = sessionQueryToDict({
      agentType: "assistant",
      ownerId: "user-1",
      labels: { env: "prod" },
      includeDeleted: true,
      limit: 50,
    });
    assert.equal(result.agent_type, "assistant");
    assert.equal(result.owner_id, "user-1");
    assert.deepEqual(result.labels, { env: "prod" });
    assert.equal(result.include_deleted, true);
    assert.equal(result.limit, 50);
  });

  it("defaults includeDeleted to false and limit to 100", () => {
    const result = sessionQueryToDict({});
    assert.equal(result.include_deleted, false);
    assert.equal(result.limit, 100);
    // Other fields should be absent
    assert.equal(result.agent_type, undefined);
    assert.equal(result.owner_id, undefined);
    assert.equal(result.labels, undefined);
  });

  it("omits empty labels object", () => {
    const result = sessionQueryToDict({ labels: {} });
    assert.equal(result.labels, undefined);
  });

  it("copies labels (no shared reference)", () => {
    const labels = { x: "y" };
    const result = sessionQueryToDict({ labels });
    assert.deepEqual(result.labels, { x: "y" });
    assert.notEqual(result.labels, labels);
  });

  it("respects explicit includeDeleted=false", () => {
    const result = sessionQueryToDict({ includeDeleted: false });
    assert.equal(result.include_deleted, false);
  });

  it("respects explicit limit=0", () => {
    const result = sessionQueryToDict({ limit: 0 });
    assert.equal(result.limit, 0);
  });
});

// ---------------------------------------------------------------------------
// SessionBuildOptions
// ---------------------------------------------------------------------------

describe("SessionBuildOptions", () => {
  it("starts with sensible defaults", () => {
    const opts = new SessionBuildOptions();
    assert.equal(opts.appContext, undefined);
    assert.deepEqual(opts.additionalInstructions, []);
    assert.equal(opts.sessionId, null);
    assert.deepEqual(opts.labels, {});
    assert.equal(opts.profileName, null);
    assert.deepEqual(opts.tools, []);
    assert.equal(opts.toolHandlers.size, 0);
  });

  describe("addTools", () => {
    it("adds tool names", () => {
      const opts = new SessionBuildOptions();
      opts.addTools(["search", "calculator"]);
      assert.deepEqual(opts.tools, ["search", "calculator"]);
    });

    it("accumulates across multiple calls", () => {
      const opts = new SessionBuildOptions();
      opts.addTools(["a"]);
      opts.addTools(["b", "c"]);
      assert.deepEqual(opts.tools, ["a", "b", "c"]);
    });

    it("throws TypeError for non-string tool", () => {
      const opts = new SessionBuildOptions();
      assert.throws(
        () => opts.addTools([42 as unknown as string]),
        (err: unknown) =>
          err instanceof TypeError && /tools must be strings/.test(err.message),
      );
    });
  });

  describe("registerTool", () => {
    it("adds tool name and handler", () => {
      const opts = new SessionBuildOptions();
      const handler = () => "result";
      opts.registerTool("myTool", handler);
      assert.deepEqual(opts.tools, ["myTool"]);
      assert.equal(opts.toolHandlers.size, 1);
      assert.equal(opts.toolHandlers.get("myTool"), handler);
    });

    it("throws TypeError for non-string name", () => {
      const opts = new SessionBuildOptions();
      assert.throws(
        () =>
          opts.registerTool(
            123 as unknown as string,
            () => {},
          ),
        (err: unknown) =>
          err instanceof TypeError &&
          /tool name must be a string/.test(err.message),
      );
    });

    it("throws TypeError for non-function handler", () => {
      const opts = new SessionBuildOptions();
      assert.throws(
        () =>
          opts.registerTool(
            "tool",
            "not-a-function" as unknown as () => unknown,
          ),
        (err: unknown) =>
          err instanceof TypeError &&
          /handler must be callable/.test(err.message),
      );
    });

    it("carries description + inputSchema onto the build_agent wire", () => {
      const schema = {
        type: "object",
        properties: { city: { type: "string" } },
        required: ["city"],
      };
      const opts = new SessionBuildOptions();
      opts.registerTool("plain", () => "ok");
      opts.registerTool("weather", () => "ok", {
        description: "Look up the weather",
        inputSchema: schema,
      });
      assert.deepEqual(opts.toDict().tools, [
        "plain",
        {
          name: "weather",
          description: "Look up the weather",
          input_schema: schema,
        },
      ]);
    });
  });

  describe("tools getter", () => {
    it("returns a copy (no shared reference)", () => {
      const opts = new SessionBuildOptions();
      opts.addTools(["a"]);
      const tools1 = opts.tools;
      const tools2 = opts.tools;
      assert.deepEqual(tools1, tools2);
      assert.notEqual(tools1, tools2);
    });
  });

  describe("toolHandlers getter", () => {
    it("returns a copy (no shared reference)", () => {
      const opts = new SessionBuildOptions();
      const handler = () => "r";
      opts.registerTool("t", handler);
      const handlers1 = opts.toolHandlers;
      const handlers2 = opts.toolHandlers;
      assert.equal(handlers1.size, handlers2.size);
      assert.notEqual(handlers1, handlers2);
    });
  });

  describe("toDict", () => {
    it("converts full options to snake_case", () => {
      const opts = new SessionBuildOptions();
      opts.appContext = { theme: "dark" };
      opts.additionalInstructions = ["Be concise"];
      opts.sessionId = "sess-1";
      opts.labels = { env: "prod" };
      opts.profileName = "assistant";
      opts.addTools(["search"]);
      opts.registerTool("calc", () => 42);

      const dict = opts.toDict();
      assert.deepEqual(dict.app_context, { theme: "dark" });
      assert.deepEqual(dict.additional_instructions, ["Be concise"]);
      assert.equal(dict.session_id, "sess-1");
      assert.deepEqual(dict.labels, { env: "prod" });
      assert.equal(dict.profile_name, "assistant");
      assert.deepEqual(dict.tools, ["search", "calc"]);
    });

    it("omits unset optional fields", () => {
      const opts = new SessionBuildOptions();
      const dict = opts.toDict();
      assert.deepEqual(dict, {});
    });

    it("omits empty additionalInstructions", () => {
      const opts = new SessionBuildOptions();
      opts.additionalInstructions = [];
      const dict = opts.toDict();
      assert.equal(dict.additional_instructions, undefined);
    });

    it("omits empty labels", () => {
      const opts = new SessionBuildOptions();
      opts.labels = {};
      const dict = opts.toDict();
      assert.equal(dict.labels, undefined);
    });

    it("omits empty tools", () => {
      const opts = new SessionBuildOptions();
      const dict = opts.toDict();
      assert.equal(dict.tools, undefined);
    });

    it("copies arrays and objects (no shared references)", () => {
      const opts = new SessionBuildOptions();
      opts.additionalInstructions = ["x"];
      opts.labels = { a: "1" };
      opts.addTools(["t"]);
      const dict = opts.toDict();
      assert.notEqual(dict.additional_instructions, opts.additionalInstructions);
      assert.notEqual(dict.labels, opts.labels);
      assert.notEqual(dict.tools, opts.tools);
    });
  });

  describe("mutable public fields", () => {
    it("allows setting appContext", () => {
      const opts = new SessionBuildOptions();
      opts.appContext = { key: "value" };
      assert.deepEqual(opts.appContext, { key: "value" });
    });

    it("allows setting additionalInstructions", () => {
      const opts = new SessionBuildOptions();
      opts.additionalInstructions = ["instruction1"];
      assert.deepEqual(opts.additionalInstructions, ["instruction1"]);
    });

    it("allows setting sessionId", () => {
      const opts = new SessionBuildOptions();
      opts.sessionId = "my-session";
      assert.equal(opts.sessionId, "my-session");
    });

    it("allows setting labels", () => {
      const opts = new SessionBuildOptions();
      opts.labels = { env: "test" };
      assert.deepEqual(opts.labels, { env: "test" });
    });

    it("allows setting profileName", () => {
      const opts = new SessionBuildOptions();
      opts.profileName = "worker";
      assert.equal(opts.profileName, "worker");
    });
  });
});
