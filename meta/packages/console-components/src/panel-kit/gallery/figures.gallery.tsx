import { useState } from "react";

import { FigureTile } from "../figure-tile";
import { GlanceStrip } from "../glance-strip";
import { Meter } from "../meter";
import { PanelShell } from "../panel";
import { ActivityBars } from "../../viz/charts/activity-bars";
import { BarcodeLegend, BarcodeMeter } from "../../viz/charts/barcode-meter";
import { DotStrip, type DotStripItem } from "../../viz/charts/dot-strip";
import { HeatStrip } from "../../viz/charts/heat-strip";
import { MiniBar } from "../../viz/charts/mini-bar";
import { ProgressRing } from "../../viz/charts/progress-ring";
import { Sparkline } from "../../viz/charts/sparkline";
import { Item, Specimen, SpecimenRow } from "./specimen";
import type { GallerySection } from "./types";

const NOW = Date.now();
const HOUR = 3_600_000;
const DAY = 24 * HOUR;

// A seeded pseudo-random series, so screenshots are stable.
function series(seed: number, count: number, scale: number, zeroEvery = 0): number[] {
  let x = seed;
  return Array.from({ length: count }, (_, index) => {
    x = (x * 9301 + 49297) % 233280;
    const wave = (Math.sin(index / 3 + seed) + 1.2) * scale * 0.45;
    const value = Math.round(wave + (x / 233280) * scale * 0.6);
    return zeroEvery && index % zeroEvery === 0 ? 0 : Math.max(0, value);
  });
}

const RECORDS: DotStripItem[] = Array.from({ length: 64 }, (_, index) => {
  const t = NOW - 30 * DAY + ((index * 7919) % 1000) / 1000 * 30 * DAY;
  const quarantined = index === 41;
  const faint = index % 9 === 4;
  return {
    id: `record-${index}`,
    t,
    shape: quarantined ? "triangle" : "tick",
    tone: quarantined ? "warn" : undefined,
    faint,
    label: quarantined ? "Quarantined: refund policy override" : faint ? "Replaced: team calendar" : `Record ${index + 1}`,
  } satisfies DotStripItem;
});

const DECISIONS: DotStripItem[] = [
  { id: "d1", t: NOW - 20 * HOUR, shape: "dot", tone: "ok", label: "Approved: deploy 2.3.9" },
  { id: "d2", t: NOW - 17 * HOUR, shape: "dot", tone: "ok", label: "Approved: rotate keys" },
  { id: "d3", t: NOW - 14 * HOUR, shape: "diamond", tone: "crit", label: "Rejected: delete archive" },
  { id: "d4", t: NOW - 9 * HOUR, shape: "triangle", tone: "warn", label: "Escalated: refund over limit" },
  { id: "d5", t: NOW - 6 * HOUR, shape: "dot", tone: "ok", label: "Approved: vendor scorecards" },
  { id: "d6", t: NOW - 3 * HOUR, shape: "hollow", label: "Timed out: nightly digest" },
  { id: "d7", t: NOW - 1 * HOUR, shape: "dot", tone: "ok", label: "Approved: publish notes" },
];

const WEEK_ROWS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map((day, index) => ({
  key: day,
  label: day,
  values: series(index + 3, 24, index > 4 ? 2 : 6).map((value, hour) => (hour < 6 || hour > 21 ? 0 : value)),
}));

function FilteringStrip() {
  const [filter, setFilter] = useState<string | null>("blocked");
  const toggle = (id: string) => setFilter((current) => (current === id ? null : id));
  return (
    <PanelShell>
      <GlanceStrip ariaLabel="WorkGraph at a glance">
        <FigureTile
          hero
          label="Done"
          value={1}
          of={9}
          meter={<ProgressRing done={1} inProgress={3} blocked={1} total={9} size={24} />}
          foot="3 in progress · 1 blocked"
        />
        <FigureTile label="In progress" value={3} live onActivate={() => toggle("live")} pressed={filter === "live"} />
        <FigureTile label="Blocked" value={1} tone="warn" onActivate={() => toggle("blocked")} pressed={filter === "blocked"} foot="Publish to all regions" />
        <FigureTile label="Ready" value={2} tone="ok" onActivate={() => toggle("ready")} pressed={filter === "ready"} />
        <FigureTile label="To finish" value="3 steps" foot="Critical path on" />
      </GlanceStrip>
    </PanelShell>
  );
}

// WorkGraph's five figures (or four), as a pane between 480 and 720 px wraps them.
function WorkStrip({ width, four }: { width: number; four?: boolean }) {
  return (
    <div className="kg-pane" style={{ width, flex: "none" }} data-testid={`kit-glance-pane:${width}${four ? ":four" : ""}`}>
      <PanelShell>
        <GlanceStrip ariaLabel="Work at a glance">
          <FigureTile hero label="Done" value={1} of={9} meter={<ProgressRing done={1} inProgress={1} blocked={1} total={9} size={24} />} foot="none today" />
          <FigureTile label="In progress" tone="live" live value={1} foot="1 agent on them" />
          <FigureTile label="Blocked" tone="warn" value={1} foot="Publish to all regions" />
          <FigureTile label="Ready" tone="ok" value={2} foot="can start now" />
          {four ? null : <FigureTile label="To finish" value={3} unit="steps" foot="Critical path" />}
        </GlanceStrip>
      </PanelShell>
    </div>
  );
}

// Every kind of tile, loaded and loading, each alone in a strip, so its own
// height shows (in a strip of several the row takes the tallest).
const LOADING_KINDS: Array<[string, Omit<Parameters<typeof FigureTile>[0], "loading">]> = [
  ["plain", { label: "Ready", value: 2 }],
  ["foot", { label: "Ready", value: 2, tone: "ok", foot: "can start now" }],
  ["of", { label: "Done", value: 1, of: 9, foot: "none today" }],
  ["unit", { label: "To finish", value: 3, unit: "steps", foot: "Critical path" }],
  ["delta", { label: "Messages", value: 48, delta: { value: 12, period: "the hour before" }, foot: "rising" }],
  ["trend", { label: "Given · 24h", value: 1284, trend: series(9, 24, 8), trendLive: true, foot: "to 3 agents" }],
  ["meter", { label: "Checks", value: 3, of: 4, meter: <ProgressRing done={3} total={4} size={24} />, foot: "holding" }],
  ["hero", { label: "Records", value: 74, unit: "active", hero: true, meter: <Sparkline values={series(4, 30, 4)} kind="bars" width={128} height={26} label="Records per day" />, foot: "+9 written this week" }],
  ["button", { label: "Blocked", value: 1, tone: "warn", foot: "Publish to all regions", onActivate: () => {} }],
];

function LoadingPairs({ density }: { density: "comfortable" | "compact" }) {
  return (
    <PanelShell density={density}>
      <div className="kg-grid" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(460px, 1fr))", padding: 12 }}>
        {LOADING_KINDS.map(([kind, props]) => (
          <div key={kind} className="kg-inline" style={{ alignItems: "flex-start", gap: 8, flexWrap: "nowrap" }} data-testid={`kit-figure-pair:${density}:${kind}`}>
            {[false, true].map((loading) => (
              <div key={String(loading)} className="cc-glance" style={{ flex: "1 1 0", border: "1px solid var(--_cc-border-soft)" }}>
                <FigureTile {...props} loading={loading} testId={`kit-figure:${density}:${kind}:${loading ? "loading" : "loaded"}`} />
              </div>
            ))}
          </div>
        ))}
      </div>
    </PanelShell>
  );
}

export const gallerySections: GallerySection[] = [
  {
    id: "figures",
    title: "Glance strip and figures",
    order: 40,
    description: "A3.3: an instrument panel of figures; the hero carries the ladder or the ring; tones add marks, never fills.",
    render: () => (
      <>
        <Specimen bare title="Filtering figures" note="a figure that filters is a pressed button">
          <FilteringStrip />
        </Specimen>
        <SpecimenRow>
          <Specimen bare title="Memory" note="trend, change and the check marks">
            <PanelShell>
              <GlanceStrip>
                <FigureTile label="Records" value={74} trend={series(4, 30, 4)} trendKind="bars" trendLabel="New records per day over 30 days" delta={{ value: 9, period: "last week", goodWhen: "up" }} />
                <FigureTile label="Needs review" value={3} tone="warn" foot="1 quarantined · 2 waiting" />
                <FigureTile label="Given · 24h" value={1284} trend={series(9, 24, 8)} trendKind="line" trendLive trendLabel="Injections per hour over 24 hours" />
              </GlanceStrip>
            </PanelShell>
          </Specimen>
          <Specimen bare title="Loading" note="a 40 px skeleton holds the figure">
            <PanelShell>
              <GlanceStrip>
                <FigureTile label="Turns" value={0} loading />
                <FigureTile label="Messages" value={0} loading />
                <FigureTile label="Problems" value={0} loading />
              </GlanceStrip>
            </PanelShell>
          </Specimen>
        </SpecimenRow>
        <Specimen title="Figures at mid widths" note="from 480 to 719 px five wrap three and two (520, 680 px), four two and two (600 px); 760 px keeps one row">
          <div className="kg-stack">
            {[520, 680, 760].map((width) => (
              <WorkStrip key={width} width={width} />
            ))}
            <WorkStrip width={600} four />
          </div>
        </Specimen>
        <Specimen bare title="Loading keeps the height" note="each kind of tile loaded (left) and loading (right): the same boxes, the foot line kept; comfortable and compact">
          <LoadingPairs density="comfortable" />
          <LoadingPairs density="compact" />
        </Specimen>
      </>
    ),
  },
  {
    id: "charts",
    title: "Micro charts",
    order: 45,
    description: "A3.8: SVG in the viz tokens, the numbers in words for assistive tech; hover or focus a chart for its tooltip.",
    render: () => (
      <>
        <SpecimenRow>
          <Specimen title="Sparkline" note="row 64 x 16, tile 96 x 20, drawer 240 x 40">
            <div className="kg-stack">
              <div className="kg-inline" style={{ gap: "16px 28px" }}>
                <Item label="row line">
                  <Sparkline values={series(1, 16, 5)} label="Turns over 16 hours" />
                </Item>
                <Item label="row bars, live">
                  <Sparkline values={series(2, 12, 5, 4)} kind="bars" live label="Messages over the last hour" />
                </Item>
                <Item label="tile bars">
                  <Sparkline values={series(3, 12, 6)} kind="bars" width={96} height={20} live label="Messages per 5 minutes" />
                </Item>
                <Item label="tile line, area">
                  <Sparkline values={series(5, 20, 6)} width={96} height={20} area label="Records per day" />
                </Item>
                <Item label="flat">
                  <Sparkline values={[3, 3, 3, 3, 3]} width={96} height={20} label="Constant" />
                </Item>
              </div>
              <Item label="drawer, with crosshair tooltip">
                <Sparkline
                  values={series(7, 24, 9).map((v, index) => ({ t: NOW - (24 - index) * HOUR, v }))}
                  width={240}
                  height={40}
                  area
                  live
                  tooltip
                  label="Turns per hour over 24 hours"
                  formatValue={(value) => `${value} turns`}
                />
              </Item>
            </div>
          </Specimen>
          <Specimen title="ActivityBars" note="24 h ending now, local hour ticks">
            <div className="kg-stack">
              <ActivityBars values={series(11, 24, 6, 7)} live label="46 turns in the last 24 hours" unit="turns" />
              <ActivityBars values={series(12, 48, 4)} width={300} label="Messages per half hour" unit="messages" />
            </div>
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="BarcodeMeter" note="the brand ladder: long-lived ink, live accent, helpers outlined">
            <div className="kg-stack" style={{ gap: 18 }}>
              <div className="kg-stack" style={{ gap: 6 }}>
                <BarcodeMeter filled={7} live={1} outlined={1} segments={40} width={220} label="8 agents" />
                <BarcodeLegend filled={7} outlined={1} live={1} labels={{ live: "working" }} />
              </div>
              <BarcodeMeter filled={12} live={3} outlined={4} peak={18} segments={40} width={220} scale label="16 agents, peak 18" />
              <BarcodeMeter filled={276} live={22} outlined={24} segments={40} width={220} legend scale label="300 agents" />
            </div>
          </Specimen>
          <Specimen title="Ratios" note="MiniBar, Meter with surface gaps, ProgressRing">
            <div className="kg-stack" style={{ gap: 14 }}>
              <MiniBar value={2.2} max={8} label="2.2 KB of 8 KB used" valueLabel="2.2 KB of 8 KB" />
              <MiniBar value={7.4} max={8} tone="warn" label="7.4 KB of 8 KB used, near the limit" valueLabel="7.4 KB of 8 KB" />
              <Meter
                label="7 active, 1 quarantined, 2 replaced"
                segments={[
                  { value: 7, tone: "ok", label: "7 active" },
                  { value: 1, tone: "warn", label: "1 quarantined" },
                  { value: 2, tone: "neutral", label: "2 replaced" },
                ]}
              />
              <Meter label="1 of 5 done" total={5} segments={[{ value: 1, tone: "ok", label: "1 done" }]} />
              <div className="kg-inline" style={{ gap: 18 }}>
                <Item label="16 px">
                  <ProgressRing done={1} inProgress={1} total={5} />
                </Item>
                <Item label="24 px">
                  <ProgressRing done={3} inProgress={1} blocked={1} total={5} size={24} />
                </Item>
                <Item label="done">
                  <ProgressRing done={4} total={4} size={24} />
                </Item>
                <Item label="empty">
                  <ProgressRing done={0} total={3} size={24} />
                </Item>
              </div>
            </div>
          </Specimen>
        </SpecimenRow>
        <SpecimenRow>
          <Specimen title="HeatStrip" note="this week by hour; ink ramp by quantile">
            <HeatStrip
              rows={WEEK_ROWS}
              columns={Array.from({ length: 24 }, (_, hour) => `${String(hour).padStart(2, "0")}:00`)}
              rowLabelWidth={36}
              label="Turns per hour this week, busiest Wednesday afternoon"
              formatCell={(row, column, value) => `${row.label} ${String(column).padStart(2, "0")}:00 · ${value} turns`}
            />
          </Specimen>
          <Specimen title="DotStrip" note="records over 30 days; decisions over 24 h">
            <div className="kg-stack" style={{ gap: 18 }}>
              <DotStrip items={RECORDS} start={NOW - 30 * DAY} end={NOW} width={320} height={18} now axis label="64 records over 30 days, 1 quarantined" onSelect={() => {}} />
              <DotStrip items={DECISIONS} start={NOW - 24 * HOUR} end={NOW} width={320} height={18} axis now markSize={8} label="7 decisions in the last 24 hours" />
            </div>
          </Specimen>
        </SpecimenRow>
      </>
    ),
  },
];
