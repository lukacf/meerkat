import { describe, expect, it } from "vitest";

import { createActivityLedger } from "../../lib/activity-ledger";
import type { ConsoleFrame } from "../../types";
import { buildActivityEntries } from "./activity-model";
import {
  PROBLEMS_LANE,
  QUIETER_LANE,
  buildLaneModel,
  buildStory,
  foldFacts,
  storyRows,
  todayBins,
  todayWindow,
  turnFacts,
  turnOutcomeWords,
  type StoryFold,
  type StoryTurn,
} from "./story-model";

const NOW = Date.parse("2026-11-01T10:30:00Z");
let serial = 0;
function frame(identity: string, event: string, minutesAgo: number, data: Record<string, unknown> = {}, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  serial += 1;
  return { id: `s-${serial}`, event, identity, timestampMs: NOW - minutesAgo * 60_000, sourceKind: "console_event", data, ...extra };
}
function turn(identity: string, run: string, startedAgo: number, endedAgo: number | null, outcome: "ok" | "failed" = "ok"): ConsoleFrame[] {
  const frames = [
    frame(identity, "user_input", startedAgo, { content: `Work for ${identity}`, origin: "console:pane-1", origin_kind: "operator" }, { interactionId: `turn-${run}` }),
    frame(identity, "run_started", startedAgo, { run_id: run }, { runId: run, interactionId: `turn-${run}` }),
  ];
  if (endedAgo !== null) {
    frames.push(outcome === "ok"
      ? frame(identity, "interaction_complete", endedAgo, { result: "Done." }, { runId: run, interactionId: `turn-${run}` })
      : frame(identity, "interaction_failed", endedAgo, { error_report: { class: "llm", message: "The provider timed out." } }, { runId: run, interactionId: `turn-${run}` }));
  }
  return frames;
}
const nameOf = (key: string) => key.charAt(0).toUpperCase() + key.slice(1);

describe("Today's windows", () => {
  it("covers the day from local midnight, or the last 3 hours or hour, in about 12 to 24 bins", () => {
    const day = todayWindow("day", NOW);
    expect(new Date(day.start).getHours()).toBe(0);
    expect(new Date(day.start).getMinutes()).toBe(0);
    expect(day.end).toBe(NOW);
    expect(todayWindow("1h", NOW)).toEqual({ start: NOW - 3_600_000, end: NOW });
    const hour = todayBins("1h", todayWindow("1h", NOW));
    expect(hour.binMs).toBe(5 * 60_000);
    expect(hour.bins).toBe(12);
    const threeHours = todayBins("3h", todayWindow("3h", NOW));
    expect(threeHours.binMs).toBe(15 * 60_000);
    expect(todayBins("day", { start: NOW - 80 * 60_000, end: NOW }).binMs).toBe(5 * 60_000);
  });
});

describe("span pairing", () => {
  it("draws a turn from its start to its end, open turns as live, failed turns as failed, with the tool calls inside", () => {
    const ledger = createActivityLedger({ now: () => NOW, notifyMs: 0 });
    const frames = [
      ...turn("billing", "run-done", 30, 25),
      ...turn("billing", "run-failed", 20, 19, "failed"),
      ...turn("orders", "run-open", 10, null),
      frame("orders", "tool_call_requested", 9, { name: "pull_export", id: "call-1" }, { runId: "run-open", interactionId: "turn-run-open" }),
      frame("orders", "tool_execution_started", 9, { name: "pull_export", id: "call-1" }, { runId: "run-open" }),
    ];
    ledger.ingestMany(frames);
    const window = todayWindow("1h", NOW);
    const model = buildLaneModel({
      entries: buildActivityEntries(frames),
      runs: ledger.runSpans(null, { sinceMs: window.start, untilMs: window.end }),
      tools: ledger.toolSpans(null, { sinceMs: window.start, untilMs: window.end }),
      window,
      nameOf,
      isLive: (agent) => ledger.hasOpenRun(agent),
    });
    const byRun = new Map(model.spans.map((span) => [span.runId, span]));
    expect(byRun.get("run-done")?.state).toBe("done");
    expect(byRun.get("run-done")?.start).toBe(NOW - 30 * 60_000);
    expect(byRun.get("run-done")?.end).toBe(NOW - 25 * 60_000);
    expect(byRun.get("run-failed")?.state).toBe("failed");
    expect(byRun.get("run-open")?.state).toBe("live");
    expect(byRun.get("run-open")?.end).toBeUndefined();
    expect(byRun.get("run-open")?.ticks).toEqual([NOW - 9 * 60_000]);
    expect(model.lanes.find((lane) => lane.id === "orders")?.live).toBe(true);
    // The failure lines up on the problems lane.
    expect(model.problems.map((mark) => [mark.lane, mark.shape])).toEqual([[PROBLEMS_LANE, "diamond"]]);
  });

  it("keeps the busiest twelve lanes and folds the rest into one", () => {
    const frames = Array.from({ length: 15 }, (_, index) => frame(`agent${String(index).padStart(2, "0")}`, "member_ready", 5 + index));
    // agent00 is the busiest.
    frames.push(frame("agent00", "member_ready", 3), frame("agent00", "member_ready", 2));
    const window = todayWindow("1h", NOW);
    const model = buildLaneModel({ entries: buildActivityEntries(frames), runs: [], tools: [], window, nameOf });
    expect(model.lanes.length).toBe(13);
    expect(model.lanes[0].id).toBe("agent00");
    const folded = model.lanes[12];
    expect(folded.id).toBe(QUIETER_LANE);
    expect(folded.label).toBe("+3 quieter agents");
    expect(model.quieter.length).toBe(3);
  });

  it("connects a message's sender and recipient lanes", () => {
    const frames = [
      frame("intake", "tool_call_requested", 8, { name: "send_message", id: "c-1", args: { to: "billing", body: "Ticket 1" } }, { interactionId: "t-1" }),
      frame("billing", "member_ready", 7),
    ];
    const window = todayWindow("1h", NOW);
    const model = buildLaneModel({ entries: buildActivityEntries(frames), runs: [], tools: [], window, nameOf });
    expect(model.links.map((link) => [link.from, link.to])).toEqual([["intake", "billing"]]);
  });
});

describe("the story", () => {
  it("keeps a turn as one row: who asked, what it did, how it ended", () => {
    const frames = [
      ...turn("intake", "run-1", 12, 10),
      frame("intake", "tool_call_requested", 11, { name: "send_message", id: "m-1", args: { to: "billing", body: "Ticket 1" } }, { interactionId: "turn-run-1" }),
      frame("intake", "tool_call_requested", 11, { name: "send_message", id: "m-2", args: { to: "billing", body: "Ticket 2" } }, { interactionId: "turn-run-1" }),
    ];
    const rows = storyRows(buildStory(buildActivityEntries(frames), { nameOf }));
    expect(rows.length).toBe(1);
    const row = rows[0] as StoryTurn;
    expect(row.kind).toBe("turn");
    expect(row.request?.frame.event).toBe("user_input");
    expect(row.outcome?.frame.event).toBe("interaction_complete");
    expect(turnFacts(row, nameOf)).toEqual(["messaged Billing 2 times"]);
    expect(turnOutcomeWords(row)?.text).toBe("replied");
  });

  it("folds a run of one agent's quiet rows, and never folds a problem away", () => {
    const frames = [
      frame("orders", "tool_call_requested", 40, { name: "pull_export", id: "a" }),
      frame("orders", "tool_call_requested", 39, { name: "pull_export", id: "b" }),
      frame("orders", "tool_call_requested", 38, { name: "pull_export", id: "c" }),
      frame("orders", "tool_execution_timed_out", 37, { name: "pull_export", id: "d", timeout_ms: 4000 }),
      frame("orders", "tool_call_requested", 36, { name: "pull_export", id: "e" }),
    ];
    const rows = storyRows(buildStory(buildActivityEntries(frames), { nameOf }));
    expect(rows.map((row) => row.kind)).toEqual(["event", "event", "fold"]);
    expect(rows[1].severity).toBe("warn");
    const fold = rows[2] as StoryFold;
    expect(fold.rows.length).toBe(3);
    expect(foldFacts(fold, nameOf)).toEqual(["3 tool calls"]);
  });

  it("filters to problems and searches the words", () => {
    const frames = [
      ...turn("shipping", "run-x", 15, 14, "failed"),
      ...turn("billing", "run-y", 13, 12),
      frame("insights", "budget_warning", 11, { budget_type: "tokens", percent: 0.85 }),
    ];
    const entries = buildActivityEntries(frames);
    const problems = storyRows(buildStory(entries, { filter: "problems", nameOf }));
    expect(problems.map((row) => row.severity).sort()).toEqual(["crit", "warn"]);
    const found = storyRows(buildStory(entries, { query: "billing", nameOf }));
    expect(found.length).toBe(1);
    expect(found[0].kind === "turn" && found[0].actor).toBe("billing");
  });
});
