// Knowledge (L1): what one agent can draw on (its own memory, team,
// operator and shared memory, as one stacked bar and as rows), and the
// memory given to agents over the last week (one dot strip per agent, the
// picked agent first), with the full injection list under it.

import React from "react";

import type { MemoryPanelRecord } from "../../types";
import {
  AgentName,
  Chip,
  DotStrip,
  EmptyState,
  ListRow,
  RelativeTime,
  RowList,
  SectionHeader,
  SelectField,
  Toolbar,
  cx,
  formatAbsoluteTime,
  formatRelativeTime,
  joinMeta,
  plural,
} from "./kit";
import {
  knowledgeComposition,
  knowledgeSegmentTitle,
  type AnnotatedInjection,
  type MemoryRecordsFilter,
} from "./model";
import { RecordLink, useMeasuredWidth, useNameOf } from "./parts";
import { DAY_MS, injectionDots } from "./viz-model";
import { injectionMoment } from "./words";

const WEEK_DAYS = 7;

/// The four places an agent's knowledge comes from, deepest ink for its own.
const SEGMENT_STEP: Record<string, string> = { identity: "own", mob: "team", operator: "operator", realm: "shared" };

function DrawOnBar({
  segments,
  agentName,
}: {
  segments: ReturnType<typeof knowledgeComposition>;
  agentName: string;
}): React.JSX.Element {
  const total = segments.reduce((sum, segment) => sum + segment.count, 0);
  const words = segments
    .filter((segment) => segment.count > 0)
    .map((segment) => `${knowledgeSegmentTitle(segment, agentName)} ${segment.count}`)
    .join(", ");
  return (
    <div className="memory-drawon">
      <span className="memory-drawon__bar" role="img" aria-label={total > 0 ? words : "Nothing to draw on yet"}>
        {segments.map((segment) =>
          segment.count > 0 ? (
            <span
              key={segment.label}
              className="memory-drawon__seg"
              data-step={SEGMENT_STEP[segment.filter.scope ?? ""] ?? "shared"}
              style={{ flexGrow: segment.count }}
            />
          ) : null,
        )}
      </span>
      <span className="memory-drawon__legend" aria-hidden="true">
        {segments.map((segment) => (
          <span key={segment.label} className="memory-drawon__key" data-step={SEGMENT_STEP[segment.filter.scope ?? ""] ?? "shared"}>
            <span className="memory-drawon__swatch" />
            <span className="memory-drawon__word">{knowledgeSegmentTitle(segment, agentName)}</span>
            <span className="memory-drawon__count">{segment.count}</span>
          </span>
        ))}
      </span>
    </div>
  );
}

/// One agent's week of memory given, as dots: filled during a turn, hollow
/// as the agent started.
function GivenLane({
  identity,
  injections,
  now,
  picked,
  width,
  recordTitles,
  onSelectRecord,
}: {
  identity: string;
  injections: ReadonlyArray<AnnotatedInjection>;
  now: number;
  picked: boolean;
  /** The strip's width in px (measured once for every lane). */
  width: number;
  recordTitles: ReadonlyMap<string, string>;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const name = nameOf(identity) || identity;
  const dots = injectionDots(injections.map((entry) => entry.entry), identity, now, WEEK_DAYS);
  const realms = new Map(dots.map((dot) => [dot.key, dot.realm]));
  const byKey = new Map(dots.map((dot) => [dot.key, dot]));
  return (
    <div className={cx("memory-given-lane", picked && "is-picked")} data-testid={`memory-knowledge-given:${identity}`}>
      <span className="memory-given-lane__who">
        <AgentName id={identity} link />
      </span>
      <DotStrip
        className="memory-given-lane__dots"
        items={dots.map((dot) => ({
          id: dot.key,
          t: dot.at,
          shape: dot.surface === "build" ? "hollow" : "dot",
          label: joinMeta(
            recordTitles.get(dot.recordId) ?? "A record",
            dot.surface === "build" ? "given at startup" : "given during a turn",
            formatRelativeTime(dot.at, now),
          ),
        }))}
        start={now - WEEK_DAYS * DAY_MS}
        end={now}
        width={width}
        height={22}
        markSize={7}
        label={`Memory given to ${name}: ${plural(dots.length, "time")} in the last week`}
        onSelect={(item) => {
          const dot = byKey.get(item.id);
          if (dot) onSelectRecord(realms.get(item.id), dot.recordId);
        }}
      />
      <span className="memory-given-lane__count">{dots.length}</span>
    </div>
  );
}

export function KnowledgeTab({
  records,
  identities,
  selectedIdentity,
  onSelectIdentity,
  annotatedInjections,
  injectionsDenied,
  recordsDenied,
  recordTitles,
  now,
  onOpenRecordsFiltered,
  onSelectRecord,
}: {
  records: MemoryPanelRecord[];
  identities: string[];
  selectedIdentity: string;
  onSelectIdentity: (identity: string) => void;
  annotatedInjections: AnnotatedInjection[];
  injectionsDenied: boolean;
  recordsDenied: boolean;
  recordTitles: ReadonlyMap<string, string>;
  now: number;
  onOpenRecordsFiltered: (filter: MemoryRecordsFilter) => void;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const agentName = selectedIdentity ? nameOf(selectedIdentity) || selectedIdentity : "";
  const segments = selectedIdentity ? knowledgeComposition(records, selectedIdentity) : [];
  const total = segments.reduce((sum, segment) => sum + segment.count, 0);
  const weekStart = now - WEEK_DAYS * DAY_MS;
  const [axisRef, axisWidth] = useMeasuredWidth<HTMLSpanElement>(420);
  // Agents given memory this week, the picked agent first.
  const givenAgents = React.useMemo(() => {
    const recent = new Set<string>();
    for (const { entry } of annotatedInjections) if (entry.at_ms >= weekStart) recent.add(entry.identity);
    const rest = Array.from(recent).filter((identity) => identity !== selectedIdentity).sort((a, b) => a.localeCompare(b));
    return selectedIdentity ? [selectedIdentity, ...rest] : rest;
  }, [annotatedInjections, selectedIdentity, weekStart]);
  return (
    <div className="memory-tab memory-knowledge" data-testid="memory-knowledge">
      {identities.length === 0 ? (
        <EmptyState
          inline
          kind={recordsDenied ? "no-access" : "empty"}
          title={recordsDenied ? "Records: no access." : "No agent memory is loaded yet."}
        />
      ) : (
        <>
          <Toolbar className="memory-toolbar memory-knowledge__picker">
            <SelectField
              label="Agent"
              value={selectedIdentity}
              options={identities.map((identity) => ({ value: identity, label: nameOf(identity) || identity }))}
              testId="memory-knowledge-identity"
              onChange={(value) => onSelectIdentity(value)}
            />
          </Toolbar>
          <section className="memory-block" aria-labelledby="memory-drawon-head">
            <SectionHeader id="memory-drawon-head" label={`What ${agentName} can draw on`} count={total} />
            <DrawOnBar segments={segments} agentName={agentName} />
            <RowList>
              {segments.map((segment) => (
                <ListRow
                  key={segment.label}
                  className="memory-segment-row"
                  testId={`memory-knowledge-segment:${segment.label}`}
                  onActivate={() => onOpenRecordsFiltered(segment.filter)}
                  lead={<span className="memory-drawon__swatch" data-step={SEGMENT_STEP[segment.filter.scope ?? ""] ?? "shared"} aria-hidden="true" />}
                  title={knowledgeSegmentTitle(segment, agentName)}
                  subtitle={segment.approximate ? "Every record of this kind" : "Only this agent's records"}
                  meta={<span className="memory-count">{plural(segment.count, "record")}</span>}
                />
              ))}
            </RowList>
            <p className="memory-caption">
              Team, operator and shared counts include every record of that kind, because this
              console cannot tell which teams an agent belongs to.
            </p>
          </section>
        </>
      )}
      <section className="memory-block" data-testid="memory-knowledge-history" aria-labelledby="memory-given-head">
        <SectionHeader
          id="memory-given-head"
          label={<span title="Injection history">Memory given to agents</span>}
          count={!injectionsDenied && annotatedInjections.length > 0 ? annotatedInjections.length : null}
          action={
            !injectionsDenied && givenAgents.length > 0 ? (
              <span className="memory-given__key" aria-hidden="true">
                <span className="memory-given__dot" data-surface="turn" /> during a turn
                <span className="memory-given__dot" data-surface="build" /> at startup
              </span>
            ) : undefined
          }
        />
        {injectionsDenied ? (
          <EmptyState inline kind="no-access" title="Memory given to agents: no access." />
        ) : annotatedInjections.length === 0 ? (
          <EmptyState inline title="No memory has been given to an agent yet." />
        ) : (
          <>
            {givenAgents.length > 0 ? (
              <div className="memory-given-week">
                <div className="memory-given__axis" aria-hidden="true">
                  <span className="memory-given__axis-track" ref={axisRef}>
                    <span>7 days ago</span>
                    <span>now</span>
                  </span>
                </div>
                {givenAgents.map((identity) => (
                  <GivenLane
                    key={identity}
                    identity={identity}
                    injections={annotatedInjections}
                    now={now}
                    picked={identity === selectedIdentity}
                    width={axisWidth}
                    recordTitles={recordTitles}
                    onSelectRecord={onSelectRecord}
                  />
                ))}
              </div>
            ) : null}
            <RowList>
              {annotatedInjections.map(({ entry, dup }, index) => (
                <ListRow
                  key={`inj-${index}`}
                  className="memory-injection-row"
                  testId={`memory-injection:${index}`}
                  lead={<span className="memory-given" data-surface={entry.surface === "build" ? "build" : "turn"} aria-hidden="true" />}
                  title={
                    <RecordLink
                      id={entry.record_id}
                      realm={entry.realm}
                      titles={recordTitles}
                      testId={`memory-injection-record:${index}`}
                      onOpen={onSelectRecord}
                    />
                  }
                  subtitle={
                    <>
                      {"Given to "}
                      <AgentName id={entry.identity} link />
                      {` ${injectionMoment(entry.surface)}`}
                    </>
                  }
                  meta={
                    <>
                      {dup ? (
                        <Chip
                          tone="warn"
                          testId={`memory-injection-dup:${index}`}
                          title="The agent was given the same record in its previous injection too."
                        >
                          Repeated
                        </Chip>
                      ) : null}
                      <RelativeTime
                        value={entry.at_ms}
                        title={joinMeta(formatAbsoluteTime(entry.at_ms), entry.session_key ? `session ${entry.session_key}` : null)}
                      />
                    </>
                  }
                />
              ))}
            </RowList>
          </>
        )}
      </section>
      <p className="memory-caption memory-footnote">
        <span data-testid="memory-knowledge-as-injected">The exact memory text given to an agent is not shown here.</span>{" "}
        <span data-testid="memory-knowledge-budget">Per-session memory budgets are not shown here either.</span>
      </p>
    </div>
  );
}
