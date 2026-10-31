import { fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import {
  BarcodeMeter,
  DotStrip,
  FigureTile,
  GlanceStrip,
  HeatStrip,
  Meter,
  MiniBar,
  ProgressRing,
  Sparkline,
  barcodeUnitsPerSegment,
  binCounts,
  binIndex,
  brushRange,
  createTimeScale,
  deltaTone,
  formatDelta,
  formatFigure,
  formatTimeRange,
  heatLevels,
  timeTicks,
} from "../../panel-kit";

// Micro charts and the time axis (console v2 spec A3.3, A3.8). DOM
// assertions compare booleans and strings, never elements.

const HOUR = 3_600_000;
const DAY_START = new Date(2026, 9, 30, 0, 0, 0).getTime();

describe("time axis", () => {
  it("maps time to x and back", () => {
    const scale = createTimeScale({ start: 1000, end: 2000, range: [10, 110] });
    expect(scale.x(1500)).toBe(60);
    expect(scale.t(60)).toBe(1500);
    expect(createTimeScale({ start: 5, end: 5, range: [10, 20] }).x(5)).toBe(10);
  });

  it("puts 6 h ticks on the local clock's 00, 06, 12 and 18", () => {
    const ticks = timeTicks(DAY_START + 1.5 * HOUR, DAY_START + 25 * HOUR, { step: 6 * HOUR, format: "hour" });
    expect(ticks.map((tick) => tick.label)).toEqual(["06", "12", "18", "00"]);
    expect(ticks.every((tick) => new Date(tick.t).getMinutes() === 0 && new Date(tick.t).getHours() % 6 === 0)).toBe(true);
    expect(ticks[3].major).toBe(true);
  });

  it("counts times into bins and orders a brushed range", () => {
    expect(binCounts([0, 1, 9, 10, 25, 31], 0, 30, 3)).toEqual([3, 1, 1]);
    expect(binIndex(30, 0, 30, 3)).toBe(2);
    expect(binIndex(31, 0, 30, 3)).toBe(-1);
    const scale = createTimeScale({ start: 0, end: 100, range: [0, 100] });
    expect(brushRange(scale, 80, -20)).toEqual({ start: 0, end: 80 });
    expect(formatTimeRange({ start: DAY_START + 17 * HOUR + 2 * 60_000, end: DAY_START + 17 * HOUR + 9 * 60_000 })).toBe("17:02-17:09");
  });
});

describe("figures", () => {
  it("compacts values and words the change", () => {
    expect([formatFigure(999), formatFigure(1284), formatFigure(12_900), formatFigure(3_400_000), formatFigure(10_000)]).toEqual(["999", "1,284", "12.9K", "3.4M", "10K"]);
    expect(formatDelta({ value: 2, period: "1h ago" })).toBe("+2 vs 1h ago");
    expect(formatDelta({ value: -3, period: "the hour before" })).toBe("-3 vs the hour before");
    expect(formatDelta({ value: 0, period: "1h ago" })).toBe("No change vs 1h ago");
    expect(deltaTone({ value: 2, period: "", goodWhen: "up" })).toBe("ok");
    expect(deltaTone({ value: 2, period: "", goodWhen: "down" })).toBe("crit");
    expect(deltaTone({ value: 2, period: "" })).toBe(undefined);
  });

  it("shows a figure with its label, whole, mark and trend in a strip", () => {
    const view = render(
      <GlanceStrip ariaLabel="Topology at a glance" testId="glance">
        <FigureTile label="Active now" value={2} of={7} live trend={[1, 2, 0, 3]} trendLive testId="tile" />
        <FigureTile label="Problems" value={1} tone="warn" delta={{ value: 1, period: "1h ago", goodWhen: "down" }} testId="problems" />
      </GlanceStrip>,
    );
    expect(view.getByRole("group", { name: "Topology at a glance" }) !== null).toBe(true);
    const tile = view.getByTestId("tile");
    expect(tile.textContent?.startsWith("Active now2of 7")).toBe(true);
    expect(tile.querySelector('.cc-mark[data-tone="live"]') !== null).toBe(true);
    expect(tile.querySelector('svg[role="img"]')?.getAttribute("aria-label")).toBe("Active now trend");
    expect(view.getByTestId("problems").querySelector(".cc-figure__delta")?.getAttribute("data-tone")).toBe("crit");
  });

  it("makes a filtering figure a pressed button, and holds a skeleton while loading", () => {
    const onActivate = vi.fn();
    const view = render(<FigureTile label="Blocked" value={1} onActivate={onActivate} pressed testId="tile" />);
    const button = view.getByTestId("tile");
    expect(button.tagName).toBe("BUTTON");
    expect(button.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(button);
    expect(onActivate).toHaveBeenCalledTimes(1);
    view.rerender(<FigureTile label="Blocked" value={1} loading testId="tile" />);
    expect(view.getByTestId("tile").querySelector(".cc-figure__skeleton") !== null).toBe(true);
    expect(view.getByTestId("tile").querySelector(".cc-figure__value") === null).toBe(true);
  });
});

describe("micro charts", () => {
  it("draws sparkline bars with faint zero bins and the live last bin", () => {
    const view = render(<Sparkline values={[2, 0, 5]} kind="bars" live label="7 messages" testId="spark" />);
    expect(view.getByRole("img", { name: "7 messages" }) !== null).toBe(true);
    const root = view.getByTestId("spark");
    expect(root.querySelectorAll(".cc-spark__bar").length).toBe(2);
    expect(root.querySelectorAll(".cc-spark__zero").length).toBe(1);
    expect(root.querySelectorAll(".cc-spark__bar.is-live").length).toBe(1);
  });

  it("ends a line with a dot, in the accent when live", () => {
    const view = render(<Sparkline values={[1, 3, 2]} label="trend" testId="spark" />);
    expect(view.getByTestId("spark").querySelector(".cc-spark__dot.is-live") === null).toBe(true);
    view.rerender(<Sparkline values={[1, 3, 2]} live label="trend" testId="spark" />);
    expect(view.getByTestId("spark").querySelector(".cc-spark__dot.is-live") !== null).toBe(true);
  });

  it("says a progress ring's numbers in words", () => {
    const view = render(<ProgressRing done={3} inProgress={1} blocked={1} total={5} />);
    expect(view.getByRole("img").getAttribute("aria-label")).toBe("3 of 5 done, 1 in progress, 1 blocked");
    expect(view.container.querySelectorAll(".cc-ring__arc").length).toBe(3);
  });

  it("lights the barcode in ink, accent and outline, and states the unit when one segment holds more", () => {
    const view = render(<BarcodeMeter filled={7} live={1} outlined={1} label="8 agents" segments={24} />);
    const kinds = Array.from(view.container.querySelectorAll(".cc-barcode__seg")).map((seg) => seg.getAttribute("data-kind"));
    expect(kinds.filter((kind) => kind === "filled").length).toBe(6);
    expect(kinds.filter((kind) => kind === "live").length).toBe(1);
    expect(kinds.filter((kind) => kind === "outlined").length).toBe(1);
    expect(kinds.filter((kind) => kind === "unlit").length).toBe(16);
    expect(barcodeUnitsPerSegment(300, 40)).toBe(10);
    const big = render(<BarcodeMeter filled={300} label="300 agents" segments={40} testId="big" />);
    expect(big.getByTestId("big").querySelector("svg")?.getAttribute("aria-label")).toBe("300 agents (1 segment = 10 agents)");
  });

  it("ranks heat cells by quantile, zero apart, with a table twin", () => {
    const level = heatLevels([0, 1, 1, 1, 1]);
    expect(level(0)).toBe(0);
    expect(level(1)).toBe(4);
    const spread = heatLevels([1, 2, 3, 4, 5, 6, 7]);
    expect(spread(1)).toBe(1);
    expect(spread(7)).toBe(7);
    const view = render(<HeatStrip label="Activity by hour" rows={[{ key: "a", label: "Triage", values: [0, 3, 1] }]} columns={["00:00", "01:00", "02:00"]} />);
    expect(view.container.querySelectorAll(".cc-heat__cell").length).toBe(3);
    expect(view.container.querySelector("table caption")?.textContent).toBe("Activity by hour");
    expect(view.container.querySelector("tbody th")?.textContent).toBe("Triage");
  });

  it("gives a dot strip one tab stop, ids per mark and selection by key", () => {
    const onSelect = vi.fn();
    const view = render(
      <DotStrip
        start={0}
        end={100}
        label="3 records"
        testIdPrefix="memory-landscape-tick"
        onSelect={onSelect}
        items={[
          { id: "b", t: 60, shape: "triangle", tone: "warn", label: "Quarantined record" },
          { id: "a", t: 10, shape: "tick", label: "First record" },
          { id: "c", t: 90, faint: true, label: "Replaced record" },
        ]}
      />,
    );
    expect(view.getByRole("group", { name: "3 records" }) !== null).toBe(true);
    const first = view.getByTestId("memory-landscape-tick:a");
    expect(first.getAttribute("tabindex")).toBe("0");
    expect(view.getByTestId("memory-landscape-tick:b").getAttribute("tabindex")).toBe("-1");
    expect(view.container.querySelector("title") === null).toBe(true);
    expect(first.getAttribute("aria-label")).toBe("First record");
    fireEvent.keyDown(first, { key: "ArrowRight" });
    fireEvent.keyDown(view.getByTestId("memory-landscape-tick:b"), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ id: "b" }));
  });

  it("fills a mini bar to its ratio and names it", () => {
    const view = render(<MiniBar value={2.2} max={8} label="2.2 KB of 8 KB used" valueLabel="2.2 KB of 8 KB" />);
    expect(view.getByRole("img", { name: "2.2 KB of 8 KB used" }) !== null).toBe(true);
    expect((view.container.querySelector(".cc-minibar__fill") as HTMLElement | null)?.style.width).toBe("27.500000000000004%");
  });

  it("marks the meter segment before an unfilled rest", () => {
    const view = render(<Meter label="1 of 5 done" total={5} segments={[{ value: 1, tone: "ok" }]} testId="meter" />);
    expect(view.getByTestId("meter").querySelector(".cc-meter__seg")?.getAttribute("data-rest")).toBe("true");
    view.rerender(<Meter label="all done" total={2} segments={[{ value: 2, tone: "ok" }]} testId="meter" />);
    expect(view.getByTestId("meter").querySelector(".cc-meter__seg")?.hasAttribute("data-rest")).toBe(false);
  });
});
