/**
 * TDD tests for Identity-First Continuity types, runtime APIs,
 * provider interfaces, builder extensions, and callback dispatcher routing.
 *
 * Covers REQ-46 through REQ-51.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { runtimeConfig } from "./runtime-config.js";

// ---------------------------------------------------------------------------
// REQ-46: DurableAgentSpec
// ---------------------------------------------------------------------------

describe("DurableAgentSpec (REQ-46)", () => {
  it("interface has all fields (camelCase)", async () => {
    const { parseDurableAgentSpec } = await import("../src/types.js");
    const spec = parseDurableAgentSpec({
      identity: "triage:main",
      profile: "triage",
      addressability: "addressable",
      display_name: "Triage Agent",
      labels: { tier: "1" },
      context: { key: "value" },
      additional_instructions: ["Be concise."],
      runtime_mode_override: "turn_driven",
      backend: "external",
      placement: "12D3KooWExactRemoteHost",
      binding: {
        kind: "external",
        address: "tcp://127.0.0.1:4777",
        identity: {
          kind: "ed25519_public_key",
          public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        },
      },
    });
    assert.equal(spec.identity, "triage:main");
    assert.equal(spec.profile, "triage");
    assert.equal(spec.addressability, "addressable");
    assert.equal(spec.displayName, "Triage Agent");
    assert.deepEqual(spec.labels, { tier: "1" });
    assert.deepEqual(spec.context, { key: "value" });
    assert.deepEqual(spec.additionalInstructions, ["Be concise."]);
    assert.equal(spec.runtimeModeOverride, "turn_driven");
    assert.equal(spec.backend, "external");
    assert.equal(spec.placement, "12D3KooWExactRemoteHost");
    assert.deepEqual(spec.binding, {
      kind: "external",
      address: "tcp://127.0.0.1:4777",
      identity: {
        kind: "ed25519_public_key",
        public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
      },
    });
  });

  it("addressability defaults to 'addressable' when omitted", async () => {
    const { parseDurableAgentSpec } = await import("../src/types.js");
    const spec = parseDurableAgentSpec({
      identity: "gate:main",
      profile: "gate",
    });
    assert.equal(spec.addressability, "addressable");
  });

  it("optional fields default gracefully", async () => {
    const { parseDurableAgentSpec } = await import("../src/types.js");
    const spec = parseDurableAgentSpec({
      identity: "worker:1",
      profile: "worker",
    });
    assert.equal(spec.displayName, null);
    assert.deepEqual(spec.labels, {});
    assert.equal(spec.context, null);
    assert.deepEqual(spec.additionalInstructions, []);
  });

  it("serializes to wire format (snake_case)", async () => {
    const { durableAgentSpecToDict } = await import("../src/types.js");
    const wire = durableAgentSpecToDict({
      identity: "triage:main",
      profile: "triage",
      addressability: "internal_only",
      displayName: "Triage",
      labels: { tier: "1" },
      context: { k: "v" },
      additionalInstructions: ["Be concise."],
      runtimeModeOverride: "turn_driven",
      backend: "external",
      placement: "12D3KooWExactRemoteHost",
      binding: {
        kind: "external",
        address: "tcp://127.0.0.1:4777",
        identity: {
          kind: "ed25519_public_key",
          public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
        },
      },
    });
    assert.equal(wire.identity, "triage:main");
    assert.equal(wire.profile, "triage");
    assert.equal(wire.addressability, "internal_only");
    assert.equal(wire.display_name, "Triage");
    assert.deepEqual(wire.labels, { tier: "1" });
    assert.deepEqual(wire.context, { k: "v" });
    assert.deepEqual(wire.additional_instructions, ["Be concise."]);
    assert.equal(wire.runtime_mode_override, "turn_driven");
    assert.equal(wire.backend, "external");
    assert.equal(wire.placement, "12D3KooWExactRemoteHost");
    assert.deepEqual(wire.binding, {
      kind: "external",
      address: "tcp://127.0.0.1:4777",
      identity: {
        kind: "ed25519_public_key",
        public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
      },
    });
  });

  // runtimeModeOverride / initialMessage are the two per-identity knobs the
  // Rust DurableAgentSpec deserializes (MobRuntimeMode snake_case, untagged
  // ContentInput). They must round-trip, and stay absent when unset so a
  // roster that never sets them is byte-identical to earlier SDKs.
  it("parses initial_message as text or as content blocks", async () => {
    const { parseDurableAgentSpec } = await import("../src/types.js");
    const text = parseDurableAgentSpec({
      identity: "archivist:main",
      profile: "archivist",
      initial_message: "Index yesterday's mail.",
    });
    assert.equal(text.initialMessage, "Index yesterday's mail.");

    const blocks = parseDurableAgentSpec({
      identity: "archivist:main",
      profile: "archivist",
      initial_message: [
        { type: "text", text: "Look at this." },
        { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
      ],
    });
    assert.deepEqual(blocks.initialMessage, [
      { type: "text", text: "Look at this." },
      { type: "image", mediaType: "image/png", source: "inline", data: "AAAA" },
    ]);

    const unset = parseDurableAgentSpec({ identity: "w:1", profile: "worker" });
    assert.equal(unset.initialMessage, null);
    assert.equal(unset.runtimeModeOverride, null);
  });

  it("serializes runtime_mode_override and initial_message, omitting them when unset", async () => {
    const { durableAgentSpecToDict } = await import("../src/types.js");
    const base = {
      identity: "archivist:main",
      profile: "archivist",
      addressability: "addressable",
      displayName: null,
      labels: {},
      context: null,
      additionalInstructions: [],
    };
    const text = durableAgentSpecToDict({
      ...base,
      runtimeModeOverride: "turn_driven",
      initialMessage: "Index yesterday's mail.",
    });
    assert.equal(text.runtime_mode_override, "turn_driven");
    assert.equal(text.initial_message, "Index yesterday's mail.");

    const blocks = durableAgentSpecToDict({
      ...base,
      initialMessage: [
        { type: "text", text: "Look at this." },
        { type: "image", mediaType: "image/png", source: "inline", data: "AAAA" },
      ],
    });
    assert.deepEqual(blocks.initial_message, [
      { type: "text", text: "Look at this." },
      { type: "image", media_type: "image/png", source: "inline", data: "AAAA" },
    ]);

    const unset = durableAgentSpecToDict(base);
    assert.equal("runtime_mode_override" in unset, false);
    assert.equal("initial_message" in unset, false);
  });

  it("the roster callback payload carries both fields verbatim", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerRosterProvider({
      async roster() {
        return [
          {
            identity: "archivist:main",
            profile: "archivist",
            addressability: "addressable",
            displayName: null,
            labels: {},
            context: null,
            additionalInstructions: [],
            runtimeModeOverride: "turn_driven",
            initialMessage: "Index yesterday's mail.",
          },
          {
            identity: "worker:1",
            profile: "worker",
            addressability: "internal_only",
            displayName: null,
            labels: {},
            context: null,
            additionalInstructions: [],
          },
        ];
      },
    });
    const payload = (await dispatcher.handleCallback(
      "callback/roster_provider/roster",
      { context: {} },
    )) as Array<Record<string, unknown>>;
    assert.equal(payload[0].runtime_mode_override, "turn_driven");
    assert.equal(payload[0].initial_message, "Index yesterday's mail.");
    assert.equal("runtime_mode_override" in payload[1], false);
    assert.equal("initial_message" in payload[1], false);
  });

  it("never turns a present placement into local omission", async () => {
    const { durableAgentSpecToDict } = await import("../src/types.js");
    const wire = durableAgentSpecToDict({
      identity: "triage:main",
      profile: "triage",
      addressability: "addressable",
      displayName: null,
      labels: {},
      context: null,
      additionalInstructions: [],
      placement: "",
    });
    assert.equal(
      wire.placement,
      "",
      "Rust must receive and reject an invalid ref instead of seeing local None",
    );
  });
});

// ---------------------------------------------------------------------------
// REQ-49: DispatchInput + DispatchContentBlock
// ---------------------------------------------------------------------------

describe("DispatchInput + DispatchContentBlock (REQ-49)", () => {
  it("text-only dispatch input round-trips", async () => {
    const { parseDispatchInput, dispatchInputToDict } = await import("../src/types.js");
    const input = parseDispatchInput({
      content: "Hello, agent!",
      origin: "connector",
      correlation_id: "corr-1",
      idempotency_key: "idem-1",
    });
    assert.equal(input.content, "Hello, agent!");
    assert.equal(input.origin, "connector");
    assert.equal(input.correlationId, "corr-1");
    assert.equal(input.idempotencyKey, "idem-1");

    const wire = dispatchInputToDict(input);
    assert.equal(wire.content, "Hello, agent!");
    assert.equal(wire.origin, "connector");
    assert.equal(wire.correlation_id, "corr-1");
    assert.equal(wire.idempotency_key, "idem-1");
  });

  it("multimodal dispatch with content blocks", async () => {
    const { parseDispatchInput, dispatchInputToDict } = await import("../src/types.js");
    const input = parseDispatchInput({
      content: [
        { type: "text", text: "Describe this." },
        { type: "image", media_type: "image/png", data: "base64data" },
      ],
      origin: "scheduler",
    });
    assert.ok(Array.isArray(input.content));
    const blocks = input.content as unknown as Array<Record<string, unknown>>;
    assert.equal(blocks.length, 2);
    assert.equal(blocks[0].type, "text");
    assert.equal(blocks[0].text, "Describe this.");
    assert.equal(blocks[1].type, "image");
    assert.equal(blocks[1].mediaType, "image/png");
    assert.equal(blocks[1].source, "inline");

    const wire = dispatchInputToDict(input);
    const wireBlocks = wire.content as Array<Record<string, unknown>>;
    assert.equal(wireBlocks[1].media_type, "image/png");
    assert.equal(wireBlocks[1].source, "inline");
    assert.equal(wireBlocks[1].data, "base64data");
  });

  it("blob-backed image dispatch blocks round-trip", async () => {
    const { parseDispatchInput, dispatchInputToDict } = await import("../src/types.js");
    const blobId = `sha256:${"a".repeat(64)}`;
    const input = parseDispatchInput({
      content: [
        { type: "image", media_type: "image/png", source: "blob", blob_id: blobId },
      ],
      origin: "system",
    });
    assert.ok(Array.isArray(input.content));
    const blocks = input.content as unknown as Array<Record<string, unknown>>;
    assert.equal(blocks[0].type, "image");
    assert.equal(blocks[0].source, "blob");
    assert.equal(blocks[0].blobId, blobId);

    const wire = dispatchInputToDict(input);
    const wireBlocks = wire.content as Array<Record<string, unknown>>;
    assert.deepEqual(wireBlocks[0], {
      type: "image",
      media_type: "image/png",
      source: "blob",
      blob_id: blobId,
    });
  });

  it("all five origin values accepted", async () => {
    const { parseDispatchInput } = await import("../src/types.js");
    for (const origin of ["connector", "scheduler", "policy", "flow", "system"]) {
      const input = parseDispatchInput({ content: "test", origin });
      assert.equal(input.origin, origin);
    }
  });

  it("optional fields default to undefined", async () => {
    const { parseDispatchInput } = await import("../src/types.js");
    const input = parseDispatchInput({
      content: "test",
      origin: "system",
    });
    assert.equal(input.correlationId, undefined);
    assert.equal(input.idempotencyKey, undefined);
  });
});

// ---------------------------------------------------------------------------
// REQ-49a: AgentBuildContext, AgentBuildDraft, ExternalToolDef, ManagedPeerEdge
// ---------------------------------------------------------------------------

describe("AgentBuildContext + AgentBuildDraft (REQ-49a)", () => {
  it("AgentBuildContext parses from wire", async () => {
    const { parseAgentBuildContext } = await import("../src/types.js");
    const ctx = parseAgentBuildContext({
      identity: "lead:main",
      active_peers: ["worker:1", "worker:2"],
      managed_edges: [{ a: "lead:main", b: "worker:1" }],
    });
    assert.equal(ctx.identity, "lead:main");
    assert.deepEqual(ctx.activePeers, ["worker:1", "worker:2"]);
    assert.equal(ctx.managedEdges.length, 1);
    assert.equal(ctx.managedEdges[0].a, "lead:main");
    assert.equal(ctx.managedEdges[0].b, "worker:1");
  });

  it("AgentBuildDraft parses and serializes", async () => {
    const { parseAgentBuildDraft, agentBuildDraftToDict } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({
      model: "claude-sonnet-4-5",
      system_prompt: "You are helpful.",
      additional_instructions: ["Be brief."],
      labels: { role: "lead" },
      app_context: { custom: true },
      external_tools: [
        { name: "search", description: "Search the web", input_schema: { type: "object" } },
      ],
    });
    assert.equal(draft.model, "claude-sonnet-4-5");
    assert.equal(draft.systemPrompt, "You are helpful.");
    assert.deepEqual(draft.additionalInstructions, ["Be brief."]);
    assert.deepEqual(draft.labels, { role: "lead" });
    assert.deepEqual(draft.appContext, { custom: true });
    assert.equal(draft.externalTools.length, 1);
    assert.equal(draft.externalTools[0].name, "search");

    const wire = agentBuildDraftToDict(draft);
    assert.equal(wire.model, "claude-sonnet-4-5");
    assert.equal(wire.system_prompt, "You are helpful.");
    assert.deepEqual(wire.additional_instructions, ["Be brief."]);
    const wireTools = wire.external_tools as Array<Record<string, unknown>>;
    assert.equal(wireTools[0].name, "search");
    assert.deepEqual(wireTools[0].input_schema, { type: "object" });
  });

  it("AgentBuildDraft round-trips provider params it does not understand", async () => {
    // The gateway replaces the draft wholesale with whatever the SDK returns,
    // so a customizer that never touches provider params must still hand them
    // back untouched — otherwise every SDK-backed build silently clears the
    // identity's cache policy.
    const { parseAgentBuildDraft, agentBuildDraftToDict } = await import("../src/types.js");
    const providerParams = {
      provider_tag: {
        provider: "open_ai",
        prompt_cache_key: "tenant-a:stable-prefix",
        prompt_cache_options: { mode: "implicit", ttl: "30m" },
      },
    };
    const draft = parseAgentBuildDraft({ model: "gpt-5.5", provider_params: providerParams });

    assert.deepEqual(draft.providerParams, providerParams);
    assert.deepEqual(agentBuildDraftToDict(draft).provider_params, providerParams);
  });

  it("AgentBuildDraft omits provider params from the wire when unset", async () => {
    const { parseAgentBuildDraft, agentBuildDraftToDict } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({ model: "gpt-5.5" });
    assert.equal(draft.providerParams, null);
    assert.equal("provider_params" in agentBuildDraftToDict(draft), false);
  });

  it("AgentBuildDraft null fields default gracefully", async () => {
    const { parseAgentBuildDraft } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({});
    assert.equal(draft.model, null);
    assert.equal(draft.systemPrompt, null);
    assert.deepEqual(draft.additionalInstructions, []);
    assert.deepEqual(draft.labels, {});
    assert.equal(draft.appContext, null);
    assert.deepEqual(draft.externalTools, []);
  });

  // These mirrors are hand-written, and a hand-written mirror drops what it
  // does not name — silently. For the completion cursor that failure is
  // invisible in the worst way: `parse` yields no cursor, the waiter never
  // sees a completion, and the consumer waits out its whole timeout on a turn
  // that finished. That is the defect the cursor exists to fix, reintroduced
  // one layer down, so the round trip is asserted on VALUES in both
  // directions rather than on the key being present.

  it("IdentityInspection round-trips completion_cursor in both directions", async () => {
    const { parseIdentityInspection, identityInspectionToDict } = await import(
      "../src/types.js"
    );
    const wire = {
      identity: "triage:main",
      output_preview: "ACK",
      is_final: false,
      peer_reachable_count: 2,
      intent_managed: false,
      completion_cursor: { epoch: 7, turns: 3 },
    };

    const parsed = parseIdentityInspection(wire);
    assert.equal(parsed.completionCursor?.epoch, 7);
    assert.equal(parsed.completionCursor?.turns, 3);

    const back = identityInspectionToDict(parsed);
    assert.deepEqual(back.completion_cursor, { epoch: 7, turns: 3 });
    assert.deepEqual(back, wire);

    // And the full cycle is a fixed point — nothing is lost on re-parse.
    assert.deepEqual(parseIdentityInspection(back), parsed);
  });

  it("DispatchResult and SendResult round-trip completion_baseline in both directions", async () => {
    const {
      parseDispatchResult,
      dispatchResultToDict,
      parseSendResult,
      sendResultToDict,
    } = await import("../src/types.js");

    const dispatchWire = {
      fencing_token: 9,
      durable: true,
      completion_baseline: { epoch: 9, turns: 4 },
    };
    const dispatch = parseDispatchResult(dispatchWire);
    assert.equal(dispatch.completionBaseline?.epoch, 9);
    assert.equal(dispatch.completionBaseline?.turns, 4);
    assert.deepEqual(dispatchResultToDict(dispatch), dispatchWire);
    assert.deepEqual(parseDispatchResult(dispatchResultToDict(dispatch)), dispatch);

    const sendWire = {
      fencing_token: 9,
      completion_baseline: { epoch: 9, turns: 4 },
    };
    const send = parseSendResult(sendWire);
    assert.equal(send.completionBaseline?.epoch, 9);
    assert.equal(send.completionBaseline?.turns, 4);
    assert.deepEqual(sendResultToDict(send), sendWire);
    assert.deepEqual(parseSendResult(sendResultToDict(send)), send);
  });

  it("payloads from a gateway with no cursor parse, and absence stays null", async () => {
    // Backward compatibility. `null` is the documented default and is NOT the
    // same as `{epoch: 0, turns: 0}` — absence means "this gateway does not
    // track completions", which a caller must not read as "no turns yet".
    const { parseIdentityInspection, parseDispatchResult, parseSendResult } =
      await import("../src/types.js");

    const inspection = parseIdentityInspection({
      identity: "triage:main",
      output_preview: "ACK",
      is_final: true,
      peer_reachable_count: 1,
    });
    assert.equal(inspection.completionCursor, null);
    assert.equal(inspection.outputPreview, "ACK");
    assert.equal(inspection.isFinal, true);
    assert.equal(inspection.peerReachableCount, 1);

    assert.equal(
      parseDispatchResult({ fencing_token: 2, durable: false }).completionBaseline,
      null,
    );
    assert.equal(parseSendResult({ fencing_token: 2 }).completionBaseline, null);

    // An explicit JSON null (what a live alias reports) is also absence.
    assert.equal(
      parseIdentityInspection({ identity: "live:alias", completion_cursor: null })
        .completionCursor,
      null,
    );
  });

  it("ExternalToolDef round-trips", async () => {
    const { parseExternalToolDef, externalToolDefToDict } = await import("../src/types.js");
    const tool = parseExternalToolDef({
      name: "fetch",
      description: "Fetch a URL",
      input_schema: { type: "object", properties: { url: { type: "string" } } },
    });
    assert.equal(tool.name, "fetch");
    assert.equal(tool.description, "Fetch a URL");
    assert.deepEqual(tool.inputSchema, { type: "object", properties: { url: { type: "string" } } });

    const wire = externalToolDefToDict(tool);
    assert.equal((wire.input_schema as Record<string, unknown>).type, "object");
  });

  it("ManagedPeerEdge round-trips", async () => {
    const { parseManagedPeerEdge, managedPeerEdgeToDict } = await import("../src/types.js");
    const edge = parseManagedPeerEdge({ a: "lead:main", b: "worker:1" });
    assert.equal(edge.a, "lead:main");
    assert.equal(edge.b, "worker:1");

    const wire = managedPeerEdgeToDict(edge);
    assert.equal(wire.a, "lead:main");
    assert.equal(wire.b, "worker:1");
  });

  it("AgentBuildDraft.registerTool appends a def and stores the handler in-process", async () => {
    const { parseAgentBuildDraft, agentBuildDraftToDict } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({});
    draft.registerTool("echo", async (args) => args, "echoes input", { type: "object" });
    assert.equal(draft.externalTools.length, 1);
    assert.equal(draft.externalTools[0].name, "echo");
    assert.equal(draft.externalTools[0].description, "echoes input");
    assert.equal(draft.toolHandlers.size, 1);

    // The handler must NOT leak onto the wire — defs only.
    const wire = agentBuildDraftToDict(draft);
    const wireTools = wire.external_tools as Array<Record<string, unknown>>;
    assert.equal(wireTools[0].name, "echo");
    assert.equal(wireTools[0].handler, undefined);
  });

  it("AgentBuildDraft.registerTool defaults the schema", async () => {
    const { parseAgentBuildDraft } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({});
    draft.registerTool("t", async () => null);
    assert.deepEqual(draft.externalTools[0].inputSchema, { type: "object" });
  });
});

// ---------------------------------------------------------------------------
// AgentCustomizer external-tool handler registration (restore-path parity)
// ---------------------------------------------------------------------------

describe("AgentCustomizer customize_build handler registration", () => {
  const CONTEXT = { identity: "agent:alpha", active_peers: [], managed_edges: [] };
  const SPEC = { identity: "agent:alpha", profile: "default" };

  async function makeDispatcher() {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerAgentCustomizer({
      async customizeBuild(_ctx, _spec, draft) {
        draft.registerTool("echo", (args) => ({ echo: args.input }), "echoes input");
      },
    });
    return dispatcher;
  }

  it("captures handlers registered in customizeBuild and dispatches call_tool", async () => {
    const dispatcher = await makeDispatcher();
    const result = (await dispatcher.handleCallback(
      "callback/agent_customizer/customize_build",
      { scope_id: "c1", context: CONTEXT, spec: SPEC, draft: {} },
    )) as Record<string, unknown>;
    const tools = result.external_tools as Array<Record<string, unknown>>;
    assert.equal(tools[0].name, "echo");
    assert.equal(tools[0].handler, undefined);

    const called = await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "c1",
      tool: "echo",
      arguments: { input: "hi" },
    });
    assert.deepEqual(called, { content: { echo: "hi" } });
  });

  it("isolates handlers by scope", async () => {
    const dispatcher = await makeDispatcher();
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "c1",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    });
    await assert.rejects(
      dispatcher.handleCallback("callback/call_tool", {
        scope_id: "c2",
        tool: "echo",
        arguments: {},
      }),
      /no handler registered/,
    );
  });

  it("re-registers under a fresh scope on restore; latest dispatches and stale releases cleanly", async () => {
    const dispatcher = await makeDispatcher();
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "customize-agent:alpha-1",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    });
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "customize-agent:alpha-2",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    });
    const r = await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "customize-agent:alpha-2",
      tool: "echo",
      arguments: { input: "yo" },
    });
    assert.deepEqual(r, { content: { echo: "yo" } });

    dispatcher.releaseScope("customize-agent:alpha-1");
    const r2 = await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "customize-agent:alpha-2",
      tool: "echo",
      arguments: { input: "ok" },
    });
    assert.deepEqual(r2, { content: { echo: "ok" } });
  });

  it("degrades gracefully when the gateway sends no scope_id", async () => {
    const dispatcher = await makeDispatcher();
    const result = (await dispatcher.handleCallback(
      "callback/agent_customizer/customize_build",
      { context: CONTEXT, spec: SPEC, draft: {} },
    )) as Record<string, unknown>;
    const tools = result.external_tools as Array<Record<string, unknown>>;
    assert.equal(tools[0].name, "echo");
  });

  it("a customizer-registered tool returns image content via toolContent()", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const { textBlock, imageBlock, toolContent } = await import("../src/tool-content.js");
    const blocks = [textBlock("see this:"), imageBlock("image/png", "aGVsbG8=")];
    const dispatcher = new CallbackDispatcher();
    dispatcher.registerAgentCustomizer({
      async customizeBuild(_ctx, _spec, draft) {
        draft.registerTool("shot", () => toolContent(...blocks));
      },
    });
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "c1",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    });
    const result = await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "c1",
      tool: "shot",
      arguments: {},
    });
    assert.deepEqual(result, { content_blocks: blocks });
  });

  it("a plain array return is NOT reinterpreted as content blocks", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    const data = [{ type: "text", text: "this is data" }];
    dispatcher.registerAgentCustomizer({
      async customizeBuild(_ctx, _spec, draft) {
        draft.registerTool("rows", () => data);
      },
    });
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "c1",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    });
    const result = (await dispatcher.handleCallback("callback/call_tool", {
      scope_id: "c1",
      tool: "rows",
      arguments: {},
    })) as Record<string, unknown>;
    assert.equal(result.content_blocks, undefined);
    assert.deepEqual(result, { content: data });
  });
});

// ---------------------------------------------------------------------------
// One live customize scope per (mob, member name)
// ---------------------------------------------------------------------------

// One gateway can host several mobs, and two of them can have a member of one
// name. The customize callback names the member's mob as `mob_id` beside
// `scope_id`; a gateway of an earlier release names none.
describe("AgentCustomizer scope keyed by mob and member name", () => {
  const CONTEXT = { identity: "agent:alpha", active_peers: [], managed_edges: [] };
  const SPEC = { identity: "agent:alpha", profile: "default" };
  const CUSTOMIZE = "callback/agent_customizer/customize_build";
  const LEFT_OUT = Symbol("mob_id left out");

  // A dispatcher whose customizer registers one tool, whose answer names the
  // build that registered it, and records the mob each build's context names.
  async function makeDispatcher() {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    const mobs: unknown[] = [];
    let builds = 0;
    dispatcher.registerAgentCustomizer({
      async customizeBuild(ctx, _spec, draft) {
        builds += 1;
        const build = builds;
        mobs.push("mobId" in ctx ? ctx.mobId : "the context has no mobId");
        draft.registerTool("lookup", () => ({ registered_by_build: build }));
      },
    });
    return { dispatcher, mobs };
  }

  type Dispatcher = Awaited<ReturnType<typeof makeDispatcher>>["dispatcher"];

  // Params of one customize callback. `mob` is sent as given (a null
  // included) unless it is left out.
  function buildParams(
    scopeId: string,
    mob: unknown = LEFT_OUT,
    extra: Record<string, unknown> = {},
  ): Record<string, unknown> {
    const params: Record<string, unknown> = {
      scope_id: scopeId,
      context: CONTEXT,
      spec: SPEC,
      draft: {},
    };
    if (mob !== LEFT_OUT) params.mob_id = mob;
    return { ...params, ...extra };
  }

  function lookup(dispatcher: Dispatcher, scopeId: string): Promise<unknown> {
    return dispatcher.handleCallback("callback/call_tool", {
      scope_id: scopeId,
      tool: "lookup",
      arguments: {},
    });
  }

  function liveScopes(dispatcher: Dispatcher): string[] {
    const scopes = (dispatcher as unknown as { _scopeTools: Map<string, string[]> })._scopeTools;
    return [...scopes.keys()].sort();
  }

  it("each scope of two mobs reaches the handler its own build registered", async () => {
    const { dispatcher } = await makeDispatcher();
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1", "mob-a"));
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-2", "mob-b"));
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-3", "mob-a"));
    await assert.rejects(lookup(dispatcher, "customize-agent:alpha-1"), /no handler registered/);
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-2"), {
      content: { registered_by_build: 2 },
    });
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-3"), {
      content: { registered_by_build: 3 },
    });
    // One live scope per (mob, member name): two, not three.
    assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-2", "customize-agent:alpha-3"]);
  });

  it("two member names of one mob keep both scopes", async () => {
    // The member name stays part of the key.
    const { dispatcher } = await makeDispatcher();
    const beta = { identity: "agent:beta", active_peers: [], managed_edges: [] };
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1", "mob-a"));
    await dispatcher.handleCallback(
      CUSTOMIZE,
      buildParams("customize-agent:beta-2", "mob-a", { context: beta }),
    );
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-1"), {
      content: { registered_by_build: 1 },
    });
    assert.deepEqual(await lookup(dispatcher, "customize-agent:beta-2"), {
      content: { registered_by_build: 2 },
    });
  });

  it("a mob and a member name are never confused with another pair that joins to the same text", async () => {
    // Mob "a:b" with member "c" and mob "a" with member "b:c" are two members,
    // and so are the same pairs joined by any other separator.
    for (const [first, second] of [
      [["a:b", "c"], ["a", "b:c"]],
      [["a\u0000b", "c"], ["a", "b\u0000c"]],
      [['a","b', "c"], ["a", 'b","c']],
    ]) {
      const { dispatcher } = await makeDispatcher();
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("s-1", first[0], {
          context: { identity: first[1], active_peers: [], managed_edges: [] },
        }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("s-2", second[0], {
          context: { identity: second[1], active_peers: [], managed_edges: [] },
        }),
      );
      assert.deepEqual(await lookup(dispatcher, "s-1"), { content: { registered_by_build: 1 } });
      assert.deepEqual(await lookup(dispatcher, "s-2"), { content: { registered_by_build: 2 } });
    }
  });

  it("one mob alone behaves as before", async () => {
    // One mob on the gateway: every build names the same mob, and a host
    // observes what it observed with a gateway that names none.
    const { dispatcher } = await makeDispatcher();
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1", "mob-a"));
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-2", "mob-a"));
    await assert.rejects(lookup(dispatcher, "customize-agent:alpha-1"), /no handler registered/);
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-2"), {
      content: { registered_by_build: 2 },
    });
    assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-2"]);
  });

  it("a member the SDK does not know is ignored beside the context and inside it", async () => {
    const { dispatcher } = await makeDispatcher();
    const result = (await dispatcher.handleCallback(
      CUSTOMIZE,
      buildParams("customize-agent:alpha-1", LEFT_OUT, {
        some_later_member: "x",
        context: { ...CONTEXT, some_later_member: "x" },
      }),
    )) as Record<string, unknown>;
    const tools = result.external_tools as Array<Record<string, unknown>>;
    assert.equal(tools[0].name, "lookup");
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-1"), {
      content: { registered_by_build: 1 },
    });
  });

  it("a mob inside the context object is not the member's mob", async () => {
    // The mob travels beside the context. A `mob_id` inside the context
    // object is a member this SDK does not know: it names no mob for the
    // customizer and none for the key.
    const { dispatcher, mobs } = await makeDispatcher();
    const context = { ...CONTEXT, mob_id: "mob-inside" };
    await dispatcher.handleCallback(
      CUSTOMIZE,
      buildParams("customize-agent:alpha-1", LEFT_OUT, { context }),
    );
    await dispatcher.handleCallback(
      CUSTOMIZE,
      buildParams("customize-agent:alpha-2", "mob-a", { context }),
    );
    assert.deepEqual(mobs, ["the context has no mobId", "mob-a"]);
    // Two keys (no mob, and mob-a): neither build released the other.
    assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-1", "customize-agent:alpha-2"]);
  });

  it("a mob_id that is a number is refused and releases nothing", async () => {
    // A `mob_id` that is present and not text is never read as no mob: the
    // callback is refused, the customizer is not asked, and the build that
    // named no mob keeps its scope.
    const { ContractMismatchError } = await import("../src/errors.js");
    const { dispatcher, mobs } = await makeDispatcher();
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1"));
    await assert.rejects(
      dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-2", 7)),
      ContractMismatchError,
    );
    assert.deepEqual(mobs, ["the context has no mobId"]);
    assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-1"), {
      content: { registered_by_build: 1 },
    });
    assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-1"]);
  });

  it("an empty mob_id is a mob of its own", async () => {
    // Text is a mob's id, the empty text included: it is not read as no mob,
    // so it and a build that names none keep their scopes.
    const { dispatcher, mobs } = await makeDispatcher();
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1"));
    await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-2", ""));
    assert.deepEqual(mobs, ["the context has no mobId", ""]);
    assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-1", "customize-agent:alpha-2"]);
  });

  // Beside `mob_id` the callback names the mob's incarnation as
  // `mob_incarnation`. A build that names another incarnation of a mob
  // releases every scope kept for that mob id; a gateway of an earlier
  // release names none and is served as before.
  describe("scopes of a mob created under a used id", () => {
    const BETA = { identity: "agent:beta", active_peers: [], managed_edges: [] };
    const GAMMA = { identity: "agent:gamma", active_peers: [], managed_edges: [] };

    it("a build of another incarnation releases every scope of that mob id", async () => {
      const { dispatcher } = await makeDispatcher();
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "one", context: BETA }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:gamma-3", "mob-a", { mob_incarnation: "two", context: GAMMA }),
      );
      for (const earlier of ["customize-agent:alpha-1", "customize-agent:beta-2"]) {
        await assert.rejects(lookup(dispatcher, earlier), /no handler registered/);
      }
      assert.deepEqual(await lookup(dispatcher, "customize-agent:gamma-3"), {
        content: { registered_by_build: 3 },
      });
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:gamma-3"]);
    });

    it("another incarnation of one mob keeps the scopes of another mob", async () => {
      const { dispatcher } = await makeDispatcher();
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-2", "mob-b", { mob_incarnation: "other" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-3", "mob-a", { mob_incarnation: "two", context: BETA }),
      );
      assert.deepEqual(await lookup(dispatcher, "customize-agent:alpha-2"), {
        content: { registered_by_build: 2 },
      });
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-2", "customize-agent:beta-3"]);
    });

    it("builds of one incarnation keep each other's scopes", async () => {
      // As before: one live scope per member of the mob, newest wins.
      const { dispatcher } = await makeDispatcher();
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "one", context: BETA }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-3", "mob-a", { mob_incarnation: "one" }),
      );
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-3", "customize-agent:beta-2"]);
    });

    it("a gateway that names no incarnation releases the member's own scope only", async () => {
      // A gateway of an earlier release names a mob and no incarnation,
      // before and after a build that named one.
      const { dispatcher } = await makeDispatcher();
      await dispatcher.handleCallback(CUSTOMIZE, buildParams("customize-agent:alpha-1", "mob-a"));
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "one", context: BETA }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:gamma-3", "mob-a", { context: GAMMA }),
      );
      assert.deepEqual(liveScopes(dispatcher), [
        "customize-agent:alpha-1",
        "customize-agent:beta-2",
        "customize-agent:gamma-3",
      ]);
    });

    it("an incarnation that is no text is refused and releases nothing", async () => {
      // A `mob_incarnation` that is present and is not a non-empty text is
      // never read as "none": the callback is refused, the customizer is
      // not asked, and the earlier build keeps its scope.
      const { ContractMismatchError } = await import("../src/errors.js");
      for (const notText of [7, true, ["one"], { id: "one" }, ""]) {
        const { dispatcher, mobs } = await makeDispatcher();
        await dispatcher.handleCallback(
          CUSTOMIZE,
          buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
        );
        await assert.rejects(
          dispatcher.handleCallback(
            CUSTOMIZE,
            buildParams("customize-agent:beta-2", "mob-a", {
              mob_incarnation: notText,
              context: BETA,
            }),
          ),
          ContractMismatchError,
        );
        assert.deepEqual(mobs, ["mob-a"]);
        assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-1"]);
      }
    });

    it("a build that registers no tool releases the earlier mob's scopes too", async () => {
      // The release follows from what the gateway names, whatever the
      // customizer does with the build.
      const { CallbackDispatcher } = await import("../src/agent-builder.js");
      const dispatcher = new CallbackDispatcher();
      dispatcher.registerAgentCustomizer({
        async customizeBuild(ctx, _spec, draft) {
          if (ctx.identity === "agent:alpha") draft.registerTool("lookup", () => ({ ok: true }));
        },
      });
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "two", context: BETA }),
      );
      assert.deepEqual(liveScopes(dispatcher), []);
    });

    it("a build that ends after another incarnation was named registers no scope", async () => {
      // A build of the earlier mob is still with the customizer when a build
      // of the later mob arrives: when it ends, its tools are not registered.
      const { CallbackDispatcher } = await import("../src/agent-builder.js");
      const dispatcher = new CallbackDispatcher();
      let asked: () => void = () => {};
      const wasAsked = new Promise<void>((resolve) => {
        asked = resolve;
      });
      let release: () => void = () => {};
      const held = new Promise<void>((resolve) => {
        release = resolve;
      });
      dispatcher.registerAgentCustomizer({
        async customizeBuild(ctx, _spec, draft) {
          if (ctx.identity === "agent:alpha") {
            asked();
            await held;
          }
          draft.registerTool("lookup", () => ({ ok: true }));
        },
      });
      const earlier = dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await wasAsked;
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "two", context: BETA }),
      );
      release();
      await earlier;
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:beta-2"]);
    });

    it("a build of an earlier incarnation handled late releases and registers nothing", async () => {
      // A build of the earlier mob that is handled after a build of the later
      // one is of a mob that is gone: the later mob's scopes stay and the late
      // build's tools are not registered. The customizer is still asked and
      // its draft is answered. The later mob's next build is kept beside its
      // others.
      const { dispatcher, mobs } = await makeDispatcher();
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" }),
      );
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "two", context: BETA }),
      );
      const late = (await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:gamma-3", "mob-a", { mob_incarnation: "one", context: GAMMA }),
      )) as Record<string, unknown>;
      assert.equal(mobs.length, 3);
      const tools = late.external_tools as Array<Record<string, unknown>>;
      assert.equal(tools[0].name, "lookup");
      assert.deepEqual(await lookup(dispatcher, "customize-agent:beta-2"), {
        content: { registered_by_build: 2 },
      });
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:beta-2"]);
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:alpha-4", "mob-a", { mob_incarnation: "two" }),
      );
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:alpha-4", "customize-agent:beta-2"]);
    });

    it("the transport hands builds on in the order they arrived", async () => {
      // A destroy does not wait for the answer to a build it aborted, so the
      // first build of the destroyed mob can arrive just before the first
      // build of the mob created under its id. The transport starts the two
      // in the order they arrived: the later mob keeps its scopes, and its
      // next build registers its tools.
      const { PersistentTransport } = await import("../src/transport.js");
      const { dispatcher } = await makeDispatcher();
      const transport = new PersistentTransport("unused-test-gateway");
      const answered: string[] = [];
      let answeredBoth: () => void = () => {};
      const bothAnswered = new Promise<void>((resolve) => {
        answeredBoth = resolve;
      });
      (transport as any)._writeLine = (value: Record<string, unknown>) => {
        answered.push(String(value.id));
        if (answered.length === 2) answeredBoth();
      };
      transport.setCallbackHandler(dispatcher.handleCallback.bind(dispatcher));
      const line = (id: string, params: Record<string, unknown>): string =>
        JSON.stringify({ jsonrpc: "2.0", id, method: CUSTOMIZE, params });
      (transport as any)._handleLine(
        line("cb-1", buildParams("customize-agent:alpha-1", "mob-a", { mob_incarnation: "one" })),
      );
      (transport as any)._handleLine(
        line(
          "cb-2",
          buildParams("customize-agent:beta-2", "mob-a", { mob_incarnation: "two", context: BETA }),
        ),
      );
      await bothAnswered;
      assert.deepEqual(answered.sort(), ["cb-1", "cb-2"]);
      await dispatcher.handleCallback(
        CUSTOMIZE,
        buildParams("customize-agent:gamma-3", "mob-a", { mob_incarnation: "two", context: GAMMA }),
      );
      assert.deepEqual(liveScopes(dispatcher), ["customize-agent:beta-2", "customize-agent:gamma-3"]);
      assert.deepEqual(await lookup(dispatcher, "customize-agent:gamma-3"), {
        content: { registered_by_build: 3 },
      });
    });
  });
});

// ---------------------------------------------------------------------------
// Tool-result content-block helpers
// ---------------------------------------------------------------------------

describe("tool-content helpers", () => {
  it("textBlock / imageBlock / imageBlobBlock build the wire shapes", async () => {
    const { textBlock, imageBlock, imageBlobBlock } = await import("../src/tool-content.js");
    assert.deepEqual(textBlock("hi"), { type: "text", text: "hi" });
    assert.deepEqual(imageBlock("image/png", "aGVsbG8="), {
      type: "image",
      media_type: "image/png",
      source: "inline",
      data: "aGVsbG8=",
    });
    assert.deepEqual(imageBlobBlock("image/jpeg", "blob-123"), {
      type: "image",
      media_type: "image/jpeg",
      source: "blob",
      blob_id: "blob-123",
    });
  });

  it("toolContent wraps blocks into a ToolResultContent marker", async () => {
    const { textBlock, imageBlock, toolContent, ToolResultContent } = await import(
      "../src/tool-content.js"
    );
    const tc = toolContent(textBlock("a"), imageBlock("image/png", "aGVsbG8="));
    assert.ok(tc instanceof ToolResultContent);
    assert.deepEqual(tc.blocks, [
      { type: "text", text: "a" },
      { type: "image", media_type: "image/png", source: "inline", data: "aGVsbG8=" },
    ]);
  });
});

// ---------------------------------------------------------------------------
// REQ-49b: IdentityStatus
// ---------------------------------------------------------------------------

describe("IdentityStatus (REQ-49b)", () => {
  it("full IdentityStatus parses from wire", async () => {
    const { parseIdentityStatus } = await import("../src/types.js");
    const status = parseIdentityStatus({
      identity: "triage:main",
      state: "active",
      agent_runtime_id: "rt-abc",
      session_id: "sess-123",
      profile: "triage",
      addressability: "addressable",
      display_name: "Triage Lead",
      labels: { tier: "1" },
      generation: 2,
      intent_managed: false,
      wake_refusal: null,
    });
    assert.equal(status.identity, "triage:main");
    assert.equal(status.lifecycleState, "active");
    assert.equal(status.agentRuntimeId, "rt-abc");
    assert.equal(status.sessionId, "sess-123");
    assert.equal(status.displayName, "Triage Lead");
    assert.equal(status.profile, "triage");
    assert.equal(status.addressability, "addressable");
    assert.deepEqual(status.labels, { tier: "1" });
    assert.equal(status.generation, 2);
    assert.equal(status.intentManaged, false);
    assert.equal(status.wakeRefusal, null);
  });
});

// ---------------------------------------------------------------------------
// REQ-47: Identity-first runtime APIs on MetaRuntime
// ---------------------------------------------------------------------------

describe("Identity-first runtime APIs (REQ-47)", () => {
  // We test that the methods exist and have the right signature by calling
  // them on a runtime with a mock transport. We stub _rpc to capture calls.

  async function makeRuntime() {
    const { MetaRuntime } = await import("../src/runtime.js");
    const calls: {
      method: string;
      params: Record<string, unknown>;
      options?: { transportTimeoutMs?: number };
    }[] = [];
    const rt = new MetaRuntime(runtimeConfig());
    // Stub _rpc to capture calls and return plausible results
    (rt as unknown as Record<string, unknown>)._rpc = async (
      method: string,
      params?: Record<string, unknown>,
      options?: { transportTimeoutMs?: number },
    ) => {
      calls.push({ method, params: params ?? {}, options });
      // Return type-appropriate stubs
      if (method === "meta/status_identity") {
        return {
          identity: params?.identity ?? "x:1",
          state: "active",
          agent_runtime_id: "rt-1",
          session_id: "sess-1",
          profile: "worker",
          addressability: "addressable",
          labels: {},
          generation: 0,
          intent_managed: false,
          wake_refusal: null,
        };
      }
      return { accepted: true };
    };
    (rt as unknown as Record<string, unknown>)._running = true;
    return { rt, calls };
  }

  it("send(identity, string) calls RPC with content", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.send("triage:main", "Hello!");
    assert.equal(calls.length, 1);
    assert.equal(calls[0].method, "meta/send");
    assert.equal(calls[0].params.identity, "triage:main");
    assert.equal(calls[0].params.content, "Hello!");
  });

  it("send(identity, content blocks) sends array", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.send("triage:main", [{ type: "text", text: "Hello" }]);
    assert.equal(calls[0].params.identity, "triage:main");
    assert.ok(Array.isArray(calls[0].params.content));
  });

  it("send(identity, image block) sends strict image source shape", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.send("triage:main", [
      { type: "image", mediaType: "image/png", data: "abc" },
    ]);
    assert.deepEqual(calls[0].params.content, [
      {
        type: "image",
        media_type: "image/png",
        source: "inline",
        data: "abc",
      },
    ]);
  });

  it("dispatch(identity, dispatchInput)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.dispatch("triage:main", {
      content: "Hello",
      origin: "connector",
    });
    assert.equal(calls[0].method, "meta/dispatch");
    assert.equal(calls[0].params.identity, "triage:main");
    assert.equal((calls[0].params.dispatch_input as Record<string, unknown>).origin, "connector");
  });

  it("agent(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    const snap = await rt.agent("lead:main");
    assert.equal(calls[0].method, "meta/status_identity");
    assert.equal(calls[0].params.identity, "lead:main");
    assert.equal(snap.agentIdentity, "rt-1");
    assert.equal(snap.role, "worker");
  });

  it("subscribe(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.subscribe("worker:1");
    assert.equal(calls[0].method, "meta/subscribe");
    assert.equal(calls[0].params.identity, "worker:1");
  });

  it("status(identity) returns parsed IdentityStatus", async () => {
    const { rt, calls } = await makeRuntime();
    const status = await rt.status("triage:main");
    assert.equal(calls[0].method, "meta/status_identity");
    assert.equal(status.identity, "triage:main");
    assert.equal(status.lifecycleState, "active");
  });

  it("respawn(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.respawn("worker:1");
    assert.equal(calls[0].method, "meta/respawn");
    assert.equal(calls[0].params.identity, "worker:1");
  });

  it("retire(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.retire("worker:1");
    assert.equal(calls[0].method, "meta/retire");
    assert.equal(calls[0].params.identity, "worker:1");
  });

  it("activateMemberInstruction(identity, activation)", async () => {
    const { rt, calls } = await makeRuntime();
    const activation = {
      revision: {
        namespace: "host",
        key: "persona",
        revision_id: "r2",
        content_sha256: "0".repeat(64),
      },
      activation_id: "a2",
      expectation: { kind: "absent" },
      body: "Use your granted tools.",
    };
    await rt.activateMemberInstruction("worker:1", activation);
    assert.equal(calls[0].method, "meta/member_activate_instruction");
    assert.deepEqual(calls[0].params, { identity: "worker:1", activation });
    assert.equal(calls[0].options?.transportTimeoutMs, undefined);
    // A mid-turn call can block for a whole turn; the caller can widen this
    // call's reply timeout.
    await rt.activateMemberInstruction("worker:1", activation, {
      transportTimeoutMs: 600_000,
    });
    assert.equal(calls[1].options?.transportTimeoutMs, 600_000);
  });

  it("memberInstructionActivations(identity, query)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.memberInstructionActivations("worker:1", { namespace: "host", key: "persona" });
    assert.equal(calls[0].method, "meta/member_instruction_activations");
    assert.deepEqual(calls[0].params, {
      identity: "worker:1",
      namespace: "host",
      key: "persona",
    });
  });

  it("setInstruction(namespace, key, revision, body, target, options)", async () => {
    const { rt, calls } = await makeRuntime();
    const member = { kind: "member" as const, identity: "worker:1" };
    await rt.setInstruction("host", "persona", 1, "Be brief.", member);
    assert.equal(calls[0].method, "meta/set_instruction");
    assert.deepEqual(calls[0].params, {
      namespace: "host",
      key: "persona",
      revision: 1,
      body: "Be brief.",
      target: member,
    });
    const digest = `sha256:${"0".repeat(64)}`;
    await rt.setInstruction(
      "host",
      "tone",
      2,
      "Be formal.",
      { kind: "profile", profile: "worker" },
      { contentSha256: digest },
    );
    assert.deepEqual(calls[1].params, {
      namespace: "host",
      key: "tone",
      revision: 2,
      body: "Be formal.",
      target: { kind: "profile", profile: "worker" },
      content_sha256: digest,
    });
    await rt.setInstruction("host", "tone", 3, "Be formal.", { kind: "mob" });
    assert.deepEqual(calls[2].params.target, { kind: "mob" });
  });

  it("revokeInstruction(namespace, key, revision)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.revokeInstruction("host", "persona", 2);
    assert.equal(calls[0].method, "meta/revoke_instruction");
    assert.deepEqual(calls[0].params, { namespace: "host", key: "persona", revision: 2 });
  });

  it("instructionStatus(query)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.instructionStatus();
    assert.equal(calls[0].method, "meta/instruction_status");
    assert.deepEqual(calls[0].params, {});
    await rt.instructionStatus({
      identity: "worker:1",
      namespace: "host",
      key: "persona",
      offset: 0,
      limit: 10,
    });
    assert.deepEqual(calls[1].params, {
      identity: "worker:1",
      namespace: "host",
      key: "persona",
      offset: 0,
      limit: 10,
    });
    await rt.instructionStatus({ profile: "worker" });
    assert.deepEqual(calls[2].params, { profile: "worker" });
  });

  it("instructionStatus keeps unknown advisory values as the gateway sent them", async () => {
    const { rt } = await makeRuntime();
    const status = {
      mob_id: "gateway",
      generation: 2,
      desired: [],
      members: [
        {
          identity: "worker:1",
          profile: "worker",
          residency: "a_future_residency",
          delivery: { state: "current", recorded_generation: 2 },
          slots: [
            {
              namespace: "host",
              key: "persona",
              desired: { revision: 1, effect: "active" },
              state: { state: "pending", reason: "a_future_reason" },
            },
          ],
          effective_read: { status: "ok" },
          live_channel_open: false,
        },
      ],
    };
    (rt as unknown as Record<string, unknown>)._rpc = async () => status;
    assert.deepEqual(await rt.instructionStatus(), status);
  });

  it("pruneMemberSystemRows(identity, remove, options)", async () => {
    const { rt, calls } = await makeRuntime();
    const remove = [{ index: 0, content_sha256: `sha256:${"0".repeat(64)}` }];
    await rt.pruneMemberSystemRows("worker:1", remove);
    assert.equal(calls[0].method, "meta/prune_member_system_rows");
    assert.deepEqual(calls[0].params, { identity: "worker:1", remove });
    await rt.pruneMemberSystemRows("worker:1", remove, {
      retireSupersededActivationBodies: true,
      expectedParentRevision: `sha256:${"1".repeat(64)}`,
      note: "legacy prompts",
    });
    assert.deepEqual(calls[1].params, {
      identity: "worker:1",
      remove,
      retire_superseded_activation_bodies: true,
      expected_parent_revision: `sha256:${"1".repeat(64)}`,
      note: "legacy prompts",
    });
  });

  it("reset(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.reset("worker:1");
    assert.equal(calls[0].method, "meta/reset");
    assert.equal(calls[0].params.identity, "worker:1");
  });

  it("deleteIdentity(identity)", async () => {
    const { rt, calls } = await makeRuntime();
    await rt.deleteIdentity("worker:1");
    assert.equal(calls[0].method, "meta/delete_identity");
    assert.equal(calls[0].params.identity, "worker:1");
  });
});

// ---------------------------------------------------------------------------
// REQ-48: Provider interfaces (structural tests)
// ---------------------------------------------------------------------------

describe("Provider interfaces (REQ-48)", () => {
  // Structural compile-time tests: prove the interfaces exist and have
  // the right shape by constructing mock implementations.

  it("RosterProvider interface is implementable", async () => {
    const provider: import("../src/types.js").RosterProvider = {
      async roster(_context) {
        return [
          {
            identity: "triage:main",
            profile: "triage",
            addressability: "addressable",
            displayName: null,
            labels: {},
            context: null,
            additionalInstructions: [],
            backend: "external",
            binding: {
              kind: "external",
              address: "tcp://127.0.0.1:4777",
              identity: {
                kind: "ed25519_public_key",
                public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
              },
            },
          },
        ];
      },
    };
    const specs = await provider.roster({ mobDefinition: null, previousIdentities: [] });
    assert.equal(specs.length, 1);
    assert.equal(specs[0].identity, "triage:main");
    assert.equal(specs[0].backend, "external");
    assert.deepEqual(specs[0].binding, {
      kind: "external",
      address: "tcp://127.0.0.1:4777",
      identity: {
        kind: "ed25519_public_key",
        public_key: "ed25519:BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
      },
    });
  });

  it("AgentCustomizer interface is implementable", async () => {
    const customizer: import("../src/types.js").AgentCustomizer = {
      async customizeBuild(_context, _spec, draft) {
        draft.model = "claude-sonnet-4-5";
      },
    };
    const { parseAgentBuildDraft } = await import("../src/types.js");
    const draft = parseAgentBuildDraft({});
    await customizer.customizeBuild(
      { identity: "x:1", activePeers: [], managedEdges: [] },
      {
        identity: "x:1", profile: "worker", addressability: "addressable",
        displayName: null, labels: {}, context: null, additionalInstructions: [],
      },
      draft,
    );
    assert.equal(draft.model, "claude-sonnet-4-5");
  });

  it("TopologyProvider interface is implementable", async () => {
    const provider: import("../src/types.js").TopologyProvider = {
      async computeEdges(_targetIdentities, _context) {
        return [{ a: "lead:main", b: "worker:1" }];
      },
    };
    const edges = await provider.computeEdges(["lead:main", "worker:1"], {});
    assert.equal(edges.length, 1);
  });
});

// ---------------------------------------------------------------------------
// REQ-50: Builder identity-first providers
// ---------------------------------------------------------------------------

describe("Builder identity-first extensions (REQ-50)", () => {
  it("runtime init params advertise identity-first provider callbacks", async () => {
    const { MetaRuntime } = await import("../src/runtime.js");
    const rosterProvider = { async roster() { return []; } };
    const topologyProvider = { async computeEdges() { return []; } };
    const agentCustomizer = { async customizeBuild() {} };
    const rt = new MetaRuntime(runtimeConfig({ gatewayBin: "/bin/rpc_gateway", rosterProvider, agentCustomizer, topologyProvider }));

    const params = (rt as unknown as { _buildInitParams(): Record<string, unknown> })
      ._buildInitParams();
    assert.equal(params.has_roster_provider, true);
    assert.equal(params.has_topology_provider, true);
    assert.equal(params.has_agent_customizer, true);
  });
});

// ---------------------------------------------------------------------------
// REQ-51: CallbackDispatcher — provider routing
// ---------------------------------------------------------------------------

describe("CallbackDispatcher provider routing (REQ-51)", () => {
  it("routes callback/roster_provider/roster to RosterProvider", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();

    dispatcher.registerRosterProvider({
      async roster(_context) {
        return [
          {
            identity: "triage:main",
            profile: "triage",
            addressability: "addressable",
            displayName: null,
            labels: {},
            context: null,
            additionalInstructions: [],
          },
        ];
      },
    });

    const result = await dispatcher.handleCallback("callback/roster_provider/roster", {
      context: {},
    });
    const specs = result as Array<Record<string, unknown>>;
    assert.equal(specs.length, 1);
    assert.equal(specs[0].identity, "triage:main");
  });

  it("hands the roster callback the gateway's typed RosterContext", async () => {
    // Regression: the roster callback used to receive `{}`. The gateway sends
    // the Rust `RosterContext` under `params.context` (same envelope as the
    // topology and customizer callbacks); the provider must see both fields.
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();

    const seen: unknown[] = [];
    dispatcher.registerRosterProvider({
      async roster(context) {
        seen.push(context);
        return [];
      },
    });

    await dispatcher.handleCallback("callback/roster_provider/roster", {
      context: {
        mob_definition: {
          id: "household",
          profiles: { assistant: { model: "gpt-5.5" } },
        },
        previous_identities: ["a:main", "b:main"],
      },
    });
    assert.equal(seen.length, 1);
    assert.deepEqual(seen[0], {
      mobDefinition: {
        id: "household",
        profiles: { assistant: { model: "gpt-5.5" } },
      },
      previousIdentities: ["a:main", "b:main"],
    });

    // An older gateway that sends a bare `{}` (or no context at all) still
    // yields a typed context with defaults.
    await dispatcher.handleCallback("callback/roster_provider/roster", { context: {} });
    await dispatcher.handleCallback("callback/roster_provider/roster", {});
    assert.deepEqual(seen[1], { mobDefinition: null, previousIdentities: [] });
    assert.deepEqual(seen[2], { mobDefinition: null, previousIdentities: [] });
  });

  it("routes callback/topology_provider/compute_edges to TopologyProvider", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();

    dispatcher.registerTopologyProvider({
      async computeEdges(targetIdentities, _context) {
        return [{ a: targetIdentities[0], b: targetIdentities[1] }];
      },
    });

    const result = await dispatcher.handleCallback("callback/topology_provider/compute_edges", {
      target_identities: ["lead:main", "worker:1"],
      context: {},
    });
    const edges = result as Array<Record<string, string>>;
    assert.equal(edges.length, 1);
    assert.equal(edges[0].a, "lead:main");
  });

  it("routes callback/agent_customizer/customize_build to AgentCustomizer", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();

    dispatcher.registerAgentCustomizer({
      async customizeBuild(_context, _spec, draft) {
        draft.model = "claude-sonnet-4-5";
        draft.externalTools.push({
          name: "search",
          description: "Search",
          inputSchema: {},
        });
      },
    });

    const result = await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      context: { identity: "x:1", active_peers: [], managed_edges: [] },
      spec: { identity: "x:1", profile: "worker", addressability: "addressable" },
      draft: { model: null, system_prompt: null, additional_instructions: [], labels: {}, app_context: null, external_tools: [] },
    }) as Record<string, unknown>;
    assert.equal(result.model, "claude-sonnet-4-5");
    const tools = result.external_tools as Array<Record<string, unknown>>;
    assert.equal(tools.length, 1);
    assert.equal(tools[0].name, "search");
  });

  it("routes callback/agent_customizer/after_create to AgentCustomizer", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    let afterCreateCalled = false;

    dispatcher.registerAgentCustomizer({
      async customizeBuild() {},
      async afterCreate(_identity, _agentRuntimeId, _sessionId) {
        afterCreateCalled = true;
      },
    });

    await dispatcher.handleCallback("callback/agent_customizer/after_create", {
      identity: "x:1",
      agent_runtime_id: "rt-1",
      session_id: "sess-1",
    });
    assert.equal(afterCreateCalled, true);
  });
});

// ---------------------------------------------------------------------------
// The three callbacks that build a member carry the member's mob
// ---------------------------------------------------------------------------

describe("callbacks of a host with more than one mob", () => {
  // A host that serves more than one mob sends a top-level `mob_id` with
  // customize_build, after_create and compute_edges. Two mobs can each have a
  // member of the same name, so the callback tools a customizer registers are
  // kept per mob and identity: a build in one mob leaves the other mob's
  // tools in place. A host that sends no `mob_id` has its own mob only; its
  // callbacks read as before.
  const CONTEXT = { identity: "agent:alpha", active_peers: [], managed_edges: [] };
  const SPEC = { identity: "agent:alpha", profile: "default" };

  async function makeDispatcher() {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    const buildContexts: unknown[] = [];
    const created: Array<{ identity: string; context: unknown }> = [];
    dispatcher.registerAgentCustomizer({
      async customizeBuild(context, _spec, draft) {
        buildContexts.push(context);
        draft.registerTool("echo", (args) => ({ echo: args.input }), "echoes input");
      },
      async afterCreate(identity, _sessionId, context) {
        created.push({ identity, context });
      },
    });
    const build = (scopeId: string, mobId?: string) =>
      dispatcher.handleCallback("callback/agent_customizer/customize_build", {
        scope_id: scopeId,
        context: CONTEXT,
        spec: SPEC,
        draft: {},
        ...(mobId === undefined ? {} : { mob_id: mobId }),
      });
    const serves = async (scopeId: string): Promise<boolean> => {
      try {
        const result = await dispatcher.handleCallback("callback/call_tool", {
          scope_id: scopeId,
          tool: "echo",
          arguments: { input: "hi" },
        });
        return JSON.stringify(result) === JSON.stringify({ content: { echo: "hi" } });
      } catch {
        return false;
      }
    };
    return { dispatcher, build, serves, buildContexts, created };
  }

  it("two mobs with the same member name keep both scopes live", async () => {
    const { build, serves } = await makeDispatcher();
    await build("customize-agent:alpha-1", "mob-a");
    await build("customize-agent:alpha-2", "mob-b");

    // The second mob's build of agent:alpha did not release the first's.
    assert.deepEqual(
      [await serves("customize-agent:alpha-1"), await serves("customize-agent:alpha-2")],
      [true, true],
    );
  });

  it("a rebuild releases the scope of its own mob only", async () => {
    const { build, serves } = await makeDispatcher();
    await build("customize-agent:alpha-1", "mob-a");
    await build("customize-agent:alpha-2", "mob-b");
    await build("customize-agent:alpha-3", "mob-a");

    // Newest wins inside mob-a; mob-b's member keeps its tools.
    assert.deepEqual(
      [
        await serves("customize-agent:alpha-1"),
        await serves("customize-agent:alpha-2"),
        await serves("customize-agent:alpha-3"),
      ],
      [false, true, true],
    );
  });

  it("a build without a mob is the host's own mob", async () => {
    const { build, serves } = await makeDispatcher();
    // A host that sends no mob_id (an earlier one) and a named mob are two
    // members: neither build releases the other's tools.
    await build("customize-agent:alpha-1");
    await build("customize-agent:alpha-2", "mob-b");
    assert.deepEqual(
      [await serves("customize-agent:alpha-1"), await serves("customize-agent:alpha-2")],
      [true, true],
    );

    // And without a mob the newest build of an identity still wins.
    await build("customize-agent:alpha-3");
    assert.deepEqual(
      [
        await serves("customize-agent:alpha-1"),
        await serves("customize-agent:alpha-2"),
        await serves("customize-agent:alpha-3"),
      ],
      [false, true, true],
    );
  });

  it("customize_build tells the customizer the member's mob", async () => {
    const { build, buildContexts } = await makeDispatcher();
    await build("customize-agent:alpha-1", "mob-b");
    await build("customize-agent:alpha-2");

    assert.deepEqual(buildContexts, [
      { identity: "agent:alpha", activePeers: [], managedEdges: [], mobId: "mob-b" },
      // No mob_id from the host, which is its own mob: the context is whole
      // as it was before a host could name one, with no key for a mob.
      { identity: "agent:alpha", activePeers: [], managedEdges: [] },
    ]);
  });

  for (const notText of [{ id: "mob-b" }, 7, true, ["mob-b"]]) {
    it(`a mob_id that is not text is refused: ${JSON.stringify(notText)}`, async () => {
      // A host that names mobs names them by text. Reading another value as
      // "no mob" would hand the customizer the host's own mob for a member of
      // another one, so each of the three callbacks is refused instead.
      const { ContractMismatchError } = await import("../src/errors.js");
      const { CallbackDispatcher } = await import("../src/agent-builder.js");
      const { dispatcher, build, serves, buildContexts, created } = await makeDispatcher();
      await build("customize-agent:alpha-1");
      await assert.rejects(
        dispatcher.handleCallback("callback/agent_customizer/customize_build", {
          scope_id: "customize-agent:alpha-2",
          context: CONTEXT,
          spec: SPEC,
          draft: {},
          mob_id: notText,
        }),
        ContractMismatchError,
      );
      await assert.rejects(
        dispatcher.handleCallback("callback/agent_customizer/after_create", {
          identity: "agent:alpha",
          session_id: "session-1",
          mob_id: notText,
        }),
        ContractMismatchError,
      );
      const asked: unknown[] = [];
      const topology = new CallbackDispatcher();
      topology.registerTopologyProvider({
        async computeEdges(_targetIdentities, context) {
          asked.push(context);
          return [];
        },
      });
      await assert.rejects(
        topology.handleCallback("callback/topology_provider/compute_edges", {
          target_identities: ["agent:alpha"],
          context: {},
          mob_id: notText,
        }),
        ContractMismatchError,
      );

      // Neither the customizer nor the provider was asked, and the refused
      // build registered and released nothing.
      assert.deepEqual([buildContexts.length, created, asked], [1, [], []]);
      assert.deepEqual(
        [await serves("customize-agent:alpha-1"), await serves("customize-agent:alpha-2")],
        [true, false],
      );
    });
  }

  it("a null mob_id is the host's own mob", async () => {
    const { dispatcher, build, serves, buildContexts } = await makeDispatcher();
    await dispatcher.handleCallback("callback/agent_customizer/customize_build", {
      scope_id: "customize-agent:alpha-1",
      context: CONTEXT,
      spec: SPEC,
      draft: {},
      mob_id: null,
    });
    await build("customize-agent:alpha-2");

    // Null is absent: no mob is told, and one member with the build that
    // names no mob.
    assert.deepEqual(
      buildContexts.map((context) => "mobId" in (context as object)),
      [false, false],
    );
    assert.deepEqual(
      [await serves("customize-agent:alpha-1"), await serves("customize-agent:alpha-2")],
      [false, true],
    );
  });

  it("after_create tells the customizer the member's mob", async () => {
    const { dispatcher, created } = await makeDispatcher();
    const params = { identity: "agent:alpha", session_id: "session-1", model: "gpt-5.5" };
    await dispatcher.handleCallback("callback/agent_customizer/after_create", {
      ...params,
      mob_id: "mob-b",
    });
    await dispatcher.handleCallback("callback/agent_customizer/after_create", params);

    const context = { model: "gpt-5.5", labels: {}, systemPrompt: null };
    assert.deepEqual(created, [
      { identity: "agent:alpha", context: { ...context, mobId: "mob-b" } },
      // No mob_id from the host: the context is whole as it was, with no key
      // for a mob.
      { identity: "agent:alpha", context },
    ]);
  });

  it("compute_edges tells the provider the mob", async () => {
    const { CallbackDispatcher } = await import("../src/agent-builder.js");
    const dispatcher = new CallbackDispatcher();
    const seen: Array<[string[], unknown]> = [];
    dispatcher.registerTopologyProvider({
      async computeEdges(targetIdentities, context) {
        seen.push([targetIdentities, context]);
        return [];
      },
    });
    const roster = { roster: [{ identity: "agent:alpha", profile: "default" }] };
    const params = { target_identities: ["agent:alpha"], context: roster };

    await dispatcher.handleCallback("callback/topology_provider/compute_edges", {
      ...params,
      mob_id: "mob-b",
    });
    await dispatcher.handleCallback("callback/topology_provider/compute_edges", params);

    assert.deepEqual(seen, [
      // The provider's context names the mob; what the host sent as context
      // is otherwise as it was.
      [["agent:alpha"], { ...roster, mob_id: "mob-b" }],
      // No mob_id from the host: the context is what it was before.
      [["agent:alpha"], roster],
    ]);
    assert.deepEqual(params.context, roster);

    // A context that is not an object has no place for the mob: it is handed
    // on as it came.
    for (const notAnObject of [["agent:alpha"], "roster"]) {
      await dispatcher.handleCallback("callback/topology_provider/compute_edges", {
        ...params,
        context: notAnObject,
        mob_id: "mob-b",
      });
      assert.deepEqual(seen.at(-1), [["agent:alpha"], notAnObject]);
    }
  });
});
