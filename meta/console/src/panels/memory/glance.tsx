// The panel's first screen (L0): the live summary facts for the header and
// the glance strip of four figures (records, what needs review, memory given
// to agents in the last day, and the six checks).

import React from "react";

import type { MemoryLedgerEntry, MemoryPanelRecord } from "../../types";
import { FigureTile, GlanceStrip, Sparkline, StatusMark, plural, type SummaryFact, type Tone } from "./kit";
import { verdictTone, type VerdictTile } from "./model";
import {
  DAY_MS,
  HOUR_MS,
  countSince,
  injectionsPerHour,
  recordWrittenAt,
  recordsPerDay,
  type StoreTotals,
} from "./viz-model";

export interface GlanceInputs {
  loaded: boolean;
  recordsDenied: boolean;
  /** Store totals when the overview answered, else the loaded records'. */
  totals: StoreTotals;
  totalsFromStore: boolean;
  /** More records exist past the loaded page (no store totals). */
  morePages: boolean;
  multiRealm: boolean;
  realmCount: number;
  canReviewQuarantine: boolean;
  quarantined: number;
  waiting: number;
  injections: ReadonlyArray<MemoryLedgerEntry>;
  injectionsDenied: boolean;
  /** Every record the console has read (the sparkline's source). */
  readRecords: ReadonlyArray<MemoryPanelRecord>;
  tiles: ReadonlyArray<VerdictTile>;
  now: number;
}

/** The header's live facts: "74 records · 2 near their limit · ▲ 1
 * quarantined · 2 waiting for approval" (freshness is added by the header). */
export function memorySummaryFacts(inputs: GlanceInputs): SummaryFact[] {
  if (!inputs.loaded) return [{ key: "loading", text: "Loading records", announce: false }];
  if (inputs.recordsDenied) return [{ key: "denied", text: "No access to records", tone: "locked" }];
  const facts: SummaryFact[] = [];
  const count = inputs.totalsFromStore ? inputs.totals.total : inputs.totals.total;
  facts.push({
    key: "records",
    text: !inputs.totalsFromStore && inputs.morePages ? `${count}+ records` : plural(count, "record"),
  });
  if (inputs.multiRealm) facts.push({ key: "spaces", text: plural(inputs.realmCount, "space") });
  if (inputs.totals.nearLimit > 0) {
    facts.push({ key: "limit", text: `${inputs.totals.nearLimit} near ${inputs.totals.nearLimit === 1 ? "its" : "their"} limit` });
  }
  if (inputs.canReviewQuarantine && inputs.quarantined > 0) {
    facts.push({ key: "quarantined", text: `${inputs.quarantined} quarantined`, tone: "warn" });
  }
  if (inputs.canReviewQuarantine && inputs.waiting > 0) {
    facts.push({ key: "waiting", text: `${inputs.waiting} waiting for approval` });
  }
  return facts;
}

/** The six checks as six tiny marks, in tile order. */
/// The six check marks' own width (marks and their 5 px gaps), the least
/// room they take beside the figure.
const CHECK_MARKS_WIDTH = 76;

function ChecksMarks({ tiles }: { tiles: ReadonlyArray<VerdictTile> }): React.JSX.Element {
  // The strip's order: the checks that run first, the two that cannot last.
  const quiet = new Set<VerdictTile["id"]>(["echo-safety", "taint-wall"]);
  const ordered = [...tiles.filter((tile) => !quiet.has(tile.id)), ...tiles.filter((tile) => quiet.has(tile.id))];
  return (
    <span className="memory-glance__marks" aria-hidden="true">
      {ordered.map((tile) => (
        <StatusMark key={tile.id} tone={verdictTone(tile.status)} />
      ))}
    </span>
  );
}

export function MemoryGlance({
  inputs,
  onOpenRecords,
  onOpenReview,
  onOpenGiven,
  onOpenChecks,
}: {
  inputs: GlanceInputs;
  onOpenRecords: () => void;
  onOpenReview: () => void;
  onOpenGiven: () => void;
  onOpenChecks: () => void;
}): React.JSX.Element {
  const { totals, now } = inputs;
  const perDay = React.useMemo(() => recordsPerDay(inputs.readRecords, now), [inputs.readRecords, now]);
  const writtenThisWeek = React.useMemo(
    () =>
      countSince(
        inputs.readRecords.map(recordWrittenAt).filter((at): at is number => at !== null),
        now,
        7 * DAY_MS,
      ),
    [inputs.readRecords, now],
  );
  const perHour = React.useMemo(() => injectionsPerHour(inputs.injections, now), [inputs.injections, now]);
  const givenToday = perHour.reduce((sum, count) => sum + count, 0);
  const givenAgents = new Set(
    inputs.injections.filter((entry) => entry.at_ms >= now - 24 * HOUR_MS).map((entry) => entry.identity),
  ).size;
  const holding = inputs.tiles.filter((tile) => tile.status === "holding").length;
  const review = inputs.quarantined + inputs.waiting;
  const reviewTone: Tone | undefined = !inputs.canReviewQuarantine ? "locked" : review > 0 ? "warn" : undefined;
  const lastHourLive = perHour.length > 0 && perHour[perHour.length - 1] > 0;
  return (
    <GlanceStrip ariaLabel="Memory at a glance" testId="memory-glance" className="memory-glance">
      <FigureTile
        testId="memory-glance:records"
        hero
        label="Records"
        loading={!inputs.loaded}
        value={inputs.recordsDenied ? "-" : totals.active}
        unit={inputs.recordsDenied ? undefined : "active"}
        meter={
          inputs.recordsDenied
            ? undefined
            : // The kit's responsive meter: the bars take the room beside
              // the figure (or under it), never more than their own 128 px.
              ({ width }: { width: number }) => (
                <Sparkline
                  className="memory-glance__trend"
                  values={perDay}
                  kind="bars"
                  width={Math.min(128, width)}
                  height={26}
                  label={`${plural(perDay.reduce((sum, count) => sum + count, 0), "record")} written in the last 30 days`}
                />
              )
        }
        foot={
          inputs.recordsDenied
            ? "No access to records"
            : writtenThisWeek > 0
              ? `+${writtenThisWeek} written this week`
              : "None written this week"
        }
        title="Open the records"
        onActivate={onOpenRecords}
      />
      <FigureTile
        testId="memory-glance:review"
        label="Needs review"
        tone={reviewTone}
        loading={!inputs.loaded}
        value={inputs.canReviewQuarantine ? review : "-"}
        foot={
          inputs.canReviewQuarantine
            ? review > 0
              ? `${inputs.quarantined} quarantined · ${inputs.waiting} waiting`
              : "Nothing waits for you"
            : "No access to the quarantine queue"
        }
        title="Open the pipeline"
        onActivate={onOpenReview}
      />
      <FigureTile
        testId="memory-glance:given"
        label="Given to agents · 24h"
        loading={!inputs.loaded}
        value={inputs.injectionsDenied ? "-" : givenToday}
        trend={inputs.injectionsDenied ? undefined : perHour}
        trendLive={lastHourLive}
        trendLabel={`Memory given to agents ${plural(givenToday, "time")} in the last 24 hours`}
        foot={
          inputs.injectionsDenied
            ? "No access to the injection history"
            : givenToday > 0
              ? `to ${plural(givenAgents, "agent")}`
              : "None in the last day"
        }
        title="Open Knowledge"
        onActivate={onOpenGiven}
      />
      <FigureTile
        testId="memory-glance:checks"
        label="Checks"
        loading={!inputs.loaded}
        value={holding}
        of={inputs.tiles.length}
        // Six marks, never squeezed: under their own width they go below
        // the figure.
        meter={() => <ChecksMarks tiles={inputs.tiles} />}
        meterMinWidth={CHECK_MARKS_WIDTH}
        foot="holding"
        title="Open the checks"
        onActivate={onOpenChecks}
      />
    </GlanceStrip>
  );
}
