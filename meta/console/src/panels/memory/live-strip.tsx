// The live memory events feed at the foot of the Pipeline tab: names and
// record titles lead, the exact technical line is the tooltip, and the
// follow-live discipline holds (scrolling away freezes the list; "N behind"
// resumes; the top auto-unfreezes).

import React from "react";

import { describeMemoryTimelineEvent } from "../../lib/adapters";
import { describeMemoryEventForPeople } from "../../lib/memory-event-words";
import type { ConsoleFrame } from "../../types";
import {
  Button,
  EmptyState,
  ListRow,
  RelativeTime,
  SectionHeader,
  agentDisplayName,
  useAgentDirectory,
} from "./kit";
import { countFramesBehind, dedupeFramesById, memoryFramePivot } from "./model";

/// Exported for the jsdom component-interaction lane, which pins the
/// pause-on-scroll / N-behind / auto-unfreeze discipline.
export function MemoryLiveStrip({
  frames,
  recordTitles,
  onPivot,
}: {
  frames: ConsoleFrame[];
  /** Record titles by memory id, so events name records people know. */
  recordTitles?: ReadonlyMap<string, string>;
  onPivot: (realm: string | undefined, recordId: string) => void;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  // Live-follow discipline: scrolling away from the top freezes the visible
  // list; "N behind · Jump to live" resumes. Frames dedupe by id so
  // snapshot_complete replays never double-render.
  const deduped = React.useMemo(() => dedupeFramesById(frames), [frames]);
  const [frozen, setFrozen] = React.useState<ConsoleFrame[] | null>(null);
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const shown = frozen ?? deduped;
  const behind = frozen ? countFramesBehind(deduped, frozen) : 0;

  function handleScroll(): void {
    const el = listRef.current;
    if (!el) return;
    if (el.scrollTop > 4) {
      setFrozen((current) => current ?? deduped);
    } else if (frozen && behind === 0) {
      setFrozen(null);
    }
  }

  function jumpToLive(): void {
    setFrozen(null);
    // Optional call: jsdom (the component-interaction lane) has no scrolling.
    listRef.current?.scrollTo?.({ top: 0 });
  }

  return (
    <section className="cc-section memory-live" data-testid="memory-live-strip">
      <SectionHeader
        label="Live events"
        action={
          behind > 0 ? (
            <Button size="sm" variant="ghost" data-testid="memory-live-jump" onClick={jumpToLive}>
              {`${behind} behind · Jump to live`}
            </Button>
          ) : null
        }
      />
      <p className="memory-caption">Recent memory events, newest first. Older events are not kept here.</p>
      <div className="cc-rows memory-live__list" ref={listRef} onScroll={handleScroll}>
        {shown.length === 0 ? (
          <EmptyState inline title="No memory events yet." />
        ) : (
          shown.map((frame) => {
            const data =
              frame.data && typeof frame.data === "object"
                ? (frame.data as Record<string, unknown>)
                : {};
            const pivot = memoryFramePivot(frame);
            // Names and record titles lead; the exact technical line is the
            // tooltip.
            const text = describeMemoryEventForPeople(frame.event, data, {
              agentName: (key) => agentDisplayName(key, directory),
              recordTitle: (memoryId) => recordTitles?.get(memoryId),
            });
            return (
              <ListRow
                key={frame.id}
                dense
                className="memory-live__row"
                testId={`memory-live-row:${frame.id}`}
                title={text}
                titleAttr={describeMemoryTimelineEvent(frame.event, data)}
                meta={
                  <>
                    {pivot ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        data-testid={`memory-live-pivot:${frame.id}`}
                        onClick={() => onPivot(pivot.realm, pivot.recordId)}
                      >
                        Open record
                      </Button>
                    ) : null}
                    <RelativeTime value={frame.timestampMs} />
                  </>
                }
              />
            );
          })
        )}
        {/* The ring keeps 1024 frames per identity, 4096 in total; anything
            older is unknowable live. The seam keeps "no events" from being
            mistaken for "nothing happened". */}
        <div className="memory-live__seam" data-testid="memory-live-seam">
          Live history starts here
        </div>
      </div>
    </section>
  );
}
