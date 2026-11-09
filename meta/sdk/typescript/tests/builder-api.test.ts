/**
 * TDD tests for the new builder API: persistentState, afterCreate, SessionCreatedContext.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { Meta, MetaBuilder } from "../dist/index.js";
import { CallbackDispatcher } from "../dist/agent-builder.js";
import { SessionBuildOptions } from "../dist/models.js";
import type { SessionCreatedContext } from "../dist/types.js";

// ---------------------------------------------------------------------------
// persistentState on builder
// ---------------------------------------------------------------------------

describe("MetaBuilder.persistentState()", () => {
  it("returns this for chaining", () => {
    const builder = Meta.builder();
    const result = builder.persistentState("/tmp/test-state");
    assert.equal(result, builder);
  });

  it("sets persistentState on config", () => {
    const builder = Meta.builder();
    builder.persistentState("/tmp/test-state");
    assert.equal(builder._config.persistentState, "/tmp/test-state");
  });

  it("defaults to null", () => {
    const builder = Meta.builder();
    assert.equal(builder._config.persistentState, null);
  });
});

describe("MetaBuilder.agentMemory()", () => {
  it("defaults to disabled", () => {
    const builder = Meta.builder();
    assert.equal(builder._config.agentMemoryConfig, null);
  });

  it("stores true for default gateway configuration", () => {
    const builder = Meta.builder();
    const result = builder.agentMemory();

    assert.equal(result, builder);
    assert.equal(builder._config.agentMemoryConfig, true);
  });

  it("serializes camelCase options to gateway wire keys", () => {
    const builder = Meta.builder();
    builder.agentMemory({
      realm: "example",
      selection: "contextual",
      maxEntries: 3,
      recallTimeoutMs: 1200,
      recallFailurePolicy: "fail",
      instructionHeader: "Remember",
    });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      realm: "example",
      selection: "contextual",
      max_entries: 3,
      recall_timeout_ms: 1200,
      recall_failure_policy: "fail",
      instruction_header: "Remember",
    });
  });

  it("serializes taint knobs to gateway wire keys", () => {
    const builder = Meta.builder();
    builder.agentMemory({
      llmWrites: "quarantined",
      recorderTool: false,
      contentTrust: {
        trustedMcpServers: ["knowledge_graph"],
        untrustedTools: ["scrape_page"],
        trustedTools: ["safe_calc"],
      },
    });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      llm_writes: "quarantined",
      recorder_tool: false,
      content_trust: {
        trusted_mcp_servers: ["knowledge_graph"],
        untrusted_tools: ["scrape_page"],
        trusted_tools: ["safe_calc"],
      },
    });
  });

  it("keeps only the retired selector off compatibility form", () => {
    const builder = Meta.builder();
    builder.agentMemory({ selector: "off" });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      selector: "off",
    });
    assert.throws(
      () => builder.agentMemory({ selector: "profile:/tmp/selector.toml" } as never),
      /selector is RETIRED/,
    );
  });

  it("serializes the distiller block to gateway wire keys", () => {
    const builder = Meta.builder();
    builder.agentMemory({
      distiller: {
        enabled: true,
        runsPerHour: 6,
        minInteractions: 5,
        model: "claude-haiku-4-5",
      },
    });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      distiller: {
        enabled: true,
        runs_per_hour: 6,
        min_interactions: 5,
        model: "claude-haiku-4-5",
      },
    });

    const boolBuilder = Meta.builder();
    boolBuilder.agentMemory({ distiller: true });
    assert.deepEqual(boolBuilder._config.agentMemoryConfig, { distiller: true });
  });

  it("serializes the steward block to gateway wire keys", () => {
    const builder = Meta.builder();
    builder.agentMemory({
      steward: {
        enabled: true,
        cadence: "*/6h",
        model: "claude-sonnet-4-6",
        perMob: false,
        runsPerDay: 4,
        minSignals: 3,
      },
    });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      steward: {
        enabled: true,
        cadence: "*/6h",
        model: "claude-sonnet-4-6",
        per_mob: false,
        runs_per_day: 4,
        min_signals: 3,
      },
    });

    const boolBuilder = Meta.builder();
    boolBuilder.agentMemory({ steward: true });
    assert.deepEqual(boolBuilder._config.agentMemoryConfig, { steward: true });
  });

  it("serializes operatorScope to the gateway wire key", () => {
    const builder = Meta.builder();
    builder.agentMemory({ store: "sqlite", operatorScope: "provisional" });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      store: "sqlite",
      operator_scope: "provisional",
    });
  });

  it("keeps only disabled hygienist compatibility forms", () => {
    const builder = Meta.builder();
    builder.agentMemory({
      hygienist: {
        enabled: false,
        runsPerDay: 3,
        model: "legacy-model",
        maxOutputTokens: 8192,
      },
    });

    assert.deepEqual(builder._config.agentMemoryConfig, {
      hygienist: {
        enabled: false,
        runs_per_day: 3,
        model: "legacy-model",
        max_output_tokens: 8192,
      },
    });

    const boolBuilder = Meta.builder();
    boolBuilder.agentMemory({ hygienist: false });
    assert.deepEqual(boolBuilder._config.agentMemoryConfig, { hygienist: false });

    assert.throws(
      () => Meta.builder().agentMemory({ hygienist: true } as never),
      /hygienist is PARKED and cannot be enabled/,
    );
    assert.throws(
      () => Meta.builder().agentMemory({ hygienist: {} } as never),
      /hygienist is PARKED and cannot be enabled/,
    );
  });

  it("rejects unknown options at runtime instead of silently dropping them", () => {
    const builder = Meta.builder();
    assert.throws(
      // Cast simulates a plain-JS caller; the TS type already rejects this.
      () => builder.agentMemory({ perTurnInjecton: "budgeted" } as never),
      /agentMemory got unsupported option\(s\): perTurnInjecton/,
    );
  });

  it("rejects unknown nested options at runtime", () => {
    assert.throws(
      () =>
        Meta.builder().agentMemory({
          distiller: { runsPerHour: 2, runsperhourTypo: 9 },
        } as never),
      /agentMemory distiller got unsupported option\(s\): runsperhourTypo/,
    );
    assert.throws(
      () => Meta.builder().agentMemory({ steward: { cadance: "*/6h" } } as never),
      /agentMemory steward got unsupported option\(s\): cadance/,
    );
    assert.throws(
      () =>
        Meta.builder().agentMemory({
          hygienist: { enabled: false, runsPerDya: 2 },
        } as never),
      /agentMemory hygienist got unsupported option\(s\): runsPerDya/,
    );
    assert.throws(
      () =>
        Meta.builder().agentMemory({
          contentTrust: { trustedMcpServrs: [] },
        } as never),
      /agentMemory contentTrust got unsupported option\(s\): trustedMcpServrs/,
    );
  });
});

// ---------------------------------------------------------------------------
// callback/after_create dispatch
// ---------------------------------------------------------------------------

describe("CallbackDispatcher callback/after_create", () => {
  it("routes to builder.afterCreate()", async () => {
    const received: { sessionId?: string; context?: SessionCreatedContext } = {};

    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(_opts: SessionBuildOptions) {},
      async afterCreate(sessionId: string, context: SessionCreatedContext) {
        received.sessionId = sessionId;
        received.context = context;
      },
    });

    await dispatcher.handleCallback("callback/after_create", {
      session_id: "sid-123",
      model: "claude-sonnet-4-5",
      labels: { agent_type: "lead" },
      system_prompt: "You are a lead.",
    });

    assert.equal(received.sessionId, "sid-123");
    assert.equal(received.context?.model, "claude-sonnet-4-5");
    assert.deepEqual(received.context?.labels, { agent_type: "lead" });
    assert.equal(received.context?.systemPrompt, "You are a lead.");
  });

  it("is a no-op when builder has no afterCreate", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(_opts: SessionBuildOptions) {},
    });

    // Should not throw.
    await dispatcher.handleCallback("callback/after_create", {
      session_id: "sid-456",
      model: "test-model",
      labels: {},
      system_prompt: null,
    });
  });

  it("swallows afterCreate errors (best-effort)", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(_opts: SessionBuildOptions) {},
      async afterCreate(_sessionId: string, _context: SessionCreatedContext) {
        throw new Error("db unavailable");
      },
    });

    // Should not throw.
    await dispatcher.handleCallback("callback/after_create", {
      session_id: "sid-789",
      model: "test-model",
      labels: {},
      system_prompt: null,
    });
  });
});

// ---------------------------------------------------------------------------
// SessionCreatedContext interface
// ---------------------------------------------------------------------------

describe("SessionCreatedContext", () => {
  it("can be constructed from wire format", () => {
    const ctx: SessionCreatedContext = {
      model: "claude-sonnet-4-5",
      labels: { agent_type: "lead" },
      systemPrompt: "You are a lead agent.",
    };
    assert.equal(ctx.model, "claude-sonnet-4-5");
    assert.deepEqual(ctx.labels, { agent_type: "lead" });
    assert.equal(ctx.systemPrompt, "You are a lead agent.");
  });
});

// ---------------------------------------------------------------------------
// fork_source on callback/build_agent (meerkat 0.8.45+)
// ---------------------------------------------------------------------------

describe("callback/build_agent fork lineage", () => {
  const SOURCE_SESSION_ID = "0192f5c4-7a3e-7d21-9b0e-4c1d2e3f4a5b";
  const forkSourceWire = () => ({
    source_member: {
      mob_id: "home",
      role: "domain",
      member: "mk--domain_ccalendar",
      future_member_field: true,
    },
    source_session_id: SOURCE_SESSION_ID,
    future_source_field: { nested: [1, 2] },
  });

  async function build(
    options: Record<string, unknown>,
    mutate: (opts: SessionBuildOptions) => void = () => {},
  ): Promise<{ opts: SessionBuildOptions; result: Record<string, unknown> }> {
    let seen: SessionBuildOptions | null = null;
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(opts: SessionBuildOptions): Promise<void> {
        seen = opts;
        mutate(opts);
      },
    });
    const result = (await dispatcher.handleCallback("callback/build_agent", {
      options,
    })) as Record<string, unknown>;
    assert.ok(seen !== null, "buildAgent must run");
    return { opts: seen, result };
  }

  it("types the source and its durable identity, ignoring unknown fields", async () => {
    const { opts } = await build({
      scope_id: "s1",
      session_id: "child-session",
      labels: { session_id: "child-session" },
      fork_source: forkSourceWire(),
      fork_source_identity: "domain:calendar",
      future_top_level_field: 1,
    });
    assert.deepEqual(opts.forkSource, {
      sourceMember: {
        mobId: "home",
        role: "domain",
        member: "mk--domain_ccalendar",
      },
      sourceSessionId: SOURCE_SESSION_ID,
    });
    assert.equal(opts.forkSourceIdentity, "domain:calendar");
    // The child keeps its own identity and session.
    assert.equal(opts.sessionId, "child-session");
    assert.deepEqual(opts.labels, { session_id: "child-session" });
  });

  it("never sends fork lineage back", async () => {
    for (const options of [
      { scope_id: "s1" },
      {
        scope_id: "s2",
        fork_source: forkSourceWire(),
        fork_source_identity: "domain:calendar",
      },
    ]) {
      const { result } = await build(options, (opts) => {
        // Receive-only: a builder cannot mint lineage into its response.
        (opts as unknown as Record<string, unknown>).forkSource = {
          sourceMember: { mobId: "x", role: "y", member: "z" },
          sourceSessionId: "spoofed",
        };
        (opts as unknown as Record<string, unknown>).forkSourceIdentity = "spoofed";
      });
      assert.equal("fork_source" in result, false);
      assert.equal("fork_source_identity" in result, false);
    }
  });

  it("gives an ordinary build no fork source", async () => {
    for (const options of [
      { scope_id: "s1" },
      { scope_id: "s2", fork_source: null, fork_source_identity: null },
    ]) {
      const { opts } = await build(options);
      assert.equal(opts.forkSource, null);
      assert.equal(opts.forkSourceIdentity, null);
    }
  });

  it("fails the build on a malformed fork source", async () => {
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerBuilder({
      async buildAgent(): Promise<void> {
        throw new Error("must not be called");
      },
    });
    await assert.rejects(
      dispatcher.handleCallback("callback/build_agent", {
        options: {
          scope_id: "s1",
          fork_source: {
            source_member: { mob_id: "home", role: "domain", member: "m" },
          },
        },
      }),
      /source_session_id/,
    );
  });

  it("carries resume_session_id both ways", async () => {
    const { opts, result } = await build({
      scope_id: "s1",
      resume_session_id: "sid-resumed",
    });
    assert.equal(opts.resumeSessionId, "sid-resumed");
    assert.equal(result.resume_session_id, "sid-resumed");

    const minted = await build({ scope_id: "s2" }, (o) => {
      o.resumeSessionId = "sid-owner-789";
    });
    assert.equal(minted.result.resume_session_id, "sid-owner-789");
    const plain = await build({ scope_id: "s3" });
    assert.equal(plain.opts.resumeSessionId, null);
    assert.equal("resume_session_id" in plain.result, false);
  });
});
