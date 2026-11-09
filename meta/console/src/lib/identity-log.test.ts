import { describe, expect, it } from "vitest";

import {
  createIdentityLogCore,
  insertSorted,
  mergeFrameUpdate,
  pushFrame,
  sortedEvents,
  trimIdentityLogCore,
} from "./identity-log";
import type { ConsoleFrame } from "../types";

function frame(id: string, timestampMs: number, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event: "tool_execution_started", timestampMs, cursor: `console:${timestampMs}`, ...extra };
}

describe("identity log core", () => {
  it("maintains the sorted view on append and keeps existing frames first on ties", () => {
    const log = createIdentityLogCore();
    pushFrame(log, "a", frame("a", 30));
    pushFrame(log, "b", frame("b", 10));
    const sorted = sortedEvents(log);
    expect(sorted.map((f) => f.id)).toEqual(["b", "a"]);
    // Appending after the view exists inserts without a rebuild.
    pushFrame(log, "c", frame("c", 20));
    expect(log.sorted).toBe(sorted);
    expect(sorted.map((f) => f.id)).toEqual(["b", "c", "a"]);
    const tie = frame("d", 20, { cursor: undefined });
    insertSorted(sorted, tie);
    expect(sorted.map((f) => f.id)).toEqual(["b", "c", "d", "a"]);
    expect(pushFrame(log, "a", frame("a", 30))).toBe(false);
  });

  it("splices a status-only frame_updated in place without re-sorting", () => {
    const log = createIdentityLogCore();
    pushFrame(log, "t1", frame("t1", 10, { status: "pending" }));
    pushFrame(log, "t2", frame("t2", 20, { status: "pending" }));
    pushFrame(log, "t3", frame("t3", 30, { status: "pending" }));
    const sorted = sortedEvents(log);
    const before = log.version;
    const result = mergeFrameUpdate(log, { id: "t2", event: "tool_execution_completed", status: "done", frameVersion: 2 } as ConsoleFrame);
    expect(result).not.toBeNull();
    expect(result?.moved).toBe(false);
    expect(log.version).toBe(before + 1);
    // Same array instance: no rebuild happened.
    expect(log.sorted).toBe(sorted);
    expect(sorted[1]).toBe(result?.next);
    expect(sorted[1].status).toBe("done");
    expect(sorted[1].timestampMs).toBe(20);
    expect(log.events[1]).toBe(result?.next);
    // An older version is ignored.
    expect(mergeFrameUpdate(log, { id: "t2", event: "tool_execution_completed", frameVersion: 1 } as ConsoleFrame)).toBeNull();
    expect(log.version).toBe(before + 1);
  });

  it("invalidates the sorted view when frame_updated moves the frame", () => {
    const log = createIdentityLogCore();
    pushFrame(log, "t1", frame("t1", 10));
    pushFrame(log, "t2", frame("t2", 20));
    pushFrame(log, "t3", frame("t3", 30));
    const sorted = sortedEvents(log);
    const result = mergeFrameUpdate(log, { id: "t1", event: "tool_execution_started", timestampMs: 40, cursor: "console:40" } as ConsoleFrame);
    expect(result?.moved).toBe(true);
    expect(log.sorted).toBeNull();
    const rebuilt = sortedEvents(log);
    expect(rebuilt).not.toBe(sorted);
    expect(rebuilt.map((f) => f.id)).toEqual(["t2", "t3", "t1"]);
  });

  it("trims the oldest frames in transcript order and rebuilds the index", () => {
    const log = createIdentityLogCore();
    // Arrival order differs from transcript order.
    for (const [id, ts] of [["a", 50], ["b", 10], ["c", 40], ["d", 20], ["e", 30]] as const) {
      pushFrame(log, id, frame(id, ts));
    }
    const retained = trimIdentityLogCore(log, 3, (f) => f.id ?? "");
    expect(retained?.map((f) => f.id)).toEqual(["a", "c", "e"]);
    expect(sortedEvents(log).map((f) => f.id)).toEqual(["e", "c", "a"]);
    expect(log.byKey.get("a")).toBe(0);
    expect(log.byKey.get("e")).toBe(2);
    expect(log.byKey.has("b")).toBe(false);
    expect(trimIdentityLogCore(log, 3, (f) => f.id ?? "")).toBeNull();
  });
});
