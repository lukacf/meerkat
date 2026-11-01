import { describe, expect, it } from "vitest";

import { createActivityLedger } from "../../lib/activity-ledger";
import type { ConsoleFrame } from "../../types";
import { feedHistoryToLedger } from "./history-frames";

const NOW = Date.parse("2026-11-01T12:00:00Z");
const RUN = "01900000-0000-7000-8000-00000000d0a1";
const TURN = "01900000-0000-7000-8000-00000000d0b1";

/** One agent's history page: a turn with a tool call. Each call builds new
 * objects, the way a page that loads again arrives. */
function page(identity: string | undefined = "billing"): ConsoleFrame[] {
  const at = (minutesAgo: number) => NOW - minutesAgo * 60_000;
  const base = { sourceKind: "console_event" as const, ...(identity ? { identity } : {}) };
  return [
    { ...base, id: "h-1", event: "user_input", timestampMs: at(30), interactionId: TURN, data: { content: "Refund order 77310.", origin_kind: "operator" } },
    { ...base, id: "h-2", event: "run_started", timestampMs: at(30), interactionId: TURN, runId: RUN, data: { run_id: RUN } },
    { ...base, id: "h-3", event: "tool_call_requested", timestampMs: at(29), interactionId: TURN, runId: RUN, data: { name: "refund", id: "call-1", args: { order: "77310" } } },
    { ...base, id: "h-4", event: "tool_execution_completed", timestampMs: at(29), interactionId: TURN, runId: RUN, data: { name: "refund", id: "call-1", result: "Refunded." } },
    { ...base, id: "h-5", event: "interaction_complete", timestampMs: at(28), interactionId: TURN, runId: RUN, data: { result: "Refunded 1,240 EUR." } },
  ] as ConsoleFrame[];
}

describe("history in the activity ledger", () => {
  it("lands once, however often its page loads", () => {
    const ledger = createActivityLedger({ now: () => NOW, notifyMs: 0 });
    expect(feedHistoryToLedger(ledger, "billing", page())).toBe(5);
    const totals = ledger.totals("billing");
    expect(totals.turnsStarted).toBe(1);
    expect(totals.turnsFinished).toBe(1);
    expect(totals.toolCalls).toBe(1);
    const events = ledger.series("billing", "events", { bins: 24, binMs: 3_600_000, endMs: NOW }).reduce((sum, n) => sum + n, 0);
    expect(events).toBe(totals.events);
    expect(ledger.runSpans("billing").map((run) => [run.runId, run.outcome])).toEqual([[RUN, "ok"]]);

    // The page again (a refresh, or the agent's chat opening): nothing goes in twice.
    expect(feedHistoryToLedger(ledger, "billing", page())).toBe(0);
    expect(ledger.totals("billing")).toEqual(totals);
    expect(ledger.runSpans("billing").length).toBe(1);

    // A newer version of a frame goes in; the ledger still counts its event once.
    const [, , , , done] = page();
    expect(feedHistoryToLedger(ledger, "billing", [{ ...done, frameVersion: 2 }])).toBe(1);
    expect(ledger.totals("billing")).toEqual(totals);
  });

  it("gives a frame without an identity the log's, and keeps each ledger's keys apart", () => {
    const ledger = createActivityLedger({ now: () => NOW, notifyMs: 0 });
    expect(feedHistoryToLedger(ledger, "billing", page(undefined))).toBe(5);
    expect(ledger.totals("billing").turnsStarted).toBe(1);
    // A new ledger (another console scope) takes the same history afresh.
    const other = createActivityLedger({ now: () => NOW, notifyMs: 0 });
    expect(feedHistoryToLedger(other, "billing", page())).toBe(5);
    expect(other.totals("billing").turnsStarted).toBe(1);
  });
});
