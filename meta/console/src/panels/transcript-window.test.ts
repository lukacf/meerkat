import { describe, expect, it } from "vitest";

import { TURN_FIND_BAND, planTurnWindow, turnSlots, type TurnMeasurement, type TurnWindowInput } from "./transcript-window";

const turns = Array.from({ length: 60 }, (_, i) => ({ id: `t${i}` }));
const heights = turns.map((_, i) => 80 + ((i * 37) % 160));
const GAP = 14;
const TOP = 46;

function measurements(width = 800, only?: (index: number) => boolean): Map<string, TurnMeasurement> {
  const map = new Map<string, TurnMeasurement>();
  turns.forEach((turn, i) => { if (!only || only(i)) map.set(turn.id, { height: heights[i], key: `k${i}`, width }); });
  return map;
}

function input(overrides: Partial<TurnWindowInput> = {}): TurnWindowInput {
  return {
    turns,
    measurements: measurements(),
    keys: turns.map((_, i) => `k${i}`),
    width: 800,
    gap: GAP,
    top: TOP,
    scrollTop: 3_000,
    viewportHeight: 600,
    current: null,
    pinned: new Set(),
    ...overrides,
  };
}

const topOf = (index: number) => TOP + heights.slice(0, index).reduce((sum, h) => sum + h + GAP, 0);

describe("planTurnWindow", () => {
  it("mounts the turns overlapping the viewport and its overscan, plus the newest turn", () => {
    const plan = planTurnWindow(input());
    const visible = turns.map((_, i) => i).filter((i) => topOf(i) < 3_600 && topOf(i) + heights[i] > 3_000);
    for (const index of visible) expect(plan.mounted).toContain(index);
    expect(plan.mounted).toContain(turns.length - 1);
    // Overscan is bounded: nothing more than 1.5 viewports beyond either edge.
    for (const index of plan.mounted.filter((i) => i !== turns.length - 1)) {
      expect(topOf(index) + heights[index]).toBeGreaterThan(3_000 - 900 - GAP);
      expect(topOf(index)).toBeLessThan(3_600 + 900);
    }
  });

  it("keeps its window while the viewport stays inside it, and moves it near an edge", () => {
    const first = planTurnWindow(input());
    expect(planTurnWindow(input({ scrollTop: 3_100, current: first.range })).range).toEqual(first.range);
    const far = planTurnWindow(input({ scrollTop: 6_000, current: first.range }));
    expect(far.range).not.toEqual(first.range);
  });

  it("mounts every turn without a measurement at the current width and content", () => {
    const unmeasured = planTurnWindow(input({ measurements: measurements(800, (i) => i !== 3) }));
    expect(unmeasured.mounted).toContain(3);
    const resized = planTurnWindow(input({ width: 640 }));
    expect(resized.mounted).toHaveLength(turns.length);
    // A turn whose render key changed (its content, or the day it follows).
    const keys = turns.map((_, i) => (i === 5 ? "k5-changed" : `k${i}`));
    expect(planTurnWindow(input({ keys })).mounted).toContain(5);
  });

  it("parks the measured turns within the find band around the window, and nothing beyond it", () => {
    const plan = planTurnWindow(input());
    const mounted = new Set(plan.mounted);
    for (const index of plan.parked) {
      expect(mounted.has(index)).toBe(false);
      expect(index).toBeGreaterThanOrEqual(plan.range.from - TURN_FIND_BAND);
      expect(index).toBeLessThan(plan.range.to + TURN_FIND_BAND);
    }
    const expected = turns.map((_, i) => i)
      .filter((i) => !mounted.has(i) && i >= plan.range.from - TURN_FIND_BAND && i < plan.range.to + TURN_FIND_BAND);
    expect(plan.parked).toEqual(expected);
  });

  it("mounts pinned turns wherever they are", () => {
    const plan = planTurnWindow(input({ pinned: new Set([0, 1]) }));
    expect(plan.mounted.slice(0, 2)).toEqual([0, 1]);
  });
});

describe("turnSlots", () => {
  it("lays parked turns out at their measured heights between spacers", () => {
    const slots = turnSlots(turns, [20, turns.length - 1], measurements(), GAP, [15, 16, 25]);
    expect(slots.map((slot) => slot.kind === "spacer" ? `${slot.from}-${slot.to}` : `${slot.kind}:${slot.index}`))
      .toEqual(["0-15", "parked:15", "parked:16", "17-20", "turn:20", "21-25", "parked:25", `26-${turns.length - 1}`, `turn:${turns.length - 1}`]);
    for (const slot of slots) if (slot.kind === "parked") expect(slot.height).toBe(heights[slot.index]);
  });

  it("replaces each run of unmounted turns with one spacer of their heights and inner gaps", () => {
    const mounted = [10, 11, 30, turns.length - 1];
    const slots = turnSlots(turns, mounted, measurements(), GAP);
    expect(slots.map((slot) => slot.kind === "turn" ? slot.index : `${slot.from}-${slot.to}`))
      .toEqual(["0-10", 10, 11, "12-30", 30, `31-${turns.length - 1}`, turns.length - 1]);
    // Total extent equals the unwindowed column: turns plus every gap.
    const extent = slots.reduce((sum, slot) => sum + (slot.kind === "turn" ? heights[slot.index] : slot.height), 0)
      + GAP * (slots.length - 1);
    expect(extent).toBe(heights.reduce((sum, h) => sum + h, 0) + GAP * (turns.length - 1));
  });
});
