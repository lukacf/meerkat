// Dreams (L1): the memory steward's dream runs of the last week as a
// timeline (a bar per run, its changes above it, quarantined changes as a
// warning segment), then each run's verdict sheet and the earlier runs the
// audit log keeps.

import React from "react";

import type { MemoryDreamRun, MemoryDreamRunSheet } from "../../types";
import {
  Chip,
  EmptyState,
  KitGlyph,
  ListRow,
  Notice,
  RelativeTime,
  RowList,
  SectionHeader,
  TechnicalDetails,
  cx,
  formatAbsoluteTime,
  formatDuration,
  formatRelativeTime,
  humanizeEnum,
  joinMeta,
  plural,
} from "./kit";
import { DREAMS_WITHHELD, normalizeDreamRunDetail } from "./model";
import { MetaLine, RecordLink, useMeasuredWidth, useNameOf } from "./parts";
import { DAY_MS, dreamTimeline, type DreamBar } from "./viz-model";
import { dreamOpKindsDisplay, dreamVerdictChips, partitionName, realmSpaceName } from "./words";

const WEEK = 7;

/// A day label's width in the axis font, with its 4 px offset from the line,
/// and the room the "now" label takes at the right edge.
const DAY_LABEL_PX = 44;
const NOW_LABEL_PX = 28;

/// Midnights between `start` and `end`, as gridlines with day labels. A
/// label is left off where it would run into "now" at the right edge, and
/// every other (or third) label is left off where the days stand closer
/// than a label is wide, counted from the newest; every day keeps its line.
function dayTicks(start: number, end: number, width: number): Array<{ x: number; label: string; key: string }> {
  const ticks: Array<{ x: number; label: string; key: string }> = [];
  const first = new Date(start);
  first.setHours(0, 0, 0, 0);
  const cursor = new Date(first);
  if (cursor.getTime() < start) cursor.setDate(cursor.getDate() + 1);
  const span = Math.max(1, end - start);
  while (cursor.getTime() <= end) {
    const at = cursor.getTime();
    ticks.push({
      key: String(at),
      x: (at - start) / span,
      label: cursor.toLocaleDateString("en-GB", { weekday: "short", day: "numeric" }),
    });
    cursor.setDate(cursor.getDate() + 1);
  }
  const daySpacing = (DAY_MS / span) * width;
  const every = daySpacing >= DAY_LABEL_PX + 6 ? 1 : daySpacing * 2 >= DAY_LABEL_PX + 6 ? 2 : 3;
  let kept = 0;
  for (let index = ticks.length - 1; index >= 0; index -= 1) {
    const tick = ticks[index];
    if ((1 - tick.x) * width < DAY_LABEL_PX + NOW_LABEL_PX) {
      tick.label = "";
      continue;
    }
    if (kept % every !== 0) tick.label = "";
    kept += 1;
  }
  return ticks;
}

/// The week of dream runs: one row per space, a bar per run.
function DreamsTimeline({
  sheets,
  runs,
  now,
  multiRealm,
  withheld,
  onOpenRun,
}: {
  sheets: ReadonlyArray<MemoryDreamRunSheet>;
  runs: ReadonlyArray<MemoryDreamRun>;
  now: number;
  multiRealm: boolean;
  /** Access withheld some runs: an empty week says so. */
  withheld: boolean;
  onOpenRun: (bar: DreamBar) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const timeline = React.useMemo(() => dreamTimeline(sheets, runs, now, WEEK), [sheets, runs, now]);
  const [axisRef, axisWidth] = useMeasuredWidth<HTMLSpanElement>(640);
  const ticks = React.useMemo(() => dayTicks(timeline.start, timeline.end, axisWidth), [timeline.start, timeline.end, axisWidth]);
  const count = timeline.rows.reduce((sum, row) => sum + row.bars.length, 0);
  return (
    <section className="memory-block memory-dreams-timeline" data-testid="memory-dreams-timeline" aria-labelledby="memory-dreams-week-head">
      <SectionHeader
        id="memory-dreams-week-head"
        label="The last 7 days"
        count={count > 0 ? count : null}
        action={timeline.older > 0 ? <span className="memory-block__meta">{`${plural(timeline.older, "earlier run")} below`}</span> : undefined}
      />
      {count === 0 && withheld ? (
        <WithheldState testId="memory-dreams-week-withheld" />
      ) : count === 0 ? (
        <EmptyState inline title="No dream ran this week." body="The memory steward dreams on its schedule; runs show here as bars." />
      ) : (
        <div className="memory-week">
          <div className="memory-week__axis" aria-hidden="true">
            <span className="memory-week__axis-label" />
            <span className="memory-week__axis-days" ref={axisRef}>
              {ticks.map((tick) =>
                tick.label ? (
                  <span key={tick.key} className="memory-week__day" style={{ left: `${tick.x * 100}%` }}>
                    {tick.label}
                  </span>
                ) : null,
              )}
              <span className="memory-week__now">now</span>
            </span>
          </div>
          {timeline.rows.map((row) => (
            <div className="memory-week__row" key={row.realm}>
              <span className="memory-week__row-label" title={multiRealm ? `Realm ${row.realm}` : undefined}>
                {multiRealm ? realmSpaceName(row.realm) : "Dreams"}
              </span>
              <span className="memory-week__track">
                {ticks.map((tick) => (
                  <span key={tick.key} className="memory-week__gridline" style={{ left: `${tick.x * 100}%` }} aria-hidden="true" />
                ))}
                {row.bars.map((bar) => {
                  const who = partitionName(bar.partition, nameOf);
                  const when = formatRelativeTime(bar.end, now);
                  const label = joinMeta(
                    who ? `Dream for ${who}` : "Dream",
                    bar.ops !== null ? plural(bar.ops, "change") : null,
                    bar.quarantined > 0 ? `${bar.quarantined} quarantined` : null,
                    when,
                  );
                  return (
                    <button
                      key={bar.runId}
                      type="button"
                      className={cx("memory-week__bar", bar.quarantined > 0 && "has-quarantine")}
                      style={{ left: `${bar.x0 * 100}%`, width: `max(6px, ${(bar.x1 - bar.x0) * 100}%)` }}
                      data-testid={`memory-dreams-bar:${bar.runId}`}
                      aria-label={label}
                      title={label}
                      onClick={() => onOpenRun(bar)}
                    >
                      <span className="memory-week__bar-label" aria-hidden="true">
                        {bar.ops !== null ? bar.ops : ""}
                      </span>
                      {bar.quarantined > 0 ? <span className="memory-week__bar-q" aria-hidden="true" /> : null}
                    </button>
                  );
                })}
              </span>
            </div>
          ))}
          <div className="memory-week__foot" aria-hidden="true">
            <span className="memory-legend" data-part="active">a run, with the changes it committed</span>
            <span className="memory-legend" data-part="quarantined">quarantined changes</span>
          </div>
        </div>
      )}
    </section>
  );
}

/// The steward's phases as a short connected strip: each phase a dot.
function PhaseStrip({ phases }: { phases: Array<[string, string]> }): React.JSX.Element | null {
  if (phases.length === 0) return null;
  return (
    <span className="memory-phase-strip" aria-label={`Phases: ${phases.map(([name]) => humanizeEnum(name).toLowerCase()).join(", ")}`} role="img">
      {phases.map(([name], index) => (
        <React.Fragment key={`${name}:${index}`}>
          {index > 0 ? <span className="memory-phase-strip__line" /> : null}
          <span className="memory-phase-strip__dot" title={humanizeEnum(name)} />
        </React.Fragment>
      ))}
    </span>
  );
}

function DreamRunRow({
  run,
  expanded,
  onToggle,
}: {
  run: MemoryDreamRunSheet;
  expanded: boolean;
  onToggle: () => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const detail = normalizeDreamRunDetail(run.detail);
  const who = partitionName(run.partition, nameOf);
  const at = run.completed_at_ms || run.started_at_ms;
  const duration = run.started_at_ms && run.completed_at_ms ? formatDuration(run.completed_at_ms - run.started_at_ms) : "";
  const chips = dreamVerdictChips(detail.verdicts);
  return (
    <div className={cx("memory-dream-run", expanded && "is-open")} data-testid={`memory-dream-run:${run.run_id}`} id={`memory-dream-run-${run.run_id}`}>
      <ListRow
        className="memory-dream-run__head"
        testId={`memory-dream-run-toggle:${run.run_id}`}
        onActivate={onToggle}
        ariaExpanded={expanded}
        lead={<span className="memory-disclosure" data-open={expanded ? "true" : undefined} aria-hidden="true" />}
        title={who ? `Dream for ${who}` : "Dream"}
        subtitle={
          <MetaLine
            parts={[
              at ? <RelativeTime value={at} /> : null,
              duration,
              typeof run.ops_committed === "number" ? plural(run.ops_committed, "change") : null,
            ]}
          />
        }
        meta={<PhaseStrip phases={detail.phases} />}
      >
        {chips.length > 0 ? (
          <span className="memory-dream-run__chips">
            {chips.map((chip) => (
              <Chip key={chip.key} tone={chip.tone} variant="quiet" mark={chip.tone === "warn"} title={chip.key}>
                {chip.label}
              </Chip>
            ))}
          </span>
        ) : null}
      </ListRow>
      {expanded ? (
        <div className="memory-dream-run__detail" data-testid={`memory-dream-run-detail:${run.run_id}`}>
          {detail.raw !== null ? (
            <Notice>The details of this run are stored in a form this console cannot read.</Notice>
          ) : (
            <>
              {detail.phases.length > 0 ? (
                <ol className="memory-phases" aria-label="Phases">
                  {detail.phases.map(([name, note], index) => (
                    <li className="memory-phases__step" key={`ph-${index}`}>
                      <KitGlyph name="check" className="memory-phases__mark" />
                      <span className="memory-phases__name">{humanizeEnum(name)}</span>
                      {note ? <span className="memory-phases__note">{note}</span> : null}
                    </li>
                  ))}
                </ol>
              ) : (
                <p className="memory-caption">No phases were recorded.</p>
              )}
              {detail.verdicts.length === 0 ? <p className="memory-caption">No changes were counted.</p> : null}
              {detail.skips.map((skip, index) => (
                <Notice tone="warn" key={`sk-${index}`}>
                  {skip}
                </Notice>
              ))}
            </>
          )}
          <TechnicalDetails
            items={[
              { label: "Run ID", value: run.run_id },
              { label: "Partition", value: run.partition },
              { label: "Realm", value: run.realm, plain: true },
              { label: "Started", value: formatAbsoluteTime(run.started_at_ms), plain: true },
              { label: "Completed", value: formatAbsoluteTime(run.completed_at_ms), plain: true },
              detail.raw !== null ? { label: "Stored details", value: detail.raw, plain: true } : null,
            ]}
          />
        </div>
      ) : null}
    </div>
  );
}

function AuditRunRow({
  run,
  partition,
  recordTitles,
  onOpenRecord,
}: {
  run: MemoryDreamRun;
  partition: string | undefined;
  recordTitles: ReadonlyMap<string, string>;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const who = partitionName(partition, nameOf);
  const at = run.last_op_at_ms || run.first_op_at_ms;
  const span =
    run.first_op_at_ms && run.last_op_at_ms && run.last_op_at_ms > run.first_op_at_ms
      ? formatDuration(run.last_op_at_ms - run.first_op_at_ms)
      : "";
  const touched = run.memory_ids || [];
  const rationales = run.rationales || [];
  return (
    <ListRow
      className="memory-audit-run"
      testId={`memory-dream:${run.run_id}`}
      title={who ? `Dream for ${who}` : "Dream run"}
      titleAttr={`Dream run ${run.run_id}`}
      subtitle={
        <MetaLine
          parts={[
            at ? <RelativeTime value={at} /> : null,
            span,
            typeof run.ops === "number" ? plural(run.ops, "change") : null,
            dreamOpKindsDisplay(run.op_kinds),
          ]}
        />
      }
      meta={run.quarantined_ops && run.quarantined_ops > 0 ? <Chip tone="warn">{`${run.quarantined_ops} quarantined`}</Chip> : undefined}
    >
      {touched.length > 0 || rationales.length > 0 ? (
        <>
          {touched.length > 0 ? (
            <span className="memory-touched">
              <span className="memory-touched__label">Touched</span>
              {touched.map((memoryId) => (
                <RecordLink
                  key={memoryId}
                  className="memory-touched__record"
                  id={memoryId}
                  realm={run.realm}
                  titles={recordTitles}
                  testId={`memory-dream-record:${run.run_id}:${memoryId}`}
                  onOpen={onOpenRecord}
                />
              ))}
            </span>
          ) : null}
          {Array.from(new Set(rationales)).map((rationale, index) => (
            <span className="memory-row-note memory-audit-run__note" key={`r-${index}`}>
              {rationale}
            </span>
          ))}
        </>
      ) : null}
    </ListRow>
  );
}

/// A dream read that access withheld: said calmly, never as "no dreams".
function WithheldState({ testId }: { testId: string }): React.JSX.Element {
  return (
    <EmptyState
      inline
      kind="no-access"
      testId={testId}
      title="Withheld by access."
      body={DREAMS_WITHHELD}
    />
  );
}

export function DreamsTab({
  dreams,
  dreamsDenied,
  dreamsWithheld = false,
  dreamRunsWithheld = false,
  dreamSheets,
  dreamRunsDenied,
  sheetPartitions,
  recordTitles,
  multiRealm,
  now,
  onSelectRecord,
}: {
  dreams: MemoryDreamRun[];
  dreamsDenied: boolean;
  dreamsWithheld?: boolean;
  dreamRunsWithheld?: boolean;
  dreamSheets: MemoryDreamRunSheet[];
  dreamRunsDenied: boolean;
  sheetPartitions: ReadonlyMap<string, string | undefined>;
  recordTitles: ReadonlyMap<string, string>;
  multiRealm: boolean;
  now: number;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const [expandedRuns, setExpandedRuns] = React.useState<Record<string, boolean>>({});
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const openRun = (bar: DreamBar) => {
    if (bar.sheet) setExpandedRuns((current) => ({ ...current, [bar.runId]: true }));
    const target = rootRef.current?.querySelector<HTMLElement>(
      bar.sheet ? `[data-testid="memory-dream-run:${CSS.escape(bar.runId)}"]` : `[data-testid="memory-dream:${CSS.escape(bar.runId)}"]`,
    );
    target?.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
  };
  return (
    <div className="memory-tab memory-dreams" ref={rootRef}>
      {dreamRunsDenied && dreamsDenied ? null : (
        <DreamsTimeline
          sheets={dreamRunsDenied ? [] : dreamSheets}
          runs={dreams}
          now={now}
          multiRealm={multiRealm}
          withheld={dreamsWithheld || dreamRunsWithheld}
          onOpenRun={openRun}
        />
      )}
      <section className="memory-block" data-testid="memory-dream-runs" aria-labelledby="memory-dream-runs-head">
        <SectionHeader
          id="memory-dream-runs-head"
          label="Dream runs"
          count={!dreamRunsDenied && dreamSheets.length > 0 ? dreamSheets.length : null}
        />
        {dreamRunsDenied ? (
          <EmptyState inline kind="no-access" title="Dream runs: no access." />
        ) : dreamSheets.length === 0 && dreamRunsWithheld ? (
          <WithheldState testId="memory-dream-runs-withheld" />
        ) : dreamSheets.length === 0 ? (
          <EmptyState inline title="No dream run is recorded yet." body="Older runs may still appear in the audit log below." />
        ) : (
          <RowList>
            {dreamSheets.map((run) => (
              <DreamRunRow
                key={run.run_id}
                run={run}
                expanded={expandedRuns[run.run_id] === true}
                onToggle={() => setExpandedRuns((current) => ({ ...current, [run.run_id]: !current[run.run_id] }))}
              />
            ))}
          </RowList>
        )}
      </section>
      <section className="memory-block" aria-labelledby="memory-audit-runs-head">
        <SectionHeader id="memory-audit-runs-head" label="Earlier runs, from the audit log" count={dreams.length > 0 ? dreams.length : null} />
        {dreams.length === 0 && dreamsWithheld && !dreamsDenied ? (
          <WithheldState testId="memory-dreams-withheld" />
        ) : dreams.length === 0 ? (
          <EmptyState
            inline
            kind={dreamsDenied ? "no-access" : "empty"}
            title={dreamsDenied ? "Dream audit: no access." : "No dream runs recorded yet."}
          />
        ) : (
          <RowList>
            {dreams.map((run) => (
              <AuditRunRow
                key={run.run_id}
                run={run}
                partition={sheetPartitions.get(run.run_id)}
                recordTitles={recordTitles}
                onOpenRecord={onSelectRecord}
              />
            ))}
          </RowList>
        )}
      </section>
    </div>
  );
}

export { DAY_MS };
