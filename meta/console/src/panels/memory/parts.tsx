// Small pieces every Memory view shares: names for keys, the kind mark, a
// record's facts line, record links, highlighted search hits and the record
// row of the List view.

import React from "react";

import type { MemoryPanelRecord, MemoryRecordKind } from "../../types";
import {
  AgentName,
  Chip,
  EntityRow,
  KitGlyph,
  RelativeTime,
  Sparkline,
  StatusPill,
  TechnicalId,
  TrustLadder,
  agentDisplayName,
  cx,
  useAgentDirectory,
  type KitGlyphName,
  type MarkTone,
} from "./kit";
import { recordUtility, utilityDisplay, type MemoryRecordDetail } from "./model";
import type { QuarantineNotice } from "./quarantine";
import { matchRanges, trustLevel } from "./viz-model";
import {
  kindLabel,
  scopeHeading,
  statusTitle,
  statusWord,
  targetName,
  trustDisplay,
  type NameOf,
} from "./words";

/// The width of an element, kept current as it resizes (one observer per
/// call). `fallback` stands in until the element is laid out (and in test
/// environments without layout).
export function useMeasuredWidth<T extends HTMLElement>(fallback: number): [React.RefCallback<T>, number] {
  const [width, setWidth] = React.useState(fallback);
  const observer = React.useRef<ResizeObserver | null>(null);
  const ref = React.useCallback((element: T | null) => {
    observer.current?.disconnect();
    observer.current = null;
    if (!element) return;
    const measure = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      if (next > 0) setWidth((current) => (current === next ? current : next));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    observer.current = new ResizeObserver(() => measure());
    observer.current.observe(element);
  }, []);
  React.useEffect(() => () => observer.current?.disconnect(), []);
  return [ref, width];
}

/// A sparkline as wide as its slot, up to `maxWidth`: never a fixed width
/// that runs out of a narrow pane. The slot sets the width (CSS); the chart
/// follows it.
export function FluidSparkline({
  maxWidth,
  minWidth = 32,
  className,
  ...props
}: Omit<React.ComponentProps<typeof Sparkline>, "width"> & { maxWidth: number; minWidth?: number }): React.JSX.Element {
  const [ref, width] = useMeasuredWidth<HTMLSpanElement>(maxWidth);
  return (
    <span className={cx("memory-fluid", className)} ref={ref} style={{ maxWidth }}>
      <Sparkline {...props} width={Math.max(minWidth, Math.min(maxWidth, width))} />
    </span>
  );
}

/// Display names for agent keys, from the directory the console provides.
export function useNameOf(): NameOf {
  const directory = useAgentDirectory();
  return React.useCallback((key: string) => agentDisplayName(key, directory), [directory]);
}

/// Parts of a meta line joined with the middle dot, skipping empty ones.
export function MetaLine({ parts }: { parts: React.ReactNode[] }): React.JSX.Element {
  const shown = parts.filter(
    (part) => part !== null && part !== undefined && part !== false && part !== "",
  );
  return (
    <>
      {shown.map((part, index) => (
        <React.Fragment key={index}>
          {index > 0 ? " · " : null}
          {part}
        </React.Fragment>
      ))}
    </>
  );
}

/// Where a proposal or promotion goes, with the agent's name when it is
/// an agent's memory and the team's name (its key in the tooltip) when it
/// is a team's.
export function ScopeTarget({ kind, scopeKey }: { kind: string; scopeKey: string }): React.JSX.Element {
  if (kind === "identity") return <AgentName id={scopeKey} />;
  if (kind === "mob") return <span title={scopeKey}>{targetName(kind, scopeKey)}</span>;
  return <>{targetName(kind, scopeKey)}</>;
}

/// The kit glyph for a record kind (A2.10).
export function kindGlyph(kind: MemoryRecordKind | string | undefined): KitGlyphName {
  switch (kind) {
    case "preference":
      return "k-preference";
    case "gotcha":
      return "k-gotcha";
    case "procedure":
      return "k-procedure";
    case "relationship":
      return "k-relationship";
    case "open_loop":
      return "k-open-loop";
    case "reference":
      return "k-reference";
    default:
      return "k-fact";
  }
}

/// A record kind's mark: its glyph in muted ink.
export function KindMark({ kind, className }: { kind: MemoryRecordKind | string | undefined; className?: string }): React.JSX.Element {
  return (
    <span className={cx("memory-kind", className)} data-kind={kind} aria-hidden="true">
      <KitGlyph name={kindGlyph(kind)} />
    </span>
  );
}

/// The tone of a record status: active ok, quarantined warn, the rest quiet.
export function statusMarkTone(status: MemoryPanelRecord["status"] | undefined): MarkTone {
  switch (status?.status) {
    case "active":
      return "ok";
    case "quarantined":
      return "warn";
    default:
      return "neutral";
  }
}

/// The facts line under a record title: its kind in words, the trust
/// ladder and, when it is not active, the status. A replaced record's
/// Biography links the newer version.
export function RecordFacts({
  record,
  showActive = false,
  hideStatus = false,
  hideKind = false,
  onOpenNewer,
}: {
  record: Pick<MemoryPanelRecord, "kind" | "trust" | "status">;
  showActive?: boolean;
  hideStatus?: boolean;
  hideKind?: boolean;
  onOpenNewer?: () => void;
}): React.JSX.Element {
  const status = record.status?.status;
  return (
    <span className="memory-facts">
      {hideKind ? null : <span className="memory-facts__kind">{kindLabel(record.kind)}</span>}
      <TrustLadder level={trustLevel(record.trust)} label={trustDisplay(record.trust)} />
      {status && !hideStatus && (showActive || status !== "active") ? (
        <StatusPill tone={statusMarkTone(record.status)} variant="quiet" title={statusTitle(record.status)}>
          {statusWord(record.status)}
        </StatusPill>
      ) : null}
      {status === "superseded" && !hideStatus && onOpenNewer ? (
        <button type="button" className="memory-link" onClick={onOpenNewer}>
          Open the newer version
        </button>
      ) : null}
    </span>
  );
}

/// A record named by its title (looked up in the loaded records) that opens
/// its Biography. A record that is not loaded shows its ID, middle-truncated.
export function RecordLink({
  id,
  realm,
  titles,
  testId,
  className,
  onOpen,
}: {
  id: string;
  realm: string | undefined;
  titles: ReadonlyMap<string, string>;
  testId?: string;
  className?: string;
  onOpen: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const title = titles.get(id);
  return (
    <button
      type="button"
      className={className ? `memory-link ${className}` : "memory-link"}
      data-testid={testId}
      title={title ? id : undefined}
      onClick={() => onOpen(realm, id)}
    >
      {title ?? <TechnicalId value={id} copy={false} />}
    </button>
  );
}

/// A quarantine message: the sentence, then the IDs it concerns.
export function QuarantineNoticeText({ notice }: { notice: QuarantineNotice }): React.JSX.Element {
  return (
    <>
      {notice.title ? <span title={notice.title}>{notice.text}</span> : notice.text}
      {notice.ids.length > 0 ? (
        <span className="memory-notice-ids">
          {notice.ids.map((id) => (
            <span className="memory-notice-ids__item" key={`${id.label}:${id.value}`}>
              <span className="memory-notice-ids__label">{id.label}</span>
              <TechnicalId value={id.value} label={id.label} />
            </span>
          ))}
        </span>
      ) : null}
    </>
  );
}

/// Text with every match of the search highlighted.
export function Highlight({ text, query }: { text: string; query?: string }): React.JSX.Element {
  const ranges = query ? matchRanges(text, query) : [];
  if (ranges.length === 0) return <>{text}</>;
  const parts: React.ReactNode[] = [];
  let at = 0;
  ranges.forEach(([start, end], index) => {
    if (start > at) parts.push(text.slice(at, start));
    parts.push(
      <mark className="memory-hit" key={index}>
        {text.slice(start, end)}
      </mark>,
    );
    at = end;
  });
  if (at < text.length) parts.push(text.slice(at));
  return <>{parts}</>;
}

/// Whether the drawer shows this record (its row reads selected).
export function isOpenRecord(detail: MemoryRecordDetail | null | undefined, record: Pick<MemoryPanelRecord, "id">): boolean {
  return detail?.record.id === record.id;
}

/// One record in the List view (A3.4 memory row): the kind mark, the title,
/// the kind, trust and status, how often it was used, and when it changed.
export function MemoryRecordRow({
  record,
  utilityMode,
  showScope,
  query,
  selected,
  onSelect,
}: {
  record: MemoryPanelRecord;
  utilityMode?: boolean;
  /** Flat lists say whose memory the record is. */
  showScope?: boolean;
  query?: string;
  selected?: boolean;
  onSelect: () => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const utility = recordUtility(record);
  const status = record.status?.status;
  const reason = status === "quarantined" ? record.status.reason : undefined;
  const title = record.title || "Untitled record";
  const needle = query?.trim().toLowerCase() ?? "";
  const tagHits = needle ? (record.tags ?? []).filter((tag) => tag.toLowerCase().includes(needle)) : [];
  const descriptionHit = Boolean(needle && record.description && record.description.toLowerCase().includes(needle));
  const figures: React.ReactNode[] = [];
  if (utilityMode && utility.dead) figures.push(<Chip tone="warn">Never useful</Chip>);
  if (utility.injected > 0 || utility.useful > 0) {
    figures.push(
      <span className="memory-uses" title="Given to agents, judged useful">
        {`given ${utility.injected}`}
        <span aria-hidden="true">{" · "}</span>
        {`useful ${utility.useful}`}
      </span>,
    );
  }
  return (
    <EntityRow
      kind="memory"
      className={cx("memory-record-row", status === "quarantined" && "is-quarantined")}
      testId={`memory-record:${record.id}`}
      onActivate={onSelect}
      selected={selected}
      mark={<KindMark kind={record.kind} />}
      title={
        <span title={record.title || undefined}>
          <Highlight text={title} query={query} />
        </span>
      }
      activateLabel={title}
      subtitle={
        <span className="memory-record-row__sub">
          <span className="memory-facts__kind">{kindLabel(record.kind)}</span>
          <TrustLadder level={trustLevel(record.trust)} label={trustDisplay(record.trust)} />
          {showScope ? <span className="memory-record-row__scope">{scopeHeading(record.scope, nameOf)}</span> : null}
          {reason ? (
            <span className="memory-record-row__reason" title={reason}>
              {reason}
            </span>
          ) : null}
          {descriptionHit && record.description ? (
            <span className="memory-record-row__hit">
              <Highlight text={record.description} query={query} />
            </span>
          ) : null}
          {tagHits.map((tag) => (
            <span className="memory-tag" key={tag}>
              <Highlight text={tag} query={query} />
            </span>
          ))}
          {utilityMode ? <span className="memory-record-row__utility">{utilityDisplay(record)}</span> : null}
        </span>
      }
      figures={figures.length > 0 ? figures : undefined}
      status={
        status && status !== "active" ? (
          <StatusPill tone={statusMarkTone(record.status)} variant="quiet" title={statusTitle(record.status)}>
            {statusWord(record.status)}
          </StatusPill>
        ) : undefined
      }
      time={<RelativeTime value={record.updated_at_ms} />}
    />
  );
}
