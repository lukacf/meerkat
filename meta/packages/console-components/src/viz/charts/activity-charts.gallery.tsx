// The Activity section of the dev-only kit gallery (?kit_gallery=1): the
// Today swimlanes and the Logs severity strip with demo data, in a
// comfortable and a compact size. The gallery discovers this file by its
// *.gallery.tsx name; nothing in the product imports it.

import React from "react";

import { Item, Specimen, SpecimenRow } from "../../panel-kit/gallery/specimen";
import { SeverityStrip, severityBins, type TimeRange } from "./severity-strip";
import { Swimlanes, type SwimlaneItem, type SwimlaneLane, type SwimlaneLink, type SwimlaneMark, type SwimlaneSpan } from "./swimlanes";

const MINUTE = 60_000;
// A fixed clock, so the gallery reads the same on every load.
const NOW = Date.parse("2026-11-01T16:40:00Z");
const START = NOW - 60 * MINUTE;

const LANES: SwimlaneLane[] = [
  { id: "triage", label: "Triage", live: true },
  { id: "support", label: "Support" },
  { id: "router", label: "Router" },
  { id: "billing", label: "Billing" },
  { id: "quiet", label: "+4 quieter agents", folded: true },
];

function span(id: string, lane: string, from: number, to: number | null, state: SwimlaneSpan["state"], ticks: number[] = []): SwimlaneSpan {
  return {
    id,
    lane,
    start: NOW - from * MINUTE,
    ...(to === null ? {} : { end: NOW - to * MINUTE }),
    state,
    ticks: ticks.map((at) => NOW - at * MINUTE),
    label: `${lane} turn, ${state}`,
  };
}

const SPANS: SwimlaneSpan[] = [
  span("turn-1", "triage", 55, 50, "done", [54, 52]),
  span("turn-2", "triage", 41, 33, "done", [40, 38, 35]),
  span("turn-3", "triage", 12, null, "live", [10, 6]),
  span("turn-4", "support", 49, 44, "done", [47]),
  span("turn-5", "support", 28, 24, "done"),
  span("turn-6", "router", 58, 57, "done"),
  span("turn-7", "router", 37, 36.5, "done"),
  span("turn-8", "router", 20, 19.4, "done"),
  span("turn-9", "billing", 46, 42, "failed", [45]),
  span("turn-10", "billing", 26, 18, "done", [25, 22, 20]),
  span("turn-11", "quiet", 52, 51, "done"),
  span("turn-12", "quiet", 30, 29, "done"),
  span("turn-13", "quiet", 8, 7, "done"),
];

const LINKS: SwimlaneLink[] = [
  { id: "m1", from: "triage", to: "support", at: NOW - 49.5 * MINUTE, label: "Triage to Support, refund 4411" },
  { id: "m2", from: "support", to: "billing", at: NOW - 46.5 * MINUTE, label: "Support to Billing, refund 4411" },
  { id: "m3", from: "router", to: "triage", at: NOW - 37 * MINUTE, label: "Router to Triage" },
  ...Array.from({ length: 5 }, (_, index) => ({ id: `burst-${index}`, from: "triage", to: "billing", at: NOW - 10 * MINUTE, label: "Triage to Billing, ticket" })),
];

const MARKS: SwimlaneMark[] = [
  { id: "a1", lane: "billing", at: NOW - 18 * MINUTE, shape: "hollow-diamond", tone: "warn", label: "Approval requested: refund 1,240 EUR" },
  { id: "l1", lane: "support", at: NOW - 15 * MINUTE, shape: "square", tone: "info", label: "Support was respawned" },
];

const PROBLEMS: SwimlaneMark[] = [
  { id: "p1", lane: "__problems", at: NOW - 42 * MINUTE, shape: "diamond", tone: "crit", label: "Billing's turn failed" },
  { id: "p2", lane: "__problems", at: NOW - 23 * MINUTE, shape: "triangle", tone: "warn", label: "A tool timed out" },
];

function SwimlanesDemo({ compact }: { compact?: boolean }) {
  const [selected, setSelected] = React.useState<string | null>(null);
  return (
    <Swimlanes
      lanes={LANES}
      spans={SPANS}
      links={LINKS}
      marks={MARKS}
      problems={PROBLEMS}
      start={START}
      end={NOW}
      now={NOW}
      laneHeight={compact ? 22 : 28}
      label="4 agents and 4 quieter ones, 13 turns, 8 messages and 2 problems in the last hour"
      onSelect={(item: SwimlaneItem) => setSelected(item.id)}
      selectedId={selected}
      testIdPrefix={compact ? "gallery-compact" : "gallery"}
    />
  );
}

function StripDemo() {
  const [range, setRange] = React.useState<TimeRange | null>({ start: NOW - 24 * MINUTE, end: NOW - 16 * MINUTE });
  const bins = React.useMemo(() => {
    const items: Array<{ t: number; severity: "info" | "warn" | "crit" }> = [];
    for (let minute = 0; minute < 60; minute += 1) {
      const count = 2 + ((minute * 7) % 9);
      for (let n = 0; n < count; n += 1) items.push({ t: START + minute * MINUTE + n * 1000, severity: "info" });
    }
    items.push({ t: NOW - 42 * MINUTE, severity: "crit" }, { t: NOW - 23 * MINUTE, severity: "warn" }, { t: NOW - 22 * MINUTE, severity: "warn" });
    return severityBins(items, START, NOW, 60);
  }, []);
  return (
    <SeverityStrip
      bins={bins}
      start={START}
      end={NOW}
      now={NOW}
      range={range}
      onRange={setRange}
      label="Events in the last hour, 1 error and 2 warnings. Drag to pick a time range."
      testIdPrefix="gallery-logs"
    />
  );
}

export const gallerySections = [
  {
    id: "activity",
    title: "Activity",
    order: 85,
    description: "Swimlanes (Today, B4) and the severity strip (Logs, B5) on the shared time axis.",
    render: () => (
      <>
        <Specimen title="Swimlanes" note="Turns as spans with tool ticks, a live turn, a failed one, message connectors (a burst of 5 counted), an approval, a respawn, the problems lane and a folded lane." bare>
          <SwimlanesDemo />
        </Specimen>
        <SpecimenRow>
          <Specimen title="Swimlanes, compact" width={520} bare>
            <SwimlanesDemo compact />
          </Specimen>
          <Specimen title="Severity strip" note="60 bins, a picked range; drag to pick another, Escape clears." width={520}>
            <Item label="the Logs strip">
              <StripDemo />
            </Item>
          </Specimen>
        </SpecimenRow>
      </>
    ),
  },
];
