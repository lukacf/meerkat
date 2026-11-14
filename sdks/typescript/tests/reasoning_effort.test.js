/**
 * The provider-neutral `reasoningEffort` option.
 *
 * `session/create` takes a level; `turn/start` and `mob/turn_start` take its
 * tri-state (a level, `{ action: "clear" }`, absent). An older server ignores
 * the parameter on `session/create` and refuses it on a turn, so the SDK
 * refuses to send it below the first contract that carries the option
 * (`REASONING_EFFORT_MIN_CONTRACT`). A level the model or route does not
 * accept is refused with `error.data.kind` `"reasoning_effort_refused"`
 * (`SessionError` in crates/meerkat-core/src/service/mod.rs), which
 * `reasoningEffortRefusal` reads back.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import {
  CapabilityUnavailableError,
  DeferredSession,
  MeerkatClient,
  MeerkatError,
  REASONING_EFFORT_LEVELS,
  REASONING_EFFORT_REFUSAL_REASONS,
  REASONING_EFFORT_REFUSED_KIND,
  Session,
  parseCoreEvent,
  parseReasoningEffortRefusal,
  parseReasoningEffortReport,
  parseReasoningEffortResolution,
  reasoningEffortRefusal,
  reasoningEffortReport,
} from "../dist/index.js";
import { REASONING_EFFORT_MIN_CONTRACT } from "../dist/client.js";

/** The first server contract that carries the option: the SDK's own gate. */
const CARRIER_CONTRACT = REASONING_EFFORT_MIN_CONTRACT.join(".");

/** The contract one release below the gate: a server without the option. */
const OLDER_CONTRACT = (([major, minor, patch]) => {
  if (patch > 0) return [major, minor, patch - 1];
  return minor > 0 ? [major, minor - 1, 0] : [major - 1, 0, 0];
})(REASONING_EFFORT_MIN_CONTRACT).join(".");

/** The text of the SDK's refusal to send the option to OLDER_CONTRACT. */
const OLDER_SERVER_REFUSAL =
  `reasoningEffort needs a server contract >= ${CARRIER_CONTRACT}; `
  + `this server reports ${OLDER_CONTRACT}`;

const SESSION_ID = "019a2b3c-0000-7000-8000-000000000001";

const RUN_RESULT = {
  session_id: SESSION_ID,
  text: "ok",
  turns: 1,
  tool_calls: 0,
  usage: { input_tokens: 1, output_tokens: 1 },
};

/** A client whose requests and written streaming requests are recorded. */
function recordingClient(contractVersion = CARRIER_CONTRACT) {
  const client = new MeerkatClient();
  client._serverContractVersion = contractVersion;
  const sent = [];
  client.request = async (method, params) => {
    sent.push({ method, params });
    return structuredClone(RUN_RESULT);
  };
  client.process = {
    stdin: {
      write: (data) => {
        const { method, params } = JSON.parse(data);
        sent.push({ method, params });
      },
    },
  };
  client.registerRequest = async () => structuredClone(RUN_RESULT);
  return { client, sent };
}

function session(client) {
  return new Session(client, {
    sessionId: SESSION_ID,
    text: "ready",
    turns: 0,
    toolCalls: 0,
    usage: { inputTokens: 0, outputTokens: 0 },
  });
}

describe("reasoningEffort on requests", () => {
  it("the level vocabulary is the seven levels in one spelling", () => {
    assert.deepEqual(
      [...REASONING_EFFORT_LEVELS],
      ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
    );
    assert.equal(REASONING_EFFORT_LEVELS.includes("x_high"), false);
  });

  it("every create path sends the level", async () => {
    const { client, sent } = recordingClient();

    await client.createSession("hello", { reasoningEffort: "high" });
    await client.createDeferredSession("hello", { reasoningEffort: "xhigh" });
    client.createSessionStreaming("hello", { reasoningEffort: "low" });
    await client.createSession("hello");

    assert.deepEqual(
      sent.map(({ method }) => method),
      ["session/create", "session/create", "session/create", "session/create"],
    );
    assert.equal(sent[0].params.reasoning_effort, "high");
    assert.equal(sent[1].params.reasoning_effort, "xhigh");
    assert.equal(sent[2].params.reasoning_effort, "low");
    assert.equal("reasoning_effort" in sent[3].params, false);
  });

  for (const [label, reasoningEffort] of [
    ["a level", "low"],
    ["an explicit set", { action: "set", value: "max" }],
    ["a clear", { action: "clear" }],
  ]) {
    it(`turn, stream, deferred startTurn and mobTurnStart send ${label}`, async () => {
      const { client, sent } = recordingClient();

      await session(client).turn("next", { reasoningEffort });
      session(client).stream("next", { reasoningEffort });
      await new DeferredSession(client, SESSION_ID).startTurn("first", { reasoningEffort });
      await client.mobTurnStart("mob-1", "worker", "next", { reasoningEffort });

      assert.deepEqual(
        sent.map(({ method }) => method),
        ["turn/start", "turn/start", "turn/start", "mob/turn_start"],
      );
      for (const { params } of sent) {
        assert.deepEqual(params.reasoning_effort, reasoningEffort);
      }
    });
  }

  it("a turn that names no level sends none", async () => {
    const { client, sent } = recordingClient();

    await session(client).turn("next", { providerParams: { temperature: 0.2 } });
    await client.mobTurnStart("mob-1", "worker", "next");

    for (const { params } of sent) {
      assert.equal("reasoning_effort" in params, false);
    }
  });

  // A server one release below the gate has no such parameter: its
  // session/create ignores it and
  // the session would run with no level; its turn/start refuses it.
  const refusedByOlderServer = (error) =>
    error instanceof CapabilityUnavailableError
    && error.code === "CAPABILITY_UNAVAILABLE"
    && error.message.includes(OLDER_SERVER_REFUSAL);

  it("the SDK refuses the field for an older server contract", async () => {
    const { client, sent } = recordingClient(OLDER_CONTRACT);
    const live = session(client);
    const level = { reasoningEffort: "high" };
    const clear = { reasoningEffort: { action: "clear" } };

    await assert.rejects(client.createSession("hello", level), refusedByOlderServer);
    await assert.rejects(client.createDeferredSession("hello", level), refusedByOlderServer);
    assert.throws(() => client.createSessionStreaming("hello", level), refusedByOlderServer);
    await assert.rejects(live.turn("next", level), refusedByOlderServer);
    await assert.rejects(live.turn("next", clear), refusedByOlderServer);
    assert.throws(() => live.stream("next", level), refusedByOlderServer);
    await assert.rejects(
      new DeferredSession(client, SESSION_ID).startTurn("first", level),
      refusedByOlderServer,
    );
    await assert.rejects(
      client.mobTurnStart("mob-1", "worker", "next", level),
      refusedByOlderServer,
    );
    assert.deepEqual(sent, [], "nothing is written to a server that would drop the level");

    // A server that reports no contract at all is refused too.
    const unknown = recordingClient("");
    await assert.rejects(
      unknown.client.createSession("hello", level),
      /this server reports none/,
    );
    assert.deepEqual(unknown.sent, []);

    // Calls that name no level are unaffected.
    await client.createSession("hello");
    await live.turn("next");
    assert.deepEqual(
      sent.map(({ method }) => method),
      ["session/create", "turn/start"],
    );
  });

  it("the gate is the first contract that carries the option", async () => {
    // A server one release below the gate does not have the option and does
    // not get the field.
    const before = recordingClient(OLDER_CONTRACT);
    const refused = (error) =>
      error instanceof CapabilityUnavailableError
      && error.message.includes(OLDER_SERVER_REFUSAL);
    const level = { reasoningEffort: "high" };
    await assert.rejects(before.client.createSession("hello", level), refused);
    await assert.rejects(session(before.client).turn("next", level), refused);
    await assert.rejects(
      before.client.mobTurnStart("mob-1", "worker", "next", level),
      refused,
    );
    assert.deepEqual(before.sent, [], "nothing is written to a server below the gate");

    // A server at the gate, the first contract that carries it, does.
    const carrier = recordingClient(CARRIER_CONTRACT);
    await carrier.client.createSession("hello", level);
    await session(carrier.client).turn("next", { reasoningEffort: "low" });
    assert.deepEqual(
      carrier.sent.map(({ method, params }) => [method, params.reasoning_effort]),
      [["session/create", "high"], ["turn/start", "low"]],
    );
  });
});

// The refusal's data for a level the model does not accept.
const UNSUPPORTED = {
  kind: "reasoning_effort_refused",
  retryable: false,
  reasoning_effort_refusal: {
    requested: "max",
    provider: "anthropic",
    model: "claude-opus-4-5",
    route: { backend: "anthropic_api", api: "messages" },
    reason: "unsupported_level",
    supported: ["low", "medium", "high"],
    facts: "catalog",
  },
};

const refused = (data) => new MeerkatError("-32602", "reasoning effort refused", data);

const withRefusal = (changes) => ({
  ...UNSUPPORTED,
  reasoning_effort_refusal: { ...UNSUPPORTED.reasoning_effort_refusal, ...changes },
});

describe("reasoningEffortRefusal", () => {
  it("reads the typed refusal from the error data", () => {
    assert.equal(REASONING_EFFORT_REFUSED_KIND, "reasoning_effort_refused");
    assert.deepEqual(reasoningEffortRefusal(refused(UNSUPPORTED)), {
      requested: "max",
      provider: "anthropic",
      model: "claude-opus-4-5",
      route: { backend: "anthropic_api", api: "messages" },
      reason: "unsupported_level",
      supported: ["low", "medium", "high"],
      facts: "catalog",
      transient: false,
      retryable: false,
    });
  });

  it("knows the eight closed reasons, of which only one is transient", () => {
    assert.deepEqual(
      [...REASONING_EFFORT_REFUSAL_REASONS],
      [
        "unsupported_level",
        "supported_levels_unknown",
        "supported_levels_unavailable",
        "conflicting_provider_effort",
        "conflicting_thinking_budget",
        "conflicting_thinking_type",
        "conflicting_reasoning_body",
        "conflicting_reasoning_mode",
      ],
    );
    for (const reason of REASONING_EFFORT_REFUSAL_REASONS) {
      const transient = reason === "supported_levels_unavailable";
      const refusal = reasoningEffortRefusal(
        refused({ ...withRefusal({ reason }), retryable: transient }),
      );
      assert.equal(refusal?.reason, reason);
      // The class is read from the reason; the server's mark agrees.
      assert.equal(refusal?.transient, transient, reason);
      assert.equal(refusal?.retryable, transient, reason);
    }

    // The class does not depend on the advisory mark being present.
    const { retryable: _mark, ...unmarked } = withRefusal({
      reason: "supported_levels_unavailable",
    });
    const refusal = reasoningEffortRefusal(refused(unmarked));
    assert.equal(refusal?.transient, true);
    assert.equal("retryable" in refusal, false);
  });

  it("unknown and unavailable levels have no supported set", () => {
    for (const reason of ["supported_levels_unknown", "supported_levels_unavailable"]) {
      const refusal = reasoningEffortRefusal(
        refused(withRefusal({ reason, supported: null, facts: null })),
      );
      assert.equal(refusal?.supported, null);
      assert.equal(refusal?.facts, null);
    }
    // A route or model that takes no level states the empty set.
    assert.deepEqual(
      reasoningEffortRefusal(refused(withRefusal({ supported: [] })))?.supported,
      [],
    );
  });

  it("a refusal that names no route parses", () => {
    const noRoute = withRefusal({
      route: null,
      reason: "supported_levels_unknown",
      supported: null,
      facts: null,
    });
    assert.equal(reasoningEffortRefusal(refused(noRoute))?.route, null);
    // A document that leaves the key out reads the same way.
    delete noRoute.reasoning_effort_refusal.route;
    const refusal = reasoningEffortRefusal(refused(noRoute));
    assert.equal(refusal?.reason, "supported_levels_unknown");
    assert.equal(refusal?.route, null);
  });

  it("a conflict names the setting and its level", () => {
    const refusal = reasoningEffortRefusal(
      refused(
        withRefusal({
          reason: "conflicting_provider_effort",
          conflicting_param: "provider_params.provider_tag.reasoning_effort",
          conflicting_level: "low",
        }),
      ),
    );
    assert.equal(refusal?.conflictingParam, "provider_params.provider_tag.reasoning_effort");
    assert.equal(refusal?.conflictingLevel, "low");
  });

  it("advisory route and facts values are kept as sent", () => {
    const refusal = reasoningEffortRefusal(
      refused(
        withRefusal({ route: { backend: "new_backend", api: "new_api" }, facts: "new_source" }),
      ),
    );
    assert.deepEqual(refusal?.route, { backend: "new_backend", api: "new_api" });
    assert.equal(refusal?.facts, "new_source");
  });

  it("a refusal this SDK cannot read keeps the raw data", () => {
    const unknownReason = withRefusal({ reason: "a_reason_from_a_newer_server" });
    const error = refused(unknownReason);
    assert.equal(reasoningEffortRefusal(error), undefined);
    assert.deepEqual(error.details, unknownReason);

    for (const unreadable of [
      withRefusal({ requested: "x_high" }),
      withRefusal({ supported: ["low", "x_high"] }),
      withRefusal({ route: "anthropic_api/messages" }),
      withRefusal({ model: null }),
      { ...UNSUPPORTED, reasoning_effort_refusal: null },
    ]) {
      assert.equal(reasoningEffortRefusal(refused(unreadable)), undefined);
    }

    // Another error's data is not a refusal.
    assert.equal(reasoningEffortRefusal(refused({ kind: "mob_quarantined" })), undefined);
    assert.equal(reasoningEffortRefusal(refused(undefined)), undefined);
    assert.equal(reasoningEffortRefusal(new Error("no details")), undefined);
    assert.equal(parseReasoningEffortRefusal("not an object"), undefined);
  });
});

// The `reasoning_effort` block of a run result after a model fallback switch.
const REPORT = {
  requested: "high",
  sent: [
    {
      level: "high",
      provider: "openai",
      model: "gpt-5.5",
      route: { backend: "openai_api", api: "responses" },
      fields: ["reasoning.effort"],
      facts: "catalog",
      decided_by: "request",
      requests: 3,
    },
    {
      level: "high",
      provider: "anthropic",
      model: "claude-opus-4-8",
      route: { backend: "new_backend", api: "new_api" },
      fields: ["thinking.type", "output_config.effort"],
      facts: "new_source",
      decided_by: "new_decider",
      requests: 1,
    },
  ],
};

describe("reasoning effort reports", () => {
  it("the run report lists what was sent", () => {
    const report = parseReasoningEffortReport(REPORT);
    assert.equal(report?.requested, "high");
    assert.deepEqual(report?.sent[0], {
      level: "high",
      provider: "openai",
      model: "gpt-5.5",
      route: { backend: "openai_api", api: "responses" },
      fields: ["reasoning.effort"],
      facts: "catalog",
      decidedBy: "request",
      requests: 3,
    });
    // Advisory values this SDK does not know are kept as sent.
    assert.deepEqual(report?.sent[1].route, { backend: "new_backend", api: "new_api" });
    assert.equal(report?.sent[1].facts, "new_source");
    assert.equal(report?.sent[1].decidedBy, "new_decider");

    assert.equal(parseReasoningEffortReport(undefined), undefined);
    assert.equal(parseReasoningEffortReport({ sent: [] }), undefined);
    // The accessor reads the block off a wire result.
    assert.deepEqual(reasoningEffortReport({ ...RUN_RESULT, reasoning_effort: REPORT }), report);
    assert.equal(reasoningEffortReport(RUN_RESULT), undefined);
  });

  it("a resolution is read with its requested level", () => {
    assert.deepEqual(
      parseReasoningEffortResolution({
        requested: "high",
        level: "high",
        provider: "openai",
        model: "gpt-5.5",
        route: { backend: "copilot", api: "chat_completions" },
        fields: ["reasoning_effort", "reasoning.effort"],
        facts: "account",
        decided_by: "request",
      }),
      {
        requested: "high",
        level: "high",
        provider: "openai",
        model: "gpt-5.5",
        route: { backend: "copilot", api: "chat_completions" },
        fields: ["reasoning_effort", "reasoning.effort"],
        facts: "account",
        decidedBy: "request",
      },
    );
    assert.equal(parseReasoningEffortResolution(undefined), undefined);
  });

  const RESOLUTION = {
    requested: "high",
    level: "high",
    provider: "openai",
    model: "gpt-5.5",
    route: { backend: "openai_api", api: "responses" },
    fields: ["reasoning.effort"],
    facts: "catalog",
    decided_by: "request",
  };

  it("the run result carries the report", async () => {
    const client = new MeerkatClient();
    client.request = async () => ({ ...structuredClone(RUN_RESULT), reasoning_effort: REPORT });
    const created = await client.createSession("hello");
    assert.deepEqual(created.lastResult.reasoningEffort, parseReasoningEffortReport(REPORT));
    assert.deepEqual(reasoningEffortReport(created.lastResult), parseReasoningEffortReport(REPORT));

    client.request = async () => structuredClone(RUN_RESULT);
    const plain = await client.createSession("hello");
    assert.equal("reasoningEffort" in plain.lastResult, false);
  });

  it("run_started and the per-request event are typed", () => {
    const started = parseCoreEvent({
      type: "run_started",
      session_id: "s1",
      input: { kind: "content", content: "yo" },
      reasoning_effort: RESOLUTION,
    });
    assert.deepEqual(started.reasoningEffort, parseReasoningEffortResolution(RESOLUTION));
    const plain = parseCoreEvent({
      type: "run_started",
      session_id: "s1",
      input: { kind: "content", content: "yo" },
    });
    assert.equal("reasoningEffort" in plain, false);

    const resolved = parseCoreEvent({
      type: "request_reasoning_effort_resolved",
      turn_number: 2,
      fallback_attempt: 1,
      resolution: { ...RESOLUTION, facts: "new_source", decided_by: "new_decider" },
    });
    assert.equal(resolved.type, "request_reasoning_effort_resolved");
    assert.equal(resolved.turnNumber, 2);
    assert.equal(resolved.fallbackAttempt, 1);
    assert.equal(resolved.resolution.level, "high");
    // Advisory values this SDK does not know are kept as sent.
    assert.equal(resolved.resolution.facts, "new_source");
    assert.equal(resolved.resolution.decidedBy, "new_decider");
    const bare = parseCoreEvent({
      type: "request_reasoning_effort_resolved",
      resolution: RESOLUTION,
    });
    assert.equal("turnNumber" in bare, false);
    assert.equal("fallbackAttempt" in bare, false);
  });

  const runFailed = (reason) =>
    parseCoreEvent({
      type: "run_failed",
      session_id: "s1",
      error_report: { class: "config", message: "refused", reason },
    });

  it("a refused run carries the typed refusal", () => {
    const refusal = UNSUPPORTED.reasoning_effort_refusal;
    const failed = runFailed({ reason_type: "reasoning_effort_refused", refusal });
    assert.equal(failed.errorReport.reason.reasonType, "reasoning_effort_refused");
    assert.deepEqual(failed.errorReport.reason.refusal, parseReasoningEffortRefusal(refusal));

    // A refusal this SDK cannot read is never read as another reason.
    const newer = { ...refusal, reason: "a_reason_from_a_newer_server" };
    const unread = runFailed({ reason_type: "reasoning_effort_refused", refusal: newer });
    assert.equal(unread.errorReport.reason.reasonType, "unknown");
    assert.equal(unread.errorReport.reason.rawReasonType, "reasoning_effort_refused");
    assert.deepEqual(unread.errorReport.reason.raw.refusal, newer);
  });

  for (const reason of ["reasoning_effort_unsupported", "a_newer_skip_reason"]) {
    it(`a fallback skip reason this SDK does not type is kept (${reason})`, () => {
      const raw = {
        type: "model_fallback_skipped",
        target: { model: "gpt-5.5", provider: "openai", reason },
      };
      const event = parseCoreEvent(raw);
      assert.equal(event.type, "model_fallback_skipped");
      assert.deepEqual(event.target, raw.target);
    });
  }
});
