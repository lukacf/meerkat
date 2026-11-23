/**
 * Fields follow Core's AgentEvent/Usage/TurnUsage on the two terminal events.
 * Meta's console_agent_event_payload preserves those usage objects.
 */
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  isRunCompleted,
  isTurnCompleted,
  parseAgentEvent,
  parseAgentEventFromSse,
  parseMobEventFromSse,
} from "../dist/index.js";
import type { Usage, ProviderTokenAccounting } from "../dist/index.js";

type TerminalKind = "turn_completed" | "run_completed";

function terminal(kind: TerminalKind, fields: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    type: kind,
    ...(kind === "run_completed"
      ? { session_id: "session-1", result: "done", structured_output: { ok: true } }
      : { stop_reason: "end_turn" }),
    ...fields,
  };
}

const MALFORMED: unknown[] = [
  {}, [], "unknown", false,
  { input_tokens: 1 },
  { input_tokens: 1, output_tokens: "2" },
  { input_tokens: true, output_tokens: 2 },
  { input_tokens: -1, output_tokens: 2 },
  { input_tokens: 1, output_tokens: 2, cache_read_tokens: "0" },
  { input_tokens: 1, output_tokens: 2, accounting: {} },
];

/** Runs `run` with `console.warn` captured, and restores it afterwards. */
function withWarnings<T>(run: () => T): { value: T; said: string[] } {
  const said: string[] = [];
  const original = console.warn;
  console.warn = (...args: unknown[]) => {
    said.push(args.map(String).join(" "));
  };
  try {
    return { value: run(), said };
  } finally {
    console.warn = original;
  }
}

function usageOf(raw: Record<string, unknown>): Usage | undefined {
  const event = parseAgentEvent(raw);
  assert.ok(isRunCompleted(event) || isTurnCompleted(event));
  return event.usage;
}

describe("terminal usage payload projection", () => {
  for (const kind of ["turn_completed", "run_completed"] as const) {
    it(`${kind} exposes typed supplied counters`, () => {
      const usage = usageOf(terminal(kind, { usage: {
        input_tokens: 600, output_tokens: 40, cache_creation_tokens: null,
        cache_read_tokens: 200, reasoning_tokens: 10,
      } }));
      assert.ok(usage, "the terminal payload's usage must not be discarded");
      assert.equal(usage.inputTokens, 600);
      assert.equal(usage.outputTokens, 40);
      assert.equal(usage.cacheCreationTokens, undefined);
      assert.equal(usage.cacheReadTokens, 200);
      assert.equal(usage.reasoningTokens, 10);
      assert.equal(usage.accounting, undefined);
      assert.ok(!("totalTokens" in usage), "these events carry no total_tokens field");
    });

    for (const fields of [{}, { usage: null }]) {
      it(`${kind} keeps ${"usage" in fields ? "null" : "absent"} usage unavailable`, () => {
        assert.equal(usageOf(terminal(kind, fields)), undefined);
      });
    }

    it(`${kind} preserves explicit zeros without supplying optional counters`, () => {
      const usage = usageOf(terminal(kind, { usage: { input_tokens: 0, output_tokens: 0 } }));
      assert.ok(usage);
      assert.equal(usage.inputTokens, 0);
      assert.equal(usage.outputTokens, 0);
      assert.equal(usage.cacheCreationTokens, undefined);
      assert.equal(usage.cacheReadTokens, undefined);
      assert.equal(usage.reasoningTokens, undefined);
      assert.equal(usage.accounting, undefined);
    });

    it(`${kind} preserves explicitly zero cache and reasoning counters`, () => {
      const usage = usageOf(terminal(kind, { usage: {
        input_tokens: 0, output_tokens: 0, cache_creation_tokens: 0,
        cache_read_tokens: 0, reasoning_tokens: 0,
      } }));
      assert.ok(usage);
      assert.equal(usage.cacheCreationTokens, 0);
      assert.equal(usage.cacheReadTokens, 0);
      assert.equal(usage.reasoningTokens, 0);
    });

    for (const wrapper of ["agent", "mob"] as const) {
      it(`${kind} usage survives the ${wrapper} SSE wrapper`, () => {
        const raw = terminal(kind, { usage: { input_tokens: 600, output_tokens: 40 } });
        const envelope = wrapper === "agent"
          ? parseAgentEventFromSse({ id: null, event: kind, data: JSON.stringify(raw) })
          : parseMobEventFromSse({ id: null, event: kind, data: JSON.stringify({
              member_id: "agent-1", timestamp_ms: 7, payload: raw,
            }) });
        assert.ok(isRunCompleted(envelope.event) || isTurnCompleted(envelope.event));
        assert.equal(envelope.event.usage?.inputTokens, 600);
        assert.equal(envelope.event.usage?.outputTokens, 40);
      });
    }
  }

  it("retains per-call accounting without normalizing raw input", () => {
    const usage = usageOf(terminal("turn_completed", { usage: {
      input_tokens: 300, output_tokens: 40,
      cache_creation_tokens: 100, cache_read_tokens: 200, reasoning_tokens: 10,
      accounting: {
        provider: "anthropic", model: "fixture-model", presented_tokens: 600,
        convention: "anthropic_disjoint_input_components",
        aggregation: "sum_disjoint_provider_components",
      },
    } }));
    assert.ok(usage, "the measured turn's usage must not be discarded");
    const accounting: ProviderTokenAccounting | undefined = usage.accounting;
    assert.equal(usage.inputTokens, 300);
    assert.deepEqual(accounting, {
      provider: "anthropic", model: "fixture-model", presentedTokens: 600,
      convention: "anthropic_disjoint_input_components",
      aggregation: "sum_disjoint_provider_components",
    });
  });

  for (const kind of ["turn_completed", "run_completed"] as const) {
    for (const raw of MALFORMED) {
      it(`${kind}: malformed usage ${JSON.stringify(raw)} makes no counters and loses no event`, () => {
        const { value: event, said } = withWarnings(
          () => parseAgentEvent(terminal(kind, { usage: raw })),
        );
        assert.ok(isRunCompleted(event) || isTurnCompleted(event));
        assert.equal(event.usage, undefined);
        assert.ok(!("usage" in event));
        if (isRunCompleted(event)) {
          assert.equal(event.result, "done");
          assert.deepEqual(event.structuredOutput, { ok: true });
        } else {
          assert.equal(event.stopReason, "end_turn");
        }
        assert.equal(said.length, 1, "the usage left out is said once, not silently");
        const line = said.join("\n");
        assert.ok(line.includes(kind) && line.includes("usage"));
      });
    }
    for (const wrapper of ["agent", "mob"] as const) {
      it(`${kind}: a malformed usage does not end the ${wrapper} stream wrapper`, () => {
        const raw = terminal(kind, { usage: { input_tokens: 1 } });
        const { value: envelope, said } = withWarnings(() => wrapper === "agent"
          ? parseAgentEventFromSse({ id: null, event: kind, data: JSON.stringify(raw) })
          : parseMobEventFromSse({ id: null, event: kind, data: JSON.stringify({
              member_id: "agent-1", timestamp_ms: 7, payload: raw,
            }) }));
        assert.ok(isRunCompleted(envelope.event) || isTurnCompleted(envelope.event));
        assert.equal(envelope.event.usage, undefined);
        assert.equal(said.length, 1);
      });
    }
  }

  it("a fitting or missing usage says nothing", () => {
    const { said } = withWarnings(() => {
      for (const kind of ["turn_completed", "run_completed"] as const) {
        for (const fields of [{}, { usage: null }, { usage: { input_tokens: 0, output_tokens: 0 } }]) {
          parseAgentEvent(terminal(kind, fields));
        }
      }
    });
    assert.deepEqual(said, []);
  });

  it("preserves structured output, stop reason, and unrelated events", () => {
    const run = parseAgentEvent(terminal("run_completed"));
    assert.ok(isRunCompleted(run));
    assert.deepEqual(run.structuredOutput, { ok: true });
    assert.equal(run.result, "done");
    const turn = parseAgentEvent(terminal("turn_completed"));
    assert.ok(isTurnCompleted(turn));
    assert.equal(turn.stopReason, "end_turn");
    assert.deepEqual(parseAgentEvent({ type: "text_delta", delta: "hello" }), {
      type: "text_delta", delta: "hello",
    });
  });
});
