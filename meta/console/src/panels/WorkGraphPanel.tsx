import React from "react";
import type {
  WorkGraphWireBinding,
  WorkGraphWireEdge,
  WorkGraphWireEvent,
  WorkGraphWireItem,
} from "../types";
import {
  WORKGRAPH_EVENTS_LIMIT,
  WORKGRAPH_OUTLINE_ROW_CAP,
  WORKGRAPH_SNAPSHOT_LIMIT,
  buildWorkGraphModel,
  isWorkGraphTerminal,
  workGraphActivityDays,
  workGraphBindingStateText,
  workGraphBindingStatusLabel,
  workGraphBindingTarget,
  workGraphBindingTargetLabel,
  workGraphBlockedByText,
  workGraphBlockedFoot,
  workGraphBlockerResolved,
  workGraphDefaultView,
  workGraphEventLine,
  workGraphEventSentence,
  workGraphGoalRevisionOf,
  workGraphItemOwner,
  workGraphItemOwnerLabel,
  workGraphModeWord,
  workGraphStatusWord,
} from "../lib/workgraph-layout";
import type { WorkGraphActivityDay, WorkGraphModel } from "../lib/workgraph-layout";
import { formatWorkGraphTimestamp } from "../lib/workgraph-time";
import {
  Button,
  Chip,
  EmptyState,
  FigureTile,
  GlanceStrip,
  ListRow,
  LoadingState,
  Notice,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  ProgressRing,
  RelativeTime,
  RowList,
  StatusPill,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  IconButton,
  KitGlyph,
  humanizeEnum,
  plural,
  priorityTone,
  standardPanelMenu,
  statusTone,
  useAgentDirectory,
  useNow,
  usePanelDensity,
  useShowTechnicalIds,
} from "../../../packages/console-components/src/panel-kit";
import type {
  MenuEntry,
  SummaryFact,
  TechnicalDetail,
} from "../../../packages/console-components/src/panel-kit";
import { StatusSquare } from "../../../packages/console-components/src/viz";
import {
  WorkGraphActivityList,
  WorkGraphAttentionRow,
  WorkGraphGraphView,
  WorkGraphPerson,
  workGraphItemActions,
} from "./WorkGraphGraphView";
import type { WorkGraphActionHandlers, WorkGraphBindingHandlers, WorkGraphItemHandlers } from "./WorkGraphGraphView";

export { WORKGRAPH_EVENTS_LIMIT, WORKGRAPH_SNAPSHOT_LIMIT };

export type WorkGraphViewMode = "tree" | "graph";

// Module-level so view choices survive dock remounts (same trick as the
// conversation card's expansion registries - dock panels are torn down and
// rebuilt on focus changes). A null view or critical path follows the
// default rule until the person chooses.
let workGraphViewModeMemory: WorkGraphViewMode | null = null;
let workGraphCriticalPathMemory: boolean | null = null;
let workGraphShowDoneMemory = false;
let workGraphShowCancelledMemory = false;

/// Snapshot-backed panel state assembled by ConsoleApp. `denied` is a -32030
/// access outcome (rendered as "no access", never as an empty store);
/// `unavailable` means no WorkGraph service is configured on the runtime.
export interface WorkGraphPanelData {
  items: WorkGraphWireItem[];
  edges: WorkGraphWireEdge[];
  attention: WorkGraphWireBinding[];
  events: WorkGraphWireEvent[];
  capturedAt: string | null;
  /// The snapshot's ready_item_ids; null or absent on runtimes that do not
  /// send them (the panel then derives readiness itself).
  readyItemIds?: string[] | null;
  /// The snapshot filled the requested limit: more items may exist.
  truncated?: boolean;
  /// Items matching before the limit, when the runtime reports it.
  total?: number | null;
  unavailable: boolean;
  denied: boolean;
  error: string | null;
}

interface WorkGraphPanelProps extends WorkGraphActionHandlers {
  /** The team picker, when the console serves more than one team; shown
   * next to the title. */
  runtimeControl?: React.ReactNode;
  data: WorkGraphPanelData;
  canManage: boolean;
  onRefresh: () => void;
}

export interface WorkGraphPanelTreeRow {
  item: WorkGraphWireItem;
  itemId: string;
  depth: number;
}

/// DFS order over parent edges (WorkEdge kind "parent" runs child→parent).
/// Roots first (by created_at, then id), children indented under parents;
/// items with a parent edge to an unknown item render as roots, and items
/// caught in a parent cycle are visited from where the cycle is entered.
export function buildWorkGraphPanelTree(
  items: WorkGraphWireItem[],
  edges: WorkGraphWireEdge[],
): WorkGraphPanelTreeRow[] {
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }
  const parentOf = new Map<string, string>();
  for (const edge of edges) {
    if (edge.kind !== "parent") continue;
    if (
      typeof edge.from_id === "string" && edge.from_id
      && typeof edge.to_id === "string" && edge.to_id
      && edge.from_id !== edge.to_id
      // Multiple parents per child are allowed upstream; placement is
      // first-parent-wins, matching the inline card fold.
      && !parentOf.has(edge.from_id)
    ) {
      parentOf.set(edge.from_id, edge.to_id);
    }
  }
  const childrenOf = new Map<string, string[]>();
  const roots: string[] = [];
  for (const id of byId.keys()) {
    const parent = parentOf.get(id);
    if (parent && byId.has(parent)) {
      const children = childrenOf.get(parent) || [];
      children.push(id);
      childrenOf.set(parent, children);
    } else {
      roots.push(id);
    }
  }
  const sortIds = (ids: string[]): string[] => (
    [...ids].sort((left, right) => {
      const leftKey = byId.get(left)?.created_at || "";
      const rightKey = byId.get(right)?.created_at || "";
      if (leftKey !== rightKey) return leftKey < rightKey ? -1 : 1;
      return left < right ? -1 : left === right ? 0 : 1;
    })
  );
  const rows: WorkGraphPanelTreeRow[] = [];
  const visited = new Set<string>();
  const visit = (start: string, startDepth: number) => {
    // Iterative, so a deep chain cannot overflow the stack.
    const stack: Array<[string, number]> = [[start, startDepth]];
    while (stack.length > 0) {
      const [id, depth] = stack.pop()!;
      if (visited.has(id)) continue;
      visited.add(id);
      const item = byId.get(id);
      if (!item) continue;
      rows.push({ item, itemId: id, depth });
      const children = sortIds(childrenOf.get(id) || []);
      for (let index = children.length - 1; index >= 0; index -= 1) stack.push([children[index], depth + 1]);
    }
  };
  for (const root of sortIds(roots)) visit(root, 0);
  // A parent cycle has no root: enter it at its oldest item.
  for (const id of sortIds([...byId.keys()])) {
    if (!visited.has(id)) visit(id, 0);
  }
  return rows;
}

/// Kept under the panel's historical export name; the single definition of
/// the precedence chain lives in workgraph-layout.ts so the outline rows, the
/// graph cards and the drawer cannot drift apart.
export function workGraphOwnerLabelOf(item: WorkGraphWireItem): string {
  return workGraphItemOwnerLabel(item);
}

/// `meta/workgraph/events` params for the panel's recent-events tail.
/// Upstream returns events ASCENDING truncated to `limit`, so a bare
/// `{limit}` query pins the OLDEST window forever once the ledger outgrows
/// it. Page from the snapshot's `event_high_water_mark` instead; a null/
/// absent mark (fresh store or older runtime) falls back to the bare query.
export function workGraphEventsParams(
  eventHighWaterMark: number | null | undefined,
  limit: number,
): { limit: number; after_seq?: number } {
  if (typeof eventHighWaterMark === "number" && Number.isFinite(eventHighWaterMark)) {
    return { limit, after_seq: Math.max(0, Math.floor(eventHighWaterMark) - limit) };
  }
  return { limit };
}

/// The panel renders the tail newest-first; upstream delivers ascending.
export function workGraphEventsNewestFirst(events: WorkGraphWireEvent[]): WorkGraphWireEvent[] {
  return [...events].reverse();
}

/// Sequences overlapping refreshes: `begin()` returns an `isCurrent` probe
/// that goes false the moment a newer refresh begins, so a stale resolution
/// never overwrites a fresher snapshot.
export function createWorkGraphRefreshSequencer(): { begin: () => () => boolean } {
  let latest = 0;
  return {
    begin() {
      latest += 1;
      const token = latest;
      return () => token === latest;
    },
  };
}

/// The header summary in plain text: "9 items · 3 in progress · 1 blocked".
export function workGraphSummary(items: WorkGraphWireItem[]): string {
  if (items.length === 0) return "No work items";
  let inProgress = 0;
  let blocked = 0;
  let failed = 0;
  for (const item of items) {
    if (item.status === "in_progress") inProgress += 1;
    else if (item.status === "blocked") blocked += 1;
    else if (item.status === "failed") failed += 1;
  }
  return [
    plural(items.length, "item"),
    inProgress ? `${inProgress} in progress` : null,
    blocked ? `${blocked} blocked` : null,
    failed ? `${failed} failed` : null,
  ].filter(Boolean).join(" · ");
}

export interface WorkGraphGoalProgress {
  total: number;
  completed: number;
  inProgress: number;
  blocked: number;
  failed: number;
}

export interface WorkGraphGoalGroup {
  root: WorkGraphPanelTreeRow;
  /// The rendered descendants, in tree order.
  children: WorkGraphPanelTreeRow[];
  /// Over every descendant, rendered or not.
  progress: WorkGraphGoalProgress;
}

/// One group per root of the tree: the root, its descendants and progress
/// over them. Rows from `visibleCount` on are past the panel's row cap: they
/// are not rendered, but still count toward their goal's progress.
export function groupWorkGraphTree(
  rows: WorkGraphPanelTreeRow[],
  visibleCount: number = rows.length,
): WorkGraphGoalGroup[] {
  const groups: WorkGraphGoalGroup[] = [];
  let current: WorkGraphGoalGroup | null = null;
  for (let index = 0; index < rows.length; index += 1) {
    const row = rows[index];
    if (row.depth === 0) {
      current = index < visibleCount
        ? { root: row, children: [], progress: { total: 0, completed: 0, inProgress: 0, blocked: 0, failed: 0 } }
        : null;
      if (current) groups.push(current);
      continue;
    }
    if (!current) continue;
    const progress = current.progress;
    progress.total += 1;
    if (row.item.status === "completed") progress.completed += 1;
    else if (row.item.status === "in_progress") progress.inProgress += 1;
    else if (row.item.status === "blocked") progress.blocked += 1;
    else if (row.item.status === "failed") progress.failed += 1;
    if (index < visibleCount) current.children.push(row);
  }
  return groups;
}

/// For each item, the titles of the blockers that still block it. A
/// "blocks" edge runs from the blocker to the item it blocks; only a present
/// and completed blocker resolves (an absent, cancelled or failed one still
/// blocks, as the server's readiness rule says).
export function workGraphBlockersOf(
  items: WorkGraphWireItem[],
  edges: WorkGraphWireEdge[],
): Map<string, string[]> {
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }
  const blockers = new Map<string, string[]>();
  const seen = new Set<string>();
  for (const edge of edges) {
    if (edge.kind !== "blocks" || !edge.from_id || !edge.to_id || edge.from_id === edge.to_id) continue;
    const key = `${edge.from_id}>${edge.to_id}`;
    if (seen.has(key)) continue;
    seen.add(key);
    const blocker = byId.get(edge.from_id);
    if (workGraphBlockerResolved(blocker)) continue;
    const titles = blockers.get(edge.to_id) || [];
    titles.push(blocker ? blocker.title || "Untitled work item" : "an item outside this view");
    blockers.set(edge.to_id, titles);
  }
  return blockers;
}

function moreItemsText(count: number, verb: string): string {
  return `+${count} more ${count === 1 ? "item" : "items"} not ${verb}`;
}

export const __workGraphPanelTest = {
  buildWorkGraphPanelTree,
  workGraphBindingStatusLabel,
  workGraphBindingTargetLabel,
  workGraphEventLine,
  workGraphOwnerLabelOf,
  workGraphGoalRevisionOf,
  workGraphEventsParams,
  workGraphEventsNewestFirst,
  createWorkGraphRefreshSequencer,
  workGraphModeWord,
  workGraphBindingStateText,
  workGraphBindingTarget,
  workGraphSummary,
  groupWorkGraphTree,
  workGraphBlockersOf,
  workGraphEventSentence,
};

// ── Rendering ────────────────────────────────────────────────────────────

function PriorityChip({ priority, className }: { priority: string | undefined; className?: string }) {
  const tone = priorityTone(priority);
  if (!tone) return null;
  return (
    <Chip tone={tone} outline={tone === "crit"} className={className}>
      {humanizeEnum(priority)}
    </Chip>
  );
}

/// Tree guides and indentation stop deepening past this depth.
const MAX_INDENT_DEPTH = 8;

function ItemRow({
  row,
  blockers,
  bindings,
  canManage,
  handlers,
  bindingHandlers,
  goalRevisionOf,
}: {
  row: WorkGraphPanelTreeRow;
  blockers?: string[];
  bindings: WorkGraphWireBinding[];
  canManage: boolean;
  handlers: WorkGraphItemHandlers;
  bindingHandlers: WorkGraphBindingHandlers;
  goalRevisionOf: (binding: WorkGraphWireBinding) => number | undefined;
}) {
  const directory = useAgentDirectory();
  const { item, itemId, depth } = row;
  const status = item.status || "open";
  const owner = workGraphItemOwner(item, directory);
  const actions = workGraphItemActions(item, itemId, canManage, handlers);
  return (
    <ListRow
      dense
      className="workgraph-row"
      testId={`workgraph-panel-item:${itemId}`}
      dataAttributes={{ "data-status": status, "data-depth": String(Math.min(depth, MAX_INDENT_DEPTH)) }}
      lead={<StatusSquare status={status} size={12} />}
      title={item.title || "Untitled work item"}
      titleAttr={item.description || item.title || undefined}
      subtitle={blockers?.length && !isWorkGraphTerminal(status) ? workGraphBlockedByText(blockers) : undefined}
      meta={(
        <>
          {/* The status word comes first so a narrow pane, which keeps only
              the first meta item, still says it; CSS orders it last. */}
          <span className="workgraph-row__status">{workGraphStatusWord(status)}</span>
          <PriorityChip priority={item.priority} className="workgraph-row__priority" />
          {owner ? (
            <WorkGraphPerson person={owner} avatar={18} className="workgraph-row__owner" />
          ) : isWorkGraphTerminal(status) ? null : (
            <span className="workgraph-row__owner workgraph-row__unassigned">Unassigned</span>
          )}
        </>
      )}
      actions={actions ?? undefined}
    >
      {bindings.length ? (
        <div className="workgraph-row__bindings">
          {bindings.map((binding, index) => (
            <WorkGraphAttentionRow
              key={binding.binding_id || `binding-${index}`}
              binding={binding}
              goalRevision={goalRevisionOf(binding)}
              canManage={canManage}
              handlers={bindingHandlers}
            />
          ))}
        </div>
      ) : null}
    </ListRow>
  );
}

function GoalCard({
  group,
  model,
  bindings,
  bindingsOf,
  blockers,
  canManage,
  collapsed,
  onToggle,
  handlers,
  bindingHandlers,
  goalRevisionOf,
}: {
  group: WorkGraphGoalGroup;
  model: WorkGraphModel;
  bindings: WorkGraphWireBinding[];
  bindingsOf: (itemId: string) => WorkGraphWireBinding[];
  blockers: Map<string, string[]>;
  canManage: boolean;
  /// A goal whose every item is finished shows its header only.
  collapsed: boolean;
  onToggle: () => void;
  handlers: WorkGraphItemHandlers;
  bindingHandlers: WorkGraphBindingHandlers;
  goalRevisionOf: (binding: WorkGraphWireBinding) => number | undefined;
}) {
  const directory = useAgentDirectory();
  const { item, itemId } = group.root;
  const status = item.status || "open";
  const owner = workGraphItemOwner(item, directory);
  const actions = workGraphItemActions(item, itemId, canManage, handlers);
  const goalBlockers = blockers.get(itemId);
  const { progress } = group;
  const frame = model.frames.find((candidate) => candidate.id === itemId);
  const technical: TechnicalDetail[] = [{ label: "Work item ID", value: itemId }];
  for (const binding of bindings) {
    if (binding.binding_id) technical.push({ label: "Attention binding ID", value: binding.binding_id });
    if (binding.target?.session_id) technical.push({ label: "Session ID", value: binding.target.session_id });
  }
  return (
    <article className="workgraph-goal" data-testid={`workgraph-panel-item:${itemId}`} data-status={status} data-state={frame?.state}>
      <div className="workgraph-goal__head">
        <div className="workgraph-goal__name">
          <KitGlyph name="flag" className="workgraph-goal__flag" />
          <h3 className="workgraph-goal__title" title={item.description || item.title || undefined}>
            {item.title || "Untitled work item"}
          </h3>
        </div>
        {progress.total > 0 ? (
          <span className="workgraph-goal__progress">
            <ProgressRing
              done={progress.completed}
              inProgress={progress.inProgress}
              blocked={progress.blocked}
              total={progress.total}
              size={16}
            />
            {`${progress.completed} of ${progress.total} done`}
          </span>
        ) : null}
        <span className="workgraph-goal__chips">
          <PriorityChip priority={item.priority} />
          <StatusPill tone={statusTone(status)}>{workGraphStatusWord(status)}</StatusPill>
        </span>
        {actions ? <span className="workgraph-goal__actions">{actions}</span> : null}
        {owner ? <WorkGraphPerson person={owner} avatar={18} className="workgraph-goal__owner" /> : null}
      </div>
      {goalBlockers?.length && !isWorkGraphTerminal(status) ? (
        <p className="workgraph-goal__blocked">{workGraphBlockedByText(goalBlockers)}</p>
      ) : null}
      {bindings.length ? (
        <div className="workgraph-goal__bindings">
          {bindings.map((binding, index) => (
            <WorkGraphAttentionRow
              key={binding.binding_id || `binding-${index}`}
              binding={binding}
              goalRevision={goalRevisionOf(binding)}
              canManage={canManage}
              handlers={bindingHandlers}
            />
          ))}
        </div>
      ) : null}
      {group.children.length && !collapsed ? (
        <RowList className="workgraph-goal__items">
          {group.children.map((row) => (
            <ItemRow
              key={row.itemId}
              row={row}
              blockers={blockers.get(row.itemId)}
              bindings={bindingsOf(row.itemId)}
              canManage={canManage}
              handlers={handlers}
              bindingHandlers={bindingHandlers}
              goalRevisionOf={goalRevisionOf}
            />
          ))}
        </RowList>
      ) : null}
      {group.children.length && (collapsed || frame?.state === "done") ? (
        <div className="workgraph-goal__fold">
          <Button size="sm" variant="ghost" aria-expanded={!collapsed} onClick={onToggle}>
            {collapsed ? `Show ${plural(group.children.length, "finished item")}` : "Fold finished items"}
          </Button>
        </div>
      ) : null}
      <TechnicalDetails className="workgraph-goal__tech" items={technical} />
    </article>
  );
}

/// Items created and closed per day as paired bars (created in the mid
/// ink, closed in the full ink), with a zero tick for empty days.
function ActivityBars({ days }: { days: readonly WorkGraphActivityDay[] }) {
  const max = Math.max(1, ...days.map((day) => Math.max(day.created, day.closed)));
  const height = 28;
  const bar = 6;
  const slot = 2 * bar + 10;
  const width = days.length * slot;
  const label = `Items created and closed per day over the last ${days.length} days: ${days
    .map((day) => `${new Date(day.start).toLocaleDateString(undefined, { weekday: "short" })} ${day.created} created, ${day.closed} closed`)
    .join("; ")}`;
  const heightOf = (value: number) => (value > 0 ? Math.max(2, Math.round((value / max) * height)) : 0);
  return (
    <svg
      className="workgraph-strip__bars"
      width={width}
      height={height + 2}
      // The console's icon reset sizes every svg to 16 px; this one is a chart.
      style={{ width, height: height + 2 }}
      viewBox={`0 0 ${width} ${height + 2}`}
      role="img"
      aria-label={label}
    >
      <line className="workgraph-strip__base" x1={0} x2={width} y1={height + 0.5} y2={height + 0.5} />
      {days.map((day, index) => {
        const x = index * slot + 4;
        const created = heightOf(day.created);
        const closed = heightOf(day.closed);
        return (
          <g key={day.start}>
            {created ? <rect className="workgraph-strip__created" x={x} y={height - created} width={bar} height={created} rx={1} /> : (
              <rect className="workgraph-strip__zero" x={x} y={height - 1} width={bar} height={1} />
            )}
            {closed ? <rect className="workgraph-strip__closed" x={x + bar + 1} y={height - closed} width={bar} height={closed} rx={1} /> : (
              <rect className="workgraph-strip__zero" x={x + bar + 1} y={height - 1} width={bar} height={1} />
            )}
          </g>
        );
      })}
    </svg>
  );
}

/// The strip under the graph: a week of created and closed items, the
/// latest two events, and the way to the full activity.
function ActivityStrip({
  events,
  titleOf,
  onShowAll,
  showingAll,
}: {
  events: readonly WorkGraphWireEvent[];
  titleOf: (itemId: string) => string | undefined;
  onShowAll: () => void;
  showingAll: boolean;
}) {
  const directory = useAgentDirectory();
  const now = useNow();
  const minute = Math.floor(now / 60_000);
  const days = React.useMemo(() => workGraphActivityDays(events, minute * 60_000, 7), [events, minute]);
  const latest = events.slice(0, 2);
  return (
    <div className="workgraph-strip" data-testid="workgraph-activity-strip">
      <div className="workgraph-strip__chart">
        <span className="workgraph-strip__label">Activity · 7 days</span>
        <ActivityBars days={days} />
        <span className="workgraph-strip__legend" aria-hidden="true">
          <span className="workgraph-strip__key is-created" />created
          <span className="workgraph-strip__key is-closed" />closed
        </span>
      </div>
      <div className="workgraph-strip__latest">
        {latest.length === 0 ? <span className="workgraph-strip__none">No activity yet</span> : latest.map((event, index) => (
          <span key={`${event.seq ?? index}`} className="workgraph-strip__event">
            <span className="workgraph-strip__sentence">{workGraphEventSentence(event, { titleOf, directory })}</span>
            {" · "}
            <RelativeTime value={event.at} />
          </span>
        ))}
        {events.length > 0 ? (
          <Button size="sm" variant="ghost" className="workgraph-strip__all" aria-pressed={showingAll} data-testid="workgraph-activity-all" onClick={onShowAll}>
            {showingAll ? "Hide activity" : `Show all activity (${events.length})`}
          </Button>
        ) : null}
      </div>
    </div>
  );
}

/// The panel's measured content width, for the default view rule; null
/// until measured (server render, jsdom).
function useMeasuredWidth(ref: React.RefObject<HTMLElement | null>): number | null {
  const [width, setWidth] = React.useState<number | null>(null);
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const apply = (next: number) => setWidth((current) => {
      const value = next > 0 ? Math.round(next) : null;
      return current === value ? current : value;
    });
    apply(element.getBoundingClientRect().width);
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver((entries) => {
      const box = entries[entries.length - 1]?.contentRect;
      if (box) apply(box.width);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}

function startOfToday(now: number): number {
  const day = new Date(now);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

export function WorkGraphPanel({
  runtimeControl,
  data,
  canManage,
  onRefresh,
  onClaim,
  onClose,
  onRelease,
  onGoalConfirm,
  onGoalRequestClose,
  onAttentionPause,
  onAttentionResume,
  onAttentionReassign,
}: WorkGraphPanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const now = useNow();
  const [density, setDensity] = usePanelDensity("workgraph");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("workgraph");
  const model = React.useMemo(
    () => buildWorkGraphModel({ items: data.items, edges: data.edges, attention: data.attention, readyItemIds: data.readyItemIds }),
    [data.items, data.edges, data.attention, data.readyItemIds],
  );
  const rows = React.useMemo(
    () => buildWorkGraphPanelTree(data.items, data.edges),
    [data.items, data.edges],
  );
  const blockers = React.useMemo(
    () => workGraphBlockersOf(data.items, data.edges),
    [data.items, data.edges],
  );
  const measureRef = React.useRef<HTMLDivElement | null>(null);
  const width = useMeasuredWidth(measureRef);
  const [viewChoice, setViewChoice] = React.useState<WorkGraphViewMode | null>(workGraphViewModeMemory);
  const viewMode: WorkGraphViewMode = viewChoice ?? workGraphDefaultView(width, model.items.size);
  const setViewMode = (mode: WorkGraphViewMode) => {
    workGraphViewModeMemory = mode;
    setViewChoice(mode);
  };
  const [criticalChoice, setCriticalChoice] = React.useState<boolean | null>(workGraphCriticalPathMemory);
  const chainLength = model.criticalPath.itemIds.length;
  const highlightCriticalPath = criticalChoice ?? chainLength >= 2;
  const setHighlightCriticalPath = (on: boolean) => {
    workGraphCriticalPathMemory = on;
    setCriticalChoice(on);
  };
  const [showDone, setShowDoneState] = React.useState(workGraphShowDoneMemory);
  const setShowDone = (on: boolean) => {
    workGraphShowDoneMemory = on;
    setShowDoneState(on);
  };
  const [showCancelled, setShowCancelledState] = React.useState(workGraphShowCancelledMemory);
  const setShowCancelled = (on: boolean) => {
    workGraphShowCancelledMemory = on;
    setShowCancelledState(on);
  };
  const [selectedId, setSelectedId] = React.useState<string | null>(null);
  const [activityOpen, setActivityOpen] = React.useState(false);
  const [unfolded, setUnfolded] = React.useState<ReadonlySet<string>>(() => new Set());
  const [allRows, setAllRows] = React.useState(false);
  // A selection that left the snapshot closes its drawer.
  const selected = selectedId && model.items.has(selectedId) ? selectedId : null;
  const select = (itemId: string | null) => {
    setSelectedId(itemId);
    if (itemId) setActivityOpen(false);
  };

  const shellProps = {
    className: "workgraph",
    testId: "workgraph-panel",
    density,
    showTechnicalIds,
  } as const;

  if (data.unavailable) {
    return (
      <PanelShell {...shellProps}>
        <PanelHeader className="workgraph__head" title="WorkGraph" scope={runtimeControl || undefined} />
        <PanelBody>
          <EmptyState
            kind="not-configured"
            illustration="not-configured"
            title="WorkGraph isn't set up on this runtime"
            body="Work items appear here once the runtime runs a WorkGraph service."
          />
        </PanelBody>
      </PanelShell>
    );
  }

  const handlers: WorkGraphActionHandlers = {
    onClaim,
    onClose,
    onRelease,
    onGoalConfirm,
    onGoalRequestClose,
    onAttentionPause,
    onAttentionResume,
    onAttentionReassign,
  };
  const goalRevisionOf = (binding: WorkGraphWireBinding) => workGraphGoalRevisionOf(binding, data.items);
  const titleOf = (itemId: string) => model.items.get(itemId)?.title;

  // A loaded snapshot always carries its capture time; until one arrives
  // (and nothing failed) the panel is loading.
  const loaded = Boolean(data.capturedAt);
  const loading = !loaded && !data.error && !data.denied
    && rows.length === 0 && data.attention.length === 0 && data.events.length === 0;
  const capturedLabel = formatWorkGraphTimestamp(data.capturedAt, { date: true, seconds: true });
  const counts = model.counts;
  const showFigures = !data.denied && !loading && model.items.size > 0;

  // ── Outline: goals as cards, plain roots in Other work ────────────────
  const visibleCount = allRows ? rows.length : Math.min(rows.length, WORKGRAPH_OUTLINE_ROW_CAP);
  const treeOverflowCount = rows.length - visibleCount;
  const groups = groupWorkGraphTree(rows, visibleCount);
  const bindingsByItem = new Map<string, WorkGraphWireBinding[]>();
  for (const binding of data.attention) {
    const itemId = binding.work_ref?.item_id;
    if (!itemId) continue;
    bindingsByItem.set(itemId, [...(bindingsByItem.get(itemId) || []), binding]);
  }
  const bindingsOf = (itemId: string) => bindingsByItem.get(itemId) || [];
  const renderedIds = new Set(rows.slice(0, visibleCount).map((row) => row.itemId));
  const otherAttention = data.attention.filter((binding) => {
    const itemId = binding.work_ref?.item_id;
    return !itemId || !renderedIds.has(itemId);
  });
  // A root with descendants or attention is a goal and gets a card; other
  // roots are plain work items in one list.
  const isGoal = (group: WorkGraphGoalGroup) =>
    group.progress.total > 0 || group.children.length > 0 || bindingsByItem.has(group.root.itemId);
  const goals = groups.filter(isGoal);
  const looseItems = groups.filter((group) => !isGoal(group));
  const goalDone = (group: WorkGraphGoalGroup) =>
    isWorkGraphTerminal(group.root.item.status || "open")
    && group.children.every((row) => isWorkGraphTerminal(row.item.status || "open"));
  const bindingSubject = (binding: WorkGraphWireBinding): React.ReactNode => {
    const itemId = binding.work_ref?.item_id;
    if (!itemId) return undefined;
    const title = titleOf(itemId);
    return title || <TechnicalId value={itemId} label="Work item ID" />;
  };
  const attentionList = (bindings: WorkGraphWireBinding[]) => (
    <div className="workgraph__attention">
      {bindings.map((binding, index) => (
        <WorkGraphAttentionRow
          key={binding.binding_id || `binding-${index}`}
          binding={binding}
          goalRevision={goalRevisionOf(binding)}
          canManage={canManage}
          subject={bindingSubject(binding)}
          handlers={handlers}
        />
      ))}
    </div>
  );

  const activityList = (
    <WorkGraphActivityList
      events={data.events}
      titleOf={titleOf}
      onSelectItem={(itemId) => select(itemId)}
      testId="workgraph-activity"
    />
  );

  let content: React.ReactNode;
  if (data.denied) {
    content = (
      <EmptyState
        kind="no-access"
        testId="workgraph-panel-denied"
        title="You don't have access to work items"
        body="An administrator can grant access in Console access."
      />
    );
  } else if (loading) {
    content = viewChoice === "graph"
      ? <LoadingState variant="graph" label="Loading work items" />
      : <LoadingState variant="cards" rows={3} label="Loading work items" />;
  } else if (!loaded && data.error && rows.length === 0) {
    content = (
      <EmptyState
        kind="error"
        title="Work items could not be loaded"
        body="Refresh to try again."
        details={[{ label: "Error", value: data.error, plain: true }]}
      />
    );
  } else if (model.items.size === 0) {
    content = (
      <>
        <EmptyState
          illustration="work"
          title="No work items yet"
          body="Agents add work items here as they plan and track their goals."
          testId="workgraph-empty"
        />
        {data.attention.length ? (
          <PanelSection label={<span title="Attention bindings">Agents on goals</span>} count={data.attention.length}>
            {attentionList(data.attention)}
          </PanelSection>
        ) : null}
      </>
    );
  } else if (viewMode === "graph") {
    content = (
      <WorkGraphGraphView
        items={data.items}
        edges={data.edges}
        attention={data.attention}
        events={data.events}
        readyItemIds={data.readyItemIds}
        model={model}
        selectedId={selected ?? undefined}
        onSelect={select}
        showDone={showDone}
        showCancelled={showCancelled}
        highlightCriticalPath={highlightCriticalPath}
        density={density}
        canManage={canManage}
        handlers={handlers}
        cacheKey="panel"
        footer={(
          <ActivityStrip
            events={data.events}
            titleOf={titleOf}
            showingAll={activityOpen && !selected}
            onShowAll={() => {
              setSelectedId(null);
              setActivityOpen((open) => !(open && !selected));
            }}
          />
        )}
        aside={activityOpen ? {
          title: "Activity",
          testId: "workgraph-activity-drawer",
          onClose: () => setActivityOpen(false),
          content: data.events.length ? activityList : <EmptyState inline title="No activity yet" />,
        } : null}
      />
    );
  } else {
    content = (
      <div className="workgraph__outline">
        {goals.length ? (
          <div className="workgraph__goals">
            {goals.map((group) => {
              const done = goalDone(group);
              const collapsed = done && !showDone && !unfolded.has(group.root.itemId);
              return (
                <GoalCard
                  key={group.root.itemId}
                  group={group}
                  model={model}
                  bindings={bindingsOf(group.root.itemId)}
                  bindingsOf={bindingsOf}
                  blockers={blockers}
                  canManage={canManage}
                  collapsed={collapsed}
                  onToggle={() => setUnfolded((current) => {
                    const next = new Set(current);
                    if (next.has(group.root.itemId)) next.delete(group.root.itemId);
                    else next.add(group.root.itemId);
                    return next;
                  })}
                  handlers={handlers}
                  bindingHandlers={handlers}
                  goalRevisionOf={goalRevisionOf}
                />
              );
            })}
          </div>
        ) : null}
        {looseItems.length ? (
          <PanelSection label={goals.length ? "Other work" : "Work items"} count={looseItems.length}>
            <RowList>
              {looseItems.map((group) => (
                <ItemRow
                  key={group.root.itemId}
                  row={group.root}
                  blockers={blockers.get(group.root.itemId)}
                  bindings={[]}
                  canManage={canManage}
                  handlers={handlers}
                  bindingHandlers={handlers}
                  goalRevisionOf={goalRevisionOf}
                />
              ))}
            </RowList>
          </PanelSection>
        ) : null}
        {treeOverflowCount > 0 ? (
          <p className="workgraph__overflow" data-testid="workgraph-panel-overflow">
            {moreItemsText(treeOverflowCount, "shown")}
            {" "}
            <Button size="sm" variant="ghost" onClick={() => setAllRows(true)}>Show all</Button>
          </p>
        ) : null}
        {otherAttention.length ? (
          <PanelSection label={<span title="Attention bindings">Agents on other goals</span>} count={otherAttention.length}>
            {attentionList(otherAttention)}
          </PanelSection>
        ) : null}
        <PanelSection label="Activity" count={data.events.length || null} className="workgraph__activity">
          {data.events.length === 0 ? <EmptyState inline title="No activity yet" /> : activityList}
        </PanelSection>
      </div>
    );
  }

  // ── Header and figures ───────────────────────────────────────────────
  const summary: Array<SummaryFact | null> = data.denied
    ? []
    : loading
      ? [{ key: "loading", text: "Loading work items", announce: false }]
      : [
        { key: "items", text: plural(counts.total, "item") },
        counts.inProgress ? { key: "in-progress", text: `${counts.inProgress} in progress`, tone: "live" } : null,
        counts.blocked ? { key: "blocked", text: `${counts.blocked} blocked`, tone: "warn" } : null,
        counts.failed ? { key: "failed", text: `${counts.failed} failed`, tone: "crit" } : null,
        counts.ready ? { key: "ready", text: `${counts.ready} ready`, tone: "ok" } : null,
      ];
  const viewOptions: MenuEntry[] = [
    {
      kind: "checkbox",
      id: "show-done",
      label: "Show done goals",
      checked: showDone,
      onChange: setShowDone,
      testId: "workgraph-menu:show-done",
    },
    {
      kind: "checkbox",
      id: "show-cancelled",
      label: "Show cancelled",
      checked: showCancelled,
      onChange: setShowCancelled,
      testId: "workgraph-menu:show-cancelled",
    },
    {
      kind: "checkbox",
      id: "critical-path",
      label: "Critical path",
      checked: highlightCriticalPath,
      onChange: setHighlightCriticalPath,
      disabled: chainLength === 0,
      testId: "workgraph-menu:critical-path",
    },
  ];
  const doneToday = (() => {
    const start = startOfToday(now);
    let count = 0;
    for (const entry of model.items.values()) {
      if (entry.status !== "completed" || !entry.item.terminal_at) continue;
      const at = Date.parse(entry.item.terminal_at);
      if (Number.isFinite(at) && at >= start) count += 1;
    }
    return count;
  })();

  return (
    <PanelShell {...shellProps}>
      <PanelHeader
        className="workgraph__head"
        title="WorkGraph"
        scope={runtimeControl || undefined}
        summary={summary}
        summaryTestId="workgraph-summary"
        freshness={data.denied || !loaded ? undefined : {
          at: data.capturedAt ?? undefined,
          title: capturedLabel ? `Snapshot as of ${capturedLabel}` : undefined,
        }}
        view={{
          ariaLabel: "WorkGraph view mode",
          value: viewMode,
          onChange: setViewMode,
          options: [
            {
              id: "graph",
              label: <><KitGlyph name="graph" className="workgraph__view-glyph" />Graph</>,
              ariaLabel: "Graph",
              testId: "workgraph-view-toggle:graph",
            },
            {
              id: "tree",
              label: <><KitGlyph name="tree" className="workgraph__view-glyph" />Outline</>,
              ariaLabel: "Outline",
              testId: "workgraph-view-toggle:tree",
            },
          ],
        }}
        tools={(
          <IconButton
            label="Refresh work items"
            glyph="refresh"
            data-testid="workgraph-panel-refresh"
            onClick={onRefresh}
          />
        )}
        menu={{
          label: "More actions for WorkGraph",
          triggerTestId: "workgraph-menu",
          items: standardPanelMenu({
            viewOptions,
            density,
            onDensityChange: setDensity,
            showTechnicalIds,
            onShowTechnicalIdsChange: setShowTechnicalIds,
            getCopyData: () => ({
              captured_at: data.capturedAt,
              items: data.items,
              edges: data.edges,
              attention: data.attention,
              ready_item_ids: data.readyItemIds ?? null,
              events: data.events,
            }),
            onRefresh,
            testIdPrefix: "workgraph",
          }),
        }}
      />
      {showFigures ? (
        <GlanceStrip ariaLabel="Work at a glance" testId="workgraph-glance" className="workgraph__glance">
          <FigureTile
            hero
            label="Done"
            value={counts.completed}
            of={counts.total}
            meter={(
              <ProgressRing
                done={counts.completed}
                inProgress={counts.inProgress}
                blocked={counts.blocked}
                total={counts.total}
                size={24}
              />
            )}
            foot={doneToday ? `+${doneToday} today` : "none today"}
            testId="workgraph-glance:done"
          />
          <FigureTile
            label="In progress"
            tone="live"
            live={counts.inProgress > 0}
            value={counts.inProgress}
            foot={counts.inProgressOwners
              ? `${plural(counts.inProgressOwners, "agent")} on them`
              : counts.inProgress ? "nobody has claimed them" : "nothing moving"}
            testId="workgraph-glance:in_progress"
          />
          <FigureTile
            label="Blocked"
            tone="warn"
            value={counts.blocked}
            foot={workGraphBlockedFoot(model)}
            testId="workgraph-glance:blocked"
          />
          <FigureTile
            label="Ready"
            tone="ok"
            value={counts.ready}
            foot={counts.ready ? "can start now" : "nothing can start yet"}
            title={model.readySource === "server" ? "Open, unblocked and due, as the runtime reports" : undefined}
            testId="workgraph-glance:ready"
          />
          <FigureTile
            label="To finish"
            value={chainLength}
            unit={chainLength === 1 ? "step" : "steps"}
            foot={(
              <Button
                size="sm"
                variant="ghost"
                className="workgraph__critical-toggle"
                aria-pressed={highlightCriticalPath}
                disabled={chainLength === 0}
                title="The longest chain of unfinished items, in order"
                data-testid="workgraph-critical-path"
                onClick={() => {
                  const next = !highlightCriticalPath;
                  setHighlightCriticalPath(next);
                  // The path is drawn on the graph: turning it on shows it.
                  if (next && viewMode !== "graph") setViewMode("graph");
                }}
              >
                {highlightCriticalPath ? <KitGlyph name="check" /> : null}
                Critical path
              </Button>
            )}
            testId="workgraph-glance:to_finish"
          />
        </GlanceStrip>
      ) : null}
      {data.truncated && !data.denied ? (
        <Notice tone="warn" flush testId="workgraph-truncated">
          {typeof data.total === "number" && data.total > data.items.length
            ? `Showing ${data.items.length} of ${data.total} items. The figures count the ones shown.`
            : `Showing the first ${data.items.length} items; the runtime holds more. The figures count the ones shown.`}
        </Notice>
      ) : null}
      {/* A failed refresh keeps the last items on screen; its error is a
          toast over the body, so it never moves the header controls or the
          items under a click in progress. */}
      <StatusRow
        mode="toast"
        error={data.error}
        updatedAt={loaded ? data.capturedAt : undefined}
        updatedTitle={capturedLabel ? `Snapshot as of ${capturedLabel}` : undefined}
        onRetry={onRefresh}
      />
      <PanelBody className={viewMode === "graph" && model.items.size > 0 && !data.denied && !loading ? "workgraph__body is-graph" : "workgraph__body"}>
        <div ref={measureRef} className="workgraph__measure" aria-hidden="true" />
        {content}
      </PanelBody>
    </PanelShell>
  );
}
