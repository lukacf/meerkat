import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { SeverityStrip, severityBinWords, severityBins } from "./severity-strip";
import { Swimlanes, type SwimlaneLane, type SwimlaneLink, type SwimlaneMark, type SwimlaneSpan } from "./swimlanes";

afterEach(() => {
  cleanup();
});

const SECOND = 1000;
// 704 s over the lanes' 704 px plot (720 px less 8 px a side): a pixel a second.
const T0 = Date.parse("2026-11-01T09:00:00Z");
const T1 = T0 + 704 * SECOND;
const at = (seconds: number) => T0 + seconds * SECOND;
const x = (seconds: number) => 8 + seconds;

function box(width: number, height: number) {
  return () => ({ left: 0, top: 0, x: 0, y: 0, width, height, right: width, bottom: height, toJSON: () => ({}) }) as DOMRect;
}

const LANES: SwimlaneLane[] = [
  { id: "triage", label: "Triage", live: true },
  { id: "billing", label: "Billing" },
  { id: "quiet", label: "+3 quieter agents", folded: true },
];
const SPANS: SwimlaneSpan[] = [
  { id: "run-1", lane: "triage", start: at(100), end: at(200), state: "done", ticks: [at(150)], label: "Triage's turn, done" },
  { id: "run-2", lane: "triage", start: at(600), state: "live", label: "Triage's turn, working" },
  { id: "run-3", lane: "billing", start: at(300), end: at(340), state: "failed", label: "Billing's turn failed" },
  { id: "run-4", lane: "quiet", start: at(500), end: at(510), state: "done", label: "A quieter agent's turn" },
  { id: "run-x", lane: "nobody", start: at(10), end: at(20), state: "done", label: "A turn on no lane" },
];
const LINKS: SwimlaneLink[] = [
  // A burst: five messages of one pair in one pixel column draw once.
  ...Array.from({ length: 5 }, (_, index) => ({ id: `m-${index}`, from: "triage", to: "billing", at: at(400), label: "Triage messaged Billing" })),
  { id: "m-back", from: "billing", to: "triage", at: at(450), label: "Billing messaged Triage" },
  { id: "m-lost", from: "billing", to: "nobody", at: at(460), label: "Billing messaged someone unseen" },
];
const MARKS: SwimlaneMark[] = [
  { id: "a-1", lane: "billing", at: at(320), shape: "hollow-diamond", tone: "warn", label: "Approval requested" },
];
const PROBLEMS: SwimlaneMark[] = [
  { id: "p-1", lane: "__problems", at: at(340), shape: "diamond", tone: "crit", label: "Billing's turn failed" },
];

function lanes(onSelect?: (item: { kind: string; id: string }) => void, problems: SwimlaneMark[] = PROBLEMS) {
  return render(
    <Swimlanes
      lanes={LANES}
      spans={SPANS}
      links={LINKS}
      marks={MARKS}
      problems={problems}
      start={T0}
      end={T1}
      now={at(650)}
      defaultWidth={856}
      label="3 lanes"
      onSelect={onSelect}
      testIdPrefix="t"
    />,
  );
}

describe("Swimlanes", () => {
  it("draws the lanes, the spans by state, a burst once with its count, and drops what has no lane", () => {
    const view = lanes();
    expect(view.getByTestId("t-swimlanes")).toBeTruthy();
    expect(view.getByTestId("t-lane:triage").getAttribute("data-live")).toBe("true");
    expect(view.getByTestId("t-lane:problems").textContent).toBe("Problems");
    expect(view.getByTestId("t-span:run-1").getAttribute("data-state")).toBe("done");
    expect(view.getByTestId("t-span:run-1").querySelectorAll(".cc-lanes__tick").length).toBe(1);
    expect(view.getByTestId("t-span:run-2").getAttribute("data-state")).toBe("live");
    expect(view.getByTestId("t-span:run-2").querySelector(".cc-lanes__live-end")).toBeTruthy();
    expect(view.getByTestId("t-span:run-3").getAttribute("data-state")).toBe("failed");
    // The folded lane draws its turns as ticks.
    expect(view.getByTestId("t-span:run-4").getAttribute("class")).toContain("cc-lanes__fold-tick");
    expect(view.queryByTestId("t-span:run-x")).toBeNull();

    const links = Array.from(view.container.querySelectorAll('[data-kind="link"]'));
    expect(links.map((link) => link.getAttribute("data-testid"))).toEqual(["t-link:m-0", "t-link:m-back"]);
    const burst = view.getByTestId("t-link:m-0");
    expect(burst.textContent).toBe("5");
    expect(burst.getAttribute("aria-label")).toBe("5 messages. Triage messaged Billing");
    expect(view.getByTestId("t-link:m-back").textContent).toBe("");
    expect(view.queryByTestId("t-link:m-lost")).toBeNull();
    expect(view.getByTestId("t-mark:a-1").getAttribute("data-shape")).toBe("hollow-diamond");
    expect(view.getByTestId("t-problem:p-1").getAttribute("data-tone")).toBe("crit");
  });

  it("shows no problems lane without problems", () => {
    const view = lanes(undefined, []);
    expect(view.queryByTestId("t-lane:problems")).toBeNull();
    expect(view.queryByTestId("t-problem:p-1")).toBeNull();
  });

  it("keeps one tab stop; arrows move along a lane and to the nearest mark of the next, Enter selects", () => {
    const onSelect = vi.fn();
    const view = lanes(onSelect);
    const svg = view.getByTestId("t-swimlanes").querySelector("svg")!;
    const stops = () => Array.from(view.container.querySelectorAll('[data-item][tabindex="0"]')).map((element) => element.getAttribute("data-item"));
    // The first mark of the top lane (the problems lane) holds the tab stop.
    expect(stops()).toEqual(["p-1"]);
    // Down: Triage's mark nearest in time to the problem (the burst at 400 s).
    fireEvent.keyDown(svg, { key: "ArrowDown" });
    expect(stops()).toEqual(["m-0"]);
    expect(view.getByText("5 messages. Triage messaged Billing")).toBeTruthy();
    fireEvent.keyDown(svg, { key: "ArrowLeft" });
    expect(stops()).toEqual(["run-1"]);
    fireEvent.keyDown(svg, { key: "End" });
    expect(stops()).toEqual(["run-2"]);
    fireEvent.keyDown(svg, { key: "Home" });
    expect(stops()).toEqual(["run-1"]);
    fireEvent.keyDown(svg, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect.mock.calls[0][0]).toMatchObject({ kind: "span", id: "run-1" });
    // Escape hides the tooltip and keeps the tab stop.
    expect(view.getByText("Triage's turn, done")).toBeTruthy();
    fireEvent.keyDown(svg, { key: "Escape" });
    expect(view.queryByText("Triage's turn, done")).toBeNull();
    expect(stops()).toEqual(["run-1"]);
  });

  it("selects the mark under a click, and nothing far from every mark", () => {
    const onSelect = vi.fn();
    const view = lanes(onSelect);
    const svg = view.getByTestId("t-swimlanes").querySelector("svg")!;
    // Rows: the axis (20 px), Problems (22 px, centre 31), Triage (centre 56), Billing (84), quieter (112).
    svg.getBoundingClientRect = box(720, 128);
    fireEvent.click(svg, { clientX: x(150), clientY: 56 });
    expect(onSelect.mock.calls.at(-1)?.[0]).toMatchObject({ kind: "span", id: "run-1" });
    fireEvent.click(svg, { clientX: x(340), clientY: 31 });
    expect(onSelect.mock.calls.at(-1)?.[0]).toMatchObject({ kind: "problem", id: "p-1" });
    fireEvent.click(svg, { clientX: x(400), clientY: 70 });
    expect(onSelect.mock.calls.at(-1)?.[0]).toMatchObject({ kind: "link", id: "m-0", count: 5 });
    const calls = onSelect.mock.calls.length;
    fireEvent.click(svg, { clientX: x(690), clientY: 84 });
    expect(onSelect.mock.calls.length).toBe(calls);
  });
});

describe("SeverityStrip", () => {
  // jsdom has no PointerEvent; a mouse event of the pointer type carries the
  // coordinates and button the strip reads.
  const hadPointerEvent = "PointerEvent" in window;
  beforeAll(() => {
    if (hadPointerEvent) return;
    class TestPointerEvent extends MouseEvent {
      pointerId: number;
      constructor(type: string, init: PointerEventInit = {}) {
        super(type, init);
        this.pointerId = init.pointerId ?? 1;
      }
    }
    Object.defineProperty(window, "PointerEvent", { value: TestPointerEvent, configurable: true, writable: true });
  });
  afterAll(() => {
    if (!hadPointerEvent) Reflect.deleteProperty(window, "PointerEvent");
  });

  it("counts items by severity in equal bins inside the window", () => {
    const bins = severityBins([
      { t: at(0), severity: "info" },
      { t: at(10), severity: "warn" },
      { t: at(703), severity: "crit" },
      // The window's end lands in the last bin; items outside it count nowhere.
      { t: T1, severity: "info" },
      { t: T1 + 1, severity: "crit" },
      { t: T0 - 1, severity: "warn" },
    ], T0, T1, 4);
    expect(bins.map((bin) => [bin.info, bin.warn, bin.crit])).toEqual([[1, 1, 0], [0, 0, 0], [0, 0, 0], [1, 0, 1]]);
    expect(bins[0].start).toBe(T0);
    expect(bins[3].end).toBe(T1);
    expect(severityBinWords(bins[3])).toMatch(/^\d\d:\d\d-\d\d:\d\d: 2 events, 1 error$/);
    expect(severityBinWords(bins[0])).toMatch(/: 2 events, 1 warning$/);
  });

  it("picks a range by drag, shows a bin on click, and picks and clears a range by keys", () => {
    const onRange = vi.fn();
    const onBin = vi.fn();
    const bins = severityBins([{ t: at(5), severity: "crit" }, { t: at(600), severity: "info" }], T0, T1, 60);
    const strip = (range: { start: number; end: number } | null) => (
      <SeverityStrip bins={bins} start={T0} end={T1} range={range} onRange={onRange} onBinActivate={onBin} defaultWidth={600} label="Events" testIdPrefix="s" />
    );
    const view = render(strip(null));
    const svg = view.getByTestId("s-severity-strip").querySelector("svg")!;
    svg.getBoundingClientRect = box(600, 40);
    expect(view.getByTestId("s-strip-bin:0").getAttribute("data-crit")).toBe("1");

    // A drag from 60 px to 300 px: a tenth to a half of the window.
    fireEvent.pointerDown(svg, { clientX: 60, button: 0, pointerId: 1 });
    fireEvent.pointerMove(svg, { clientX: 300, pointerId: 1 });
    fireEvent.pointerUp(svg, { clientX: 300, pointerId: 1 });
    expect(onRange).toHaveBeenCalledTimes(1);
    const picked = onRange.mock.calls[0][0] as { start: number; end: number };
    expect(picked.start).toBeCloseTo(T0 + 70_400, 3);
    expect(picked.end).toBeCloseTo(T0 + 352_000, 3);
    expect(onBin).not.toHaveBeenCalled();

    // A click without a drag shows the bin under it (10 px a bin).
    fireEvent.pointerDown(svg, { clientX: 125, button: 0, pointerId: 1 });
    fireEvent.pointerUp(svg, { clientX: 125, pointerId: 1 });
    expect(onBin).toHaveBeenCalledWith(12, bins[12]);

    // Keys: End, Space picks the bin, Shift+Left widens, Enter shows, Escape clears.
    fireEvent.focus(svg);
    fireEvent.keyDown(svg, { key: "End" });
    fireEvent.keyDown(svg, { key: " " });
    expect(onRange.mock.calls.at(-1)?.[0]).toEqual({ start: bins[59].start, end: bins[59].end });
    fireEvent.keyDown(svg, { key: "ArrowLeft", shiftKey: true });
    expect(onRange.mock.calls.at(-1)?.[0]).toEqual({ start: bins[58].start, end: bins[59].end });
    fireEvent.keyDown(svg, { key: "Enter" });
    expect(onBin).toHaveBeenLastCalledWith(58, bins[58]);
    const before = onRange.mock.calls.length;
    // Escape clears only a range that is set.
    fireEvent.keyDown(svg, { key: "Escape" });
    expect(onRange.mock.calls.length).toBe(before);
    view.rerender(strip({ start: bins[58].start, end: bins[59].end }));
    fireEvent.keyDown(svg, { key: "Escape" });
    expect(onRange.mock.calls.at(-1)?.[0]).toBeNull();
  });
});
