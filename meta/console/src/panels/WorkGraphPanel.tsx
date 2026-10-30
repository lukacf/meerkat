import React from "react";
import type {
  WorkGraphWireBinding,
  WorkGraphWireEdge,
  WorkGraphWireEvent,
  WorkGraphWireItem,
} from "../types";
import {
  WORKGRAPH_GRAPH_NODE_CAP,
  workGraphItemOwner,
  workGraphItemOwnerLabel,
  workGraphStatusWord,
} from "../lib/workgraph-layout";
import type { WorkGraphOwnerView } from "../lib/workgraph-layout";
import { formatWorkGraphTimestamp } from "../lib/workgraph-time";
import {
  Button,
  Chip,
  EmptyState,
  KitGlyph,
  ListRow,
  LoadingState,
  Meter,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  RelativeTime,
  RowList,
  SegmentedControl,
  StatusDot,
  StatusRow,
  TechnicalDetails,
  TechnicalId,
  formatClockTime,
  formatShortDate,
  humanizeEnum,
  joinMeta,
  plural,
  priorityTone,
  statusTone,
  agentDisplayName,
  toEpochMs,
  useAgentDirectory,
  useNow,
} from "../../../packages/console-components/src/panel-kit";
import type {
  AgentDirectory,
  TechnicalDetail,
} from "../../../packages/console-components/src/panel-kit";
import { WorkGraphGraphView, WorkGraphPerson } from "./WorkGraphGraphView";

export type WorkGraphViewMode = "tree" | "graph";

// Module-level so the chosen view survives dock remounts (same trick as
// the conversation card's expansion registries - dock panels are torn
// down and rebuilt on focus changes).
let workGraphViewModeMemory: WorkGraphViewMode = "tree";

/// Snapshot-backed panel state assembled by ConsoleApp. `denied` is a -32030
/// access outcome (rendered as "no grant", never as an empty store);
/// `unavailable` means no WorkGraph service is configured on the runtime.
export interface WorkGraphPanelData {
  items: WorkGraphWireItem[];
  edges: WorkGraphWireEdge[];
  attention: WorkGraphWireBinding[];
  events: WorkGraphWireEvent[];
  capturedAt: string | null;
  unavailable: boolean;
  denied: boolean;
  error: string | null;
}

// Two CAS classes on the action payloads: goal actions (confirm /
// request-close) carry the goal WORK ITEM's revision; attention actions
// (pause / resume / reassign) carry the binding's machine revision.
interface WorkGraphPanelProps {
  /** The team picker, when the console serves more than one team; shown
   * among the header actions. */
  runtimeControl?: React.ReactNode;
  data: WorkGraphPanelData;
  canManage: boolean;
  onRefresh: () => void;
  // An absent revision means the snapshot never carried one: handlers must
  // resolve the live revision before mutating (never substitute 0).
  onClaim?: (input: { itemId: string; revision?: number }) => void;
  onClose?: (input: { itemId: string; revision?: number }) => void;
  onGoalConfirm?: (input: { bindingId: string; revision?: number }) => void;
  onGoalRequestClose?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionPause?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionResume?: (input: { bindingId: string; revision?: number }) => void;
  onAttentionReassign?: (input: { bindingId: string; revision?: number; identity: string }) => void;
}

export interface WorkGraphPanelTreeRow {
  item: WorkGraphWireItem;
  itemId: string;
  depth: number;
}

/// DFS order over parent edges (WorkEdge kind "parent" runs child→parent).
/// Roots first (by created_at, then id), children indented under parents;
/// items with a parent edge to an unknown item render as roots.
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
  const visit = (id: string, depth: number) => {
    if (visited.has(id)) return;
    visited.add(id);
    const item = byId.get(id);
    if (!item) return;
    rows.push({ item, itemId: id, depth });
    for (const child of sortIds(childrenOf.get(id) || [])) {
      visit(child, depth + 1);
    }
  };
  for (const root of sortIds(roots)) visit(root, 0);
  return rows;
}

export function workGraphBindingStatusLabel(binding: WorkGraphWireBinding): string {
  const state = binding.status?.state || "active";
  if (state === "paused") {
    const until = formatWorkGraphTimestamp(binding.status?.until, { date: true });
    return until ? `paused until ${until}` : "paused";
  }
  return state;
}

export function workGraphBindingTargetLabel(binding: WorkGraphWireBinding): string {
  const target = binding.target;
  if (!target) return "";
  if (typeof target.session_id === "string" && target.session_id) return target.session_id;
  const ownerKey = target.owner_key;
  if (ownerKey) {
    return [ownerKey.kind, ownerKey.id].filter(Boolean).join(":");
  }
  return "";
}

export function workGraphEventLine(event: WorkGraphWireEvent): string {
  const kind = typeof event.kind === "string" ? event.kind.replace(/_/g, " ") : "event";
  const at = formatWorkGraphTimestamp(event.at, { date: true });
  const item = typeof event.item_id === "string" && event.item_id ? event.item_id : "";
  return [at, kind, item].filter(Boolean).join(" · ");
}

/// Kept under the panel's historical export name; the single definition of
/// the precedence chain lives in workgraph-layout.ts so the tree rows, the
/// graph nodes, and the graph selection footer cannot drift apart.
export function workGraphOwnerLabelOf(item: WorkGraphWireItem): string {
  return workGraphItemOwnerLabel(item);
}

/// Latest observed revision of the binding's bound goal work item. Goal
/// confirm / request-close CAS against this (the server checks the ITEM's
/// revision), never against the binding's machine revision.
export function workGraphGoalRevisionOf(
  binding: WorkGraphWireBinding,
  items: WorkGraphWireItem[],
): number | undefined {
  const itemId = binding.work_ref?.item_id;
  if (!itemId) return undefined;
  const item = items.find((candidate) => candidate.id === itemId);
  return typeof item?.revision === "number" ? item.revision : undefined;
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

// ── Display helpers ──────────────────────────────────────────────────────
// The helpers above keep the raw wire forms their tests pin; these turn the
// same data into the words, names and sentences the panel shows.

const TERMINAL_STATUSES = new Set(["completed", "cancelled", "failed"]);

/// What the agent on a goal does with it, in plain words; the attention
/// mode's own name ("coordinate", "falsify") stays the tooltip.
const MODE_WORDS: Record<string, string> = {
  pursue: "Doing the work",
  coordinate: "Leading the work",
  review: "Reviewing the work",
  falsify: "Testing the work",
  judge: "Judging the work",
  observe: "Watching the work",
};

/// What an attention binding does with its goal, in words. A missing mode
/// is "pursue", as upstream defaults it.
export function workGraphModeWord(mode: string | null | undefined): string {
  const key = mode || "pursue";
  return MODE_WORDS[key] || humanizeEnum(key);
}

const DAY_MS = 24 * 60 * 60 * 1000;

/// A nearby moment inside a sentence: the clock today, the weekday and clock
/// within the week, the date and clock beyond it.
function nearMoment(ms: number, now: number): string {
  const clock = formatClockTime(ms);
  const date = new Date(ms);
  if (date.toDateString() === new Date(now).toDateString()) return clock;
  if (Math.abs(ms - now) < 6 * DAY_MS) {
    return `${date.toLocaleDateString(undefined, { weekday: "short" })} ${clock}`;
  }
  return `${formatShortDate(ms, now)}, ${clock}`;
}

/// A binding's state in words: "Active", "Paused until Fri 11:16",
/// "Replaced" (superseded by a newer binding). workGraphBindingStatusLabel
/// keeps the raw form.
export function workGraphBindingStateText(
  binding: WorkGraphWireBinding,
  now: number = Date.now(),
): string {
  const state = binding.status?.state || "active";
  if (state === "paused") {
    const until = toEpochMs(binding.status?.until);
    return until === null ? "Paused" : `Paused until ${nearMoment(until, now)}`;
  }
  if (state === "superseded") return "Replaced";
  return humanizeEnum(state);
}

/// Who a binding's attention rests on, by name: the roster name for a known
/// session or owner key, the bare key for an owner the roster does not know,
/// and "Agent session" for an unknown session (its ID stays in the
/// tooltip and the technical details). workGraphBindingTargetLabel keeps the
/// raw target.
export function workGraphBindingTarget(
  binding: WorkGraphWireBinding,
  directory?: AgentDirectory,
): WorkGraphOwnerView | null {
  const target = binding.target;
  if (!target) return null;
  const sessionId = typeof target.session_id === "string" ? target.session_id.trim() : "";
  if (sessionId) {
    return { name: directory?.lookup(sessionId)?.label.trim() || "Agent session", key: sessionId };
  }
  const id = target.owner_key?.id?.trim() || "";
  if (!id) return null;
  const kind = target.owner_key?.kind?.trim() || "";
  const key = kind ? `${kind}:${id}` : id;
  return { name: directory?.lookup(key)?.label.trim() || agentDisplayName(id), key };
}

/// The agent a binding rests on, named for a sentence; null for a session
/// the roster does not know (the row says "Agent session").
function bindingAgentName(binding: WorkGraphWireBinding | null, directory?: AgentDirectory): string | null {
  if (!binding) return null;
  const target = workGraphBindingTarget(binding, directory);
  if (!target) return null;
  const sessionId = typeof binding.target?.session_id === "string" ? binding.target.session_id.trim() : "";
  if (sessionId && !directory?.lookup(sessionId)) return null;
  return target.name || null;
}

/// The header summary: "9 items · 3 in progress · 1 blocked".
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
  return joinMeta(
    plural(items.length, "item"),
    inProgress ? `${inProgress} in progress` : null,
    blocked ? `${blocked} blocked` : null,
    failed ? `${failed} failed` : null,
  );
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

/// For each item, the titles of the unfinished items that block it. A
/// "blocks" edge runs from the blocker to the item it blocks.
export function workGraphBlockersOf(
  items: WorkGraphWireItem[],
  edges: WorkGraphWireEdge[],
): Map<string, string[]> {
  const byId = new Map<string, WorkGraphWireItem>();
  for (const item of items) {
    if (typeof item.id === "string" && item.id) byId.set(item.id, item);
  }
  const blockers = new Map<string, string[]>();
  for (const edge of edges) {
    if (edge.kind !== "blocks" || !edge.from_id || !edge.to_id || edge.from_id === edge.to_id) continue;
    const blocker = byId.get(edge.from_id);
    if (!blocker || blocker.status === "completed" || blocker.status === "cancelled") continue;
    const titles = blockers.get(edge.to_id) || [];
    titles.push(blocker.title || "Untitled work item");
    blockers.set(edge.to_id, titles);
  }
  return blockers;
}

function blockedByText(titles: string[]): string {
  return titles.length > 1
    ? `Blocked by ${titles[0]} and ${plural(titles.length - 1, "other")}`
    : `Blocked by ${titles[0]}`;
}

function moreItemsText(count: number, verb: string): string {
  return `+${count} more ${count === 1 ? "item" : "items"} not ${verb}`;
}

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function textOf(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

/// One activity row as a sentence about the work item, by title ("Freeze
/// the release branch was completed"). The item ID stays out of the sentence:
/// it is on the item. workGraphEventLine keeps the compact
/// "time · kind · id" form.
export function workGraphEventSentence(
  event: WorkGraphWireEvent,
  context: { titleOf: (itemId: string) => string | undefined; directory?: AgentDirectory },
): string {
  const payload = recordOf(event.payload);
  const item = recordOf(payload?.item) as WorkGraphWireItem | null;
  const attention = recordOf(payload?.attention) as WorkGraphWireBinding | null;
  const edge = recordOf(payload?.edge) as WorkGraphWireEdge | null;
  const execution = recordOf(recordOf(payload?.execution_binding)?.work_ref);
  const itemId = textOf(event.item_id) || textOf(item?.id)
    || textOf(attention?.work_ref?.item_id) || textOf(execution?.item_id);
  const titleOf = (id: string) => (id ? context.titleOf(id) || "" : "");
  const named = titleOf(itemId) || textOf(item?.title);
  const subject = named || "A work item";
  const object = named || "a work item";
  // The agent an attention event is about, by name, when the console knows it.
  const agentOnGoal = bindingAgentName(attention, context.directory);
  const kind = textOf(event.kind).toLowerCase().replace(/^item_/, "");
  switch (kind) {
    case "created":
      return `${subject} was created`;
    case "updated":
      return `${subject} was updated`;
    case "ready":
    case "readiness_observed":
      return `${subject} is ready to start`;
    case "claimed": {
      const owner = item ? workGraphItemOwner(item, context.directory) : null;
      return owner ? `${owner.name} claimed ${object}` : `${subject} was claimed`;
    }
    case "released":
      return `${subject} was released`;
    case "blocked":
      return `${subject} became blocked`;
    case "closed":
      if (item?.status === "completed") return `${subject} was completed`;
      if (item?.status === "failed") return `${subject} failed`;
      if (item?.status === "cancelled") return `${subject} was cancelled`;
      return `${subject} was closed`;
    case "linked": {
      const from = titleOf(textOf(edge?.from_id)) || "A work item";
      const to = titleOf(textOf(edge?.to_id)) || "another work item";
      if (edge?.kind === "parent") return `${from} was placed under ${to}`;
      if (edge?.kind === "blocks") return `${from} now blocks ${to}`;
      return `${from} was linked to ${to}`;
    }
    case "evidence_added":
      return `Evidence was added to ${object}`;
    // Attention events say which agent works toward a goal, never
    // "attention": the agent is put on it, the goal is paused for it, the
    // agent is taken off it.
    case "attention_created":
      return `${agentOnGoal ?? "An agent"} was put on ${object}`;
    case "attention_paused":
      return `${subject} was paused for ${agentOnGoal ?? "its agent"}`;
    case "attention_updated": {
      const state = attention?.status?.state;
      if (state === "paused") return `${subject} was paused for ${agentOnGoal ?? "its agent"}`;
      if (state === "superseded" || state === "stopped") return `${agentOnGoal ?? "An agent"} was taken off ${object}`;
      return `${agentOnGoal ? `${agentOnGoal}'s` : "An agent's"} part in ${object} changed`;
    }
    case "execution_bound":
      return `Work on ${object} started`;
    case "execution_transitioned":
      return `Work on ${object} was updated`;
    case "goal_status_reported":
      return `Progress was reported on ${object}`;
    default: {
      const words = humanizeEnum(kind);
      return words ? `${words}: ${object}` : `Activity on ${object}`;
    }
  }
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

type ItemHandlers = Pick<WorkGraphPanelProps, "onClaim" | "onClose">;
type BindingHandlers = Pick<WorkGraphPanelProps,
  "onGoalConfirm" | "onGoalRequestClose" | "onAttentionPause" | "onAttentionResume" | "onAttentionReassign"
>;

/// Claim and Done for one item, or null when neither applies: Claim for an
/// open item nobody owns, Done for anything not finished and not blocked.
function itemActionButtons(
  item: WorkGraphWireItem,
  itemId: string,
  canManage: boolean,
  { onClaim, onClose }: ItemHandlers,
): React.ReactNode {
  const status = item.status || "open";
  const revision = typeof item.revision === "number" ? item.revision : undefined;
  const title = item.title || "this work item";
  const claim = canManage && onClaim && status === "open" && !workGraphOwnerLabelOf(item);
  const close = canManage && onClose && !TERMINAL_STATUSES.has(status) && status !== "blocked";
  if (!claim && !close) return null;
  return (
    <>
      {claim ? (
        <Button
          size="sm"
          variant="ghost"
          title={`Claim ${title}`}
          data-testid={`workgraph-panel-action:${itemId}:claim`}
          onClick={() => onClaim?.({ itemId, revision })}
        >
          Claim
        </Button>
      ) : null}
      {close ? (
        <Button
          size="sm"
          variant="ghost"
          title={`Mark ${title} as done`}
          data-testid={`workgraph-panel-action:${itemId}:close`}
          onClick={() => onClose?.({ itemId, revision })}
        >
          Done
        </Button>
      ) : null}
    </>
  );
}

function AttentionRow({
  binding,
  goalRevision,
  canManage,
  subject,
  onGoalConfirm,
  onGoalRequestClose,
  onAttentionPause,
  onAttentionResume,
  onAttentionReassign,
}: {
  binding: WorkGraphWireBinding;
  // Bound goal work item's revision: the CAS token for confirm/request-close.
  goalRevision?: number;
  canManage: boolean;
  /// What the binding is on, when it does not sit inside that item's card.
  subject?: React.ReactNode;
} & BindingHandlers) {
  const directory = useAgentDirectory();
  const now = useNow();
  const inputId = React.useId();
  const [reassignOpen, setReassignOpen] = React.useState(false);
  const [reassignIdentity, setReassignIdentity] = React.useState("");
  const bindingId = binding.binding_id || "";
  const revision = binding.machine_state?.revision;
  const statusLabel = workGraphBindingStatusLabel(binding);
  const isActive = statusLabel === "active";
  const isPaused = statusLabel.startsWith("paused");
  const live = isActive || isPaused;
  // Reassign authority is machine-derived from the binding mode upstream:
  // only coordinate-mode bindings can reassign, so others get no affordance.
  const canReassign = live && binding.mode === "coordinate";
  const bindingInput = { bindingId, revision };
  const goalInput = { bindingId, revision: goalRevision };
  if (!bindingId) return null;
  const state = binding.status?.state || "active";
  const target = workGraphBindingTarget(binding, directory);
  const pause = canManage && onAttentionPause && isActive;
  const resume = canManage && onAttentionResume && isPaused;
  const confirm = canManage && onGoalConfirm && live;
  const requestClose = canManage && onGoalRequestClose && live;
  const reassignable = canManage && onAttentionReassign && canReassign;
  const submitReassign = () => {
    const identity = reassignIdentity.trim();
    if (!identity || !onAttentionReassign) return;
    onAttentionReassign({ ...bindingInput, identity });
    setReassignOpen(false);
    setReassignIdentity("");
  };
  return (
    <div className="workgraph-binding" data-testid={`workgraph-panel-binding:${bindingId}`} data-state={state}>
      <div className="workgraph-binding__line">
        <span className="workgraph-binding__summary">
          <StatusDot tone={statusTone(state)} bound />
          <span className="workgraph-binding__text">
            <span className="workgraph-binding__mode" title={`Attention mode: ${binding.mode || "pursue"}`}>
              {workGraphModeWord(binding.mode)}
            </span>
            {" · "}
            {workGraphBindingStateText(binding, now)}
            {target ? <>{" · "}<WorkGraphPerson person={target} /></> : null}
            {subject ? <>{" · on "}{subject}</> : null}
          </span>
        </span>
        {pause || resume || confirm || requestClose || reassignable ? (
          <span className="workgraph-binding__actions">
            {pause ? (
              <Button size="sm" title="Pause this goal for its agent" onClick={() => onAttentionPause?.(bindingInput)}>
                Pause
              </Button>
            ) : null}
            {resume ? (
              <Button size="sm" title="Resume this goal for its agent" onClick={() => onAttentionResume?.(bindingInput)}>
                Resume
              </Button>
            ) : null}
            {confirm ? (
              <Button size="sm" title="Confirm the goal is complete" onClick={() => onGoalConfirm?.(goalInput)}>
                Confirm
              </Button>
            ) : null}
            {requestClose ? (
              <Button size="sm" title="Ask to close the goal" onClick={() => onGoalRequestClose?.(goalInput)}>
                Request close
              </Button>
            ) : null}
            {reassignable ? (
              <Button
                size="sm"
                title="Hand the goal to another agent"
                aria-expanded={reassignOpen}
                onClick={() => setReassignOpen((value) => !value)}
              >
                Reassign
              </Button>
            ) : null}
          </span>
        ) : null}
      </div>
      {reassignOpen && reassignable ? (
        <div className="workgraph-binding__reassign">
          <label className="cc-visually-hidden" htmlFor={inputId}>Agent identity to reassign to</label>
          <input
            id={inputId}
            className="cc-input"
            placeholder="Agent identity"
            value={reassignIdentity}
            onChange={(event) => setReassignIdentity(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                submitReassign();
              }
            }}
            data-testid={`workgraph-panel-reassign-input:${bindingId}`}
          />
          <Button
            size="sm"
            variant="primary"
            disabled={!reassignIdentity.trim()}
            data-testid={`workgraph-panel-reassign-submit:${bindingId}`}
            onClick={submitReassign}
          >
            Assign
          </Button>
        </div>
      ) : null}
    </div>
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
  handlers: ItemHandlers;
  bindingHandlers: BindingHandlers;
  goalRevisionOf: (binding: WorkGraphWireBinding) => number | undefined;
}) {
  const directory = useAgentDirectory();
  const { item, itemId, depth } = row;
  const status = item.status || "open";
  const owner = workGraphItemOwner(item, directory);
  const actions = itemActionButtons(item, itemId, canManage, handlers);
  return (
    <ListRow
      dense
      className="workgraph-row"
      testId={`workgraph-panel-item:${itemId}`}
      dataAttributes={{ "data-status": status, "data-depth": String(Math.min(depth, MAX_INDENT_DEPTH)) }}
      lead={<StatusDot tone={statusTone(status)} />}
      title={item.title || "Untitled work item"}
      titleAttr={item.description || item.title || undefined}
      subtitle={blockers?.length ? blockedByText(blockers) : undefined}
      meta={(
        <>
          {/* The status word comes first so a narrow pane, which keeps only
              the first meta item, still says it; CSS orders it last. */}
          <span className="workgraph-row__status">{workGraphStatusWord(status)}</span>
          <PriorityChip priority={item.priority} className="workgraph-row__priority" />
          {owner ? (
            <WorkGraphPerson person={owner} className="workgraph-row__owner" />
          ) : TERMINAL_STATUSES.has(status) ? null : (
            <span className="workgraph-row__owner workgraph-row__unassigned">Unassigned</span>
          )}
        </>
      )}
      actions={actions ?? undefined}
    >
      {bindings.length ? (
        <div className="workgraph-row__bindings">
          {bindings.map((binding, index) => (
            <AttentionRow
              key={binding.binding_id || `binding-${index}`}
              binding={binding}
              goalRevision={goalRevisionOf(binding)}
              canManage={canManage}
              {...bindingHandlers}
            />
          ))}
        </div>
      ) : null}
    </ListRow>
  );
}

function GoalCard({
  group,
  bindings,
  bindingsOf,
  blockers,
  canManage,
  handlers,
  bindingHandlers,
  goalRevisionOf,
}: {
  group: WorkGraphGoalGroup;
  bindings: WorkGraphWireBinding[];
  bindingsOf: (itemId: string) => WorkGraphWireBinding[];
  blockers: Map<string, string[]>;
  canManage: boolean;
  handlers: ItemHandlers;
  bindingHandlers: BindingHandlers;
  goalRevisionOf: (binding: WorkGraphWireBinding) => number | undefined;
}) {
  const directory = useAgentDirectory();
  const { item, itemId } = group.root;
  const status = item.status || "open";
  const owner = workGraphItemOwner(item, directory);
  const actions = itemActionButtons(item, itemId, canManage, handlers);
  const goalBlockers = blockers.get(itemId);
  const { progress } = group;
  const technical: TechnicalDetail[] = [{ label: "Work item ID", value: itemId }];
  for (const binding of bindings) {
    if (binding.binding_id) technical.push({ label: "Attention binding ID", value: binding.binding_id });
    if (binding.target?.session_id) technical.push({ label: "Session ID", value: binding.target.session_id });
  }
  return (
    <article className="workgraph-goal" data-testid={`workgraph-panel-item:${itemId}`} data-status={status}>
      <div className="workgraph-goal__head">
        <div className="workgraph-goal__name">
          <StatusDot tone={statusTone(status)} />
          <h3 className="workgraph-goal__title" title={item.description || item.title || undefined}>
            {item.title || "Untitled work item"}
          </h3>
        </div>
        <span className="workgraph-goal__chips">
          <PriorityChip priority={item.priority} />
          <Chip tone={statusTone(status)}>{workGraphStatusWord(status)}</Chip>
        </span>
        {actions ? <span className="workgraph-goal__actions">{actions}</span> : null}
        {owner ? <WorkGraphPerson person={owner} avatar={18} className="workgraph-goal__owner" /> : null}
      </div>
      {goalBlockers?.length ? <p className="workgraph-goal__blocked">{blockedByText(goalBlockers)}</p> : null}
      {progress.total > 0 ? (
        <Meter
          className="workgraph-goal__meter"
          label={`${progress.completed} of ${progress.total} done`}
          total={progress.total}
          segments={[
            { value: progress.completed, tone: "ok", label: `${progress.completed} completed` },
            { value: progress.inProgress, tone: "info", label: `${progress.inProgress} in progress` },
            { value: progress.blocked, tone: "warn", label: `${progress.blocked} blocked` },
            { value: progress.failed, tone: "crit", label: `${progress.failed} failed` },
          ]}
        />
      ) : null}
      {bindings.length ? (
        <div className="workgraph-goal__bindings">
          {bindings.map((binding, index) => (
            <AttentionRow
              key={binding.binding_id || `binding-${index}`}
              binding={binding}
              goalRevision={goalRevisionOf(binding)}
              canManage={canManage}
              {...bindingHandlers}
            />
          ))}
        </div>
      ) : null}
      {group.children.length ? (
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
      <TechnicalDetails className="workgraph-goal__tech" items={technical} />
    </article>
  );
}

export function WorkGraphPanel({
  runtimeControl,
  data,
  canManage,
  onRefresh,
  onClaim,
  onClose,
  onGoalConfirm,
  onGoalRequestClose,
  onAttentionPause,
  onAttentionResume,
  onAttentionReassign,
}: WorkGraphPanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const rows = React.useMemo(
    () => buildWorkGraphPanelTree(data.items, data.edges),
    [data.items, data.edges],
  );
  // The snapshot includes terminal rows (the panel is the operator
  // inspection surface), so its size tracks the store's full history. The
  // tree gets the same rendered-row cap and overflow honesty as the graph.
  const visibleCount = Math.min(rows.length, WORKGRAPH_GRAPH_NODE_CAP);
  const treeOverflowCount = rows.length - visibleCount;
  const groups = React.useMemo(() => groupWorkGraphTree(rows, visibleCount), [rows, visibleCount]);
  const blockers = React.useMemo(
    () => workGraphBlockersOf(data.items, data.edges),
    [data.items, data.edges],
  );
  const itemById = React.useMemo(() => {
    const map = new Map<string, WorkGraphWireItem>();
    for (const item of data.items) {
      if (typeof item.id === "string" && item.id) map.set(item.id, item);
    }
    return map;
  }, [data.items]);
  const [viewMode, setViewModeState] = React.useState<WorkGraphViewMode>(workGraphViewModeMemory);
  const [selectedGraphItem, setSelectedGraphItem] = React.useState<string | null>(null);
  const setViewMode = (mode: WorkGraphViewMode) => {
    workGraphViewModeMemory = mode;
    setViewModeState(mode);
  };

  if (data.unavailable) {
    return (
      <PanelShell className="workgraph" testId="workgraph-panel">
        <PanelHeader className="workgraph__head" title="WorkGraph" actions={runtimeControl || undefined} />
        <PanelBody>
          <EmptyState
            kind="unavailable"
            title="WorkGraph isn't set up on this runtime"
            body="Work items appear here once the runtime runs a WorkGraph service."
          />
        </PanelBody>
      </PanelShell>
    );
  }

  const handlers: ItemHandlers = { onClaim, onClose };
  const bindingHandlers: BindingHandlers = {
    onGoalConfirm,
    onGoalRequestClose,
    onAttentionPause,
    onAttentionResume,
    onAttentionReassign,
  };
  const goalRevisionOf = (binding: WorkGraphWireBinding) => workGraphGoalRevisionOf(binding, data.items);
  const titleOf = (itemId: string) => itemById.get(itemId)?.title;
  // A binding renders with its bound item: in the goal's card, under a child
  // row, or (item not rendered) in "Agents on other goals".
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
  const bindingSubject = (binding: WorkGraphWireBinding): React.ReactNode => {
    const itemId = binding.work_ref?.item_id;
    if (!itemId) return undefined;
    const title = titleOf(itemId);
    return title || <TechnicalId value={itemId} label="Work item ID" />;
  };
  const attentionList = (bindings: WorkGraphWireBinding[]) => (
    <div className="workgraph__attention">
      {bindings.map((binding, index) => (
        <AttentionRow
          key={binding.binding_id || `binding-${index}`}
          binding={binding}
          goalRevision={goalRevisionOf(binding)}
          canManage={canManage}
          subject={bindingSubject(binding)}
          {...bindingHandlers}
        />
      ))}
    </div>
  );

  // A loaded snapshot always carries its capture time; until one arrives
  // (and nothing failed) the panel is loading.
  const loaded = Boolean(data.capturedAt);
  const loading = !loaded && !data.error && !data.denied
    && rows.length === 0 && data.attention.length === 0 && data.events.length === 0;
  const capturedLabel = formatWorkGraphTimestamp(data.capturedAt, { date: true, seconds: true });

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
    content = <LoadingState variant="cards" rows={3} label="Loading work items" />;
  } else if (!loaded && data.error && rows.length === 0) {
    content = (
      <EmptyState
        kind="unavailable"
        title="Work items could not be loaded"
        body="Refresh to try again."
      />
    );
  } else {
    content = (
      <>
        {viewMode === "graph" ? (
          <>
            <PanelSection className="workgraph__graph" padded>
              <WorkGraphGraphView
                items={data.items}
                edges={data.edges}
                attention={data.attention}
                selectedId={selectedGraphItem ?? undefined}
                onSelect={setSelectedGraphItem}
              />
            </PanelSection>
            {data.attention.length ? (
              <PanelSection label={<span title="Attention bindings">Agents on goals</span>} count={data.attention.length}>
                {attentionList(data.attention)}
              </PanelSection>
            ) : null}
          </>
        ) : (
          <>
            {rows.length === 0 ? (
              <EmptyState
                title="No work items yet"
                body="Agents add work items here as they plan and track their goals."
              />
            ) : null}
            {goals.length ? (
              <div className="workgraph__goals">
                {goals.map((group) => (
                  <GoalCard
                    key={group.root.itemId}
                    group={group}
                    bindings={bindingsOf(group.root.itemId)}
                    bindingsOf={bindingsOf}
                    blockers={blockers}
                    canManage={canManage}
                    handlers={handlers}
                    bindingHandlers={bindingHandlers}
                    goalRevisionOf={goalRevisionOf}
                  />
                ))}
              </div>
            ) : null}
            {looseItems.length ? (
              <PanelSection label={goals.length ? "Other work items" : "Work items"} count={looseItems.length}>
                <RowList>
                  {looseItems.map((group) => (
                    <ItemRow
                      key={group.root.itemId}
                      row={group.root}
                      blockers={blockers.get(group.root.itemId)}
                      bindings={[]}
                      canManage={canManage}
                      handlers={handlers}
                      bindingHandlers={bindingHandlers}
                      goalRevisionOf={goalRevisionOf}
                    />
                  ))}
                </RowList>
              </PanelSection>
            ) : null}
            {treeOverflowCount > 0 ? (
              <p className="workgraph__overflow" data-testid="workgraph-panel-overflow">
                {moreItemsText(treeOverflowCount, "shown")}
              </p>
            ) : null}
            {otherAttention.length ? (
              <PanelSection label={<span title="Attention bindings">Agents on other goals</span>} count={otherAttention.length}>
                {attentionList(otherAttention)}
              </PanelSection>
            ) : null}
          </>
        )}
        <PanelSection label="Activity" count={data.events.length || null} className="workgraph__activity">
          {data.events.length === 0 ? (
            <EmptyState inline title="No activity yet" />
          ) : (
            <RowList>
              {data.events.map((event, index) => {
                const sentence = workGraphEventSentence(event, { titleOf, directory });
                return (
                  <ListRow
                    key={`${event.seq ?? index}`}
                    dense
                    className="workgraph-event"
                    title={sentence}
                    titleAttr={sentence}
                    meta={<RelativeTime value={event.at} />}
                  />
                );
              })}
            </RowList>
          )}
        </PanelSection>
      </>
    );
  }

  return (
    <PanelShell className="workgraph" testId="workgraph-panel">
      <PanelHeader
        className="workgraph__head"
        title="WorkGraph"
        subtitle={data.denied ? undefined : loading ? "Loading work items" : workGraphSummary(data.items)}
        actions={(
          <>
            {runtimeControl}
            <SegmentedControl
              ariaLabel="WorkGraph view mode"
              value={viewMode}
              onChange={setViewMode}
              options={[
                { id: "tree", label: "Tree", testId: "workgraph-view-toggle:tree" },
                { id: "graph", label: "Graph", testId: "workgraph-view-toggle:graph" },
              ]}
            />
            <Button
              icon={<KitGlyph name="refresh" />}
              collapsible
              aria-label="Refresh"
              title="Refresh work items"
              data-testid="workgraph-panel-refresh"
              onClick={onRefresh}
            >
              Refresh
            </Button>
          </>
        )}
      />
      {/* A failed refresh keeps the last items on screen. Its error takes the
          reserved status row below the header, so it never moves the header
          controls or the items under a click in progress. */}
      <StatusRow
        error={data.error}
        updatedAt={data.capturedAt}
        updatedTitle={capturedLabel ? `Snapshot as of ${capturedLabel}` : undefined}
      />
      <PanelBody className="workgraph__body">{content}</PanelBody>
    </PanelShell>
  );
}
