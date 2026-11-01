// The Records Timeline: records grouped by the day they last changed, as
// compact cards on a spine (newest first), with the versions of one memory
// linked by short arcs in the spine's gutter when both are on screen.

import React from "react";

import type { MemoryPanelRecord } from "../../types";
import { RelativeTime, StatusPill, TrustLadder, cx, formatClockTime, plural } from "./kit";
import { realmOfRecord } from "./model";
import { Highlight, KindMark, statusMarkTone, useNameOf } from "./parts";
import { recordChangedAt, recordsByDay, supersedeLinks, trustLevel } from "./viz-model";
import { kindLabel, scopeHeading, statusTitle, statusWord, trustDisplay } from "./words";

/// The spine's x inside the timeline (px); arcs bend left of it.
const SPINE_X = 30;

export function RecordsTimeline({
  records,
  now,
  openId,
  query,
  onSelectRecord,
}: {
  records: ReadonlyArray<MemoryPanelRecord>;
  now: number;
  openId?: string;
  query?: string;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const groups = React.useMemo(() => recordsByDay(records, now), [records, now]);
  const links = React.useMemo(() => supersedeLinks(records), [records]);
  const rootRef = React.useRef<HTMLDivElement | null>(null);
  const [arcs, setArcs] = React.useState<Array<{ key: string; d: string }>>([]);

  React.useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root || links.length === 0) {
      setArcs([]);
      return;
    }
    const measure = () => {
      const top = root.getBoundingClientRect().top;
      const yOf = (id: string): number | null => {
        const dot = root.querySelector<HTMLElement>(`[data-dot-for="${CSS.escape(id)}"]`);
        if (!dot) return null;
        const box = dot.getBoundingClientRect();
        return box.top - top + box.height / 2;
      };
      const next: Array<{ key: string; d: string }> = [];
      links.forEach((link, index) => {
        const y1 = yOf(link.newer);
        const y2 = yOf(link.older);
        if (y1 === null || y2 === null || y1 === y2) return;
        const bend = SPINE_X - 12 - (index % 3) * 4;
        next.push({
          key: `${link.newer}>${link.older}`,
          d: `M${SPINE_X - 5} ${y1}C${bend} ${y1} ${bend} ${y2} ${SPINE_X - 5} ${y2}`,
        });
      });
      setArcs(next);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => measure());
    observer.observe(root);
    return () => observer.disconnect();
  }, [links, groups]);

  return (
    <div className="memory-timeline" ref={rootRef} data-testid="memory-records-timeline">
      <svg className="memory-timeline__arcs" aria-hidden="true" focusable="false">
        {arcs.map((arc) => (
          <path key={arc.key} className="memory-timeline__arc" d={arc.d} />
        ))}
      </svg>
      {groups.map((group) => (
        <section className="memory-timeline__day" key={group.key} aria-label={`${group.label}, ${plural(group.records.length, "record")}`}>
          <h3 className="memory-timeline__day-label">
            <span>{group.label}</span>
            <span className="memory-timeline__day-count">{group.records.length}</span>
          </h3>
          <ol className="memory-timeline__items">
            {group.records.map((record) => {
              const at = recordChangedAt(record);
              const status = record.status?.status;
              const replaces = record.supersedes && records.some((other) => other.id === record.supersedes);
              return (
                <li className="memory-timeline__item" key={record.id}>
                  <span
                    className="memory-timeline__dot"
                    data-dot-for={record.id}
                    data-status={status}
                    aria-hidden="true"
                  />
                  <button
                    type="button"
                    className={cx("memory-timeline__card", record.id === openId && "is-selected")}
                    data-testid={`memory-record:${record.id}`}
                    aria-current={record.id === openId ? "true" : undefined}
                    title={record.title || undefined}
                    onClick={() => onSelectRecord(realmOfRecord(record), record.id)}
                  >
                    <span className="memory-timeline__time">{at ? formatClockTime(at) : ""}</span>
                    <KindMark kind={record.kind} />
                    <span className="memory-timeline__main">
                      <span className="memory-timeline__title">
                        <Highlight text={record.title || "Untitled record"} query={query} />
                      </span>
                      <span className="memory-timeline__facts">
                        <span>{kindLabel(record.kind)}</span>
                        <TrustLadder level={trustLevel(record.trust)} label={trustDisplay(record.trust)} />
                        <span className="memory-timeline__scope">{scopeHeading(record.scope, nameOf)}</span>
                        {replaces ? <span className="memory-timeline__replaces">replaces an earlier version</span> : null}
                      </span>
                    </span>
                    <span className="memory-timeline__end">
                      {status && status !== "active" ? (
                        <StatusPill tone={statusMarkTone(record.status)} variant="quiet" title={statusTitle(record.status)}>
                          {statusWord(record.status)}
                        </StatusPill>
                      ) : null}
                      {at ? <RelativeTime value={at} className="memory-timeline__ago" /> : null}
                    </span>
                  </button>
                </li>
              );
            })}
          </ol>
        </section>
      ))}
    </div>
  );
}
